//! A small HTTP/1.1 reader for AirDrop's three POST endpoints.
//!
//! Apple sends `Expect: 100-continue` and, for the file itself, chunked bodies.
//! The parser caps headers and the body so a malformed request cannot grow without limit.

use std::collections::HashMap;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{self, Error, Result};

const MAX_HEADERS: usize = 64 * 1024;
const BODY_STEP: usize = 256 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub(crate) struct Head {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pending: Vec<u8>,
}

pub async fn read_request<S>(io: &mut S, max_body: usize) -> Result<Request>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    read_request_reporting(io, max_body, &|_, _| {}).await
}

pub(crate) async fn read_request_reporting<S>(
    io: &mut S,
    max_body: usize,
    on_body: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<Request>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = read_head(io).await?;
    answer_continue(io, &head).await?;
    let body = read_body(io, &mut head, max_body, on_body).await?;
    Ok(Request {
        method: head.method,
        path: head.path,
        headers: head.headers,
        body,
    })
}

pub(crate) async fn read_head<S>(io: &mut S) -> Result<Head>
where
    S: AsyncRead + Unpin,
{
    let (raw, pending) = read_headers(io).await?;
    let mut lines = raw.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| Error::protocol("missing method"))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| Error::protocol("missing path"))?
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(Head {
        method,
        path,
        headers,
        pending,
    })
}

pub(crate) async fn answer_continue<S>(io: &mut S, head: &Head) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    if head
        .headers
        .get("expect")
        .is_some_and(|value| value.to_ascii_lowercase().contains("100-continue"))
    {
        io.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
        io.flush().await?;
    }
    Ok(())
}

pub(crate) async fn read_body<S>(
    io: &mut S,
    head: &mut Head,
    max_body: usize,
    on_body: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    if head
        .headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        return read_chunked(io, &mut head.pending, max_body, on_body).await;
    }
    if let Some(length) = head.headers.get("content-length") {
        let length: usize = length
            .parse()
            .map_err(|_| Error::protocol("bad content-length"))?;
        if length > max_body {
            return Err(Error::TooLarge);
        }
        return read_counted(io, &mut head.pending, length, on_body).await;
    }
    Ok(std::mem::take(&mut head.pending))
}

pub async fn write_response<S>(
    io: &mut S,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    io.write_all(head.as_bytes()).await?;
    io.write_all(body).await?;
    io.flush().await?;
    Ok(())
}

async fn read_headers<S>(io: &mut S) -> Result<(String, Vec<u8>)>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let read = match io.read(&mut tmp).await {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::Closed);
            }
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            if buf.is_empty() {
                return Err(Error::Closed);
            }
            return Err(Error::protocol("connection closed during headers"));
        }
        buf.extend_from_slice(&tmp[..read]);
        if let Some(end) = find_subsequence(&buf, b"\r\n\r\n") {
            let head = String::from_utf8(buf[..end].to_vec())
                .map_err(|_| Error::protocol("headers are not utf-8"))?;
            let pending = buf[end + 4..].to_vec();
            return Ok((head, pending));
        }
        if buf.len() > MAX_HEADERS {
            return Err(Error::protocol("headers are too large"));
        }
    }
}

async fn read_counted<S>(
    io: &mut S,
    pending: &mut Vec<u8>,
    length: usize,
    on_body: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    if length == 0 {
        return Ok(Vec::new());
    }
    let total = length as u64;
    on_body(0, total);
    if pending.len() >= length {
        let extra = pending.split_off(length);
        let body = std::mem::replace(pending, extra);
        on_body(body.len() as u64, total);
        return Ok(body);
    }
    let mut body = std::mem::take(pending);
    while body.len() < length {
        let step = (length - body.len()).min(BODY_STEP);
        let start = body.len();
        body.resize(start + step, 0);
        error::read_exact(io, &mut body[start..]).await?;
        on_body(body.len() as u64, total);
    }
    Ok(body)
}

async fn read_chunked<S>(
    io: &mut S,
    pending: &mut Vec<u8>,
    max_body: usize,
    on_body: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut incoming = std::mem::take(pending);
    let mut output = Vec::new();
    loop {
        let line = read_line(io, &mut incoming).await?;
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size =
            usize::from_str_radix(size_text, 16).map_err(|_| Error::protocol("bad chunk size"))?;
        if size == 0 {
            // Trailer, terminated by an empty line.
            loop {
                let trailer = read_line(io, &mut incoming).await?;
                if trailer.is_empty() {
                    break;
                }
            }
            break;
        }
        if output.len() + size > max_body {
            return Err(Error::TooLarge);
        }
        let mut chunk = vec![0u8; size];
        read_into(io, &mut incoming, &mut chunk).await?;
        output.extend_from_slice(&chunk);
        on_body(output.len() as u64, 0);
        let crlf = read_line(io, &mut incoming).await?;
        if !crlf.is_empty() {
            return Err(Error::protocol("chunk is missing CRLF"));
        }
    }
    Ok(output)
}

async fn read_line<S>(io: &mut S, pending: &mut Vec<u8>) -> Result<String>
where
    S: AsyncRead + Unpin,
{
    loop {
        if let Some(end) = find_subsequence(pending, b"\r\n") {
            let line = String::from_utf8(pending[..end].to_vec())
                .map_err(|_| Error::protocol("chunk line is not utf-8"))?;
            pending.drain(..end + 2);
            return Ok(line);
        }
        let mut byte = [0u8; 1];
        let read = match io.read(&mut byte).await {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::Closed);
            }
            Err(error) => return Err(error.into()),
        };
        if read == 0 {
            return Err(Error::Closed);
        }
        pending.push(byte[0]);
        if pending.len() > MAX_HEADERS {
            return Err(Error::protocol("chunk line is too long"));
        }
    }
}

async fn read_into<S>(io: &mut S, pending: &mut Vec<u8>, dest: &mut [u8]) -> Result<()>
where
    S: AsyncRead + Unpin,
{
    let from_pending = pending.len().min(dest.len());
    dest[..from_pending].copy_from_slice(&pending[..from_pending]);
    pending.drain(..from_pending);
    if from_pending < dest.len() {
        error::read_exact(io, &mut dest[from_pending..]).await?;
    }
    Ok(())
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{read_request, write_response};

    #[tokio::test]
    async fn reads_content_length_chunked_and_continue() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let client_task = tokio::spawn(async move {
            let request = b"POST /Discover HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nplist";
            tokio::io::AsyncWriteExt::write_all(&mut client, request)
                .await
                .unwrap();
            let mut buf = vec![0u8; 256];
            let n = tokio::io::AsyncReadExt::read(&mut client, &mut buf)
                .await
                .unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("200"));

            let chunked = b"POST /Upload HTTP/1.1\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n";
            tokio::io::AsyncWriteExt::write_all(&mut client, chunked)
                .await
                .unwrap();
            let mut cont = [0u8; 64];
            let n = tokio::io::AsyncReadExt::read(&mut client, &mut cont)
                .await
                .unwrap();
            assert!(std::str::from_utf8(&cont[..n]).unwrap().contains("100"));
            let body = b"5\r\nhello\r\n0\r\n\r\n";
            tokio::io::AsyncWriteExt::write_all(&mut client, body)
                .await
                .unwrap();
            let n = tokio::io::AsyncReadExt::read(&mut client, &mut buf)
                .await
                .unwrap();
            assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("204"));
        });

        let first = read_request(&mut server, 1024).await.unwrap();
        assert_eq!(first.path, "/Discover");
        assert_eq!(first.body, b"plist");
        write_response(&mut server, 200, "OK", "application/octet-stream", b"ok")
            .await
            .unwrap();
        let second = read_request(&mut server, 1024).await.unwrap();
        assert_eq!(second.body, b"hello");
        write_response(&mut server, 204, "No Content", "text/plain", b"")
            .await
            .unwrap();
        client_task.await.unwrap();
    }
}
