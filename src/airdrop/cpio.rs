//! CPIO "newc" archives, gzip, and the length-prefixed DVZip wrapper used by AirDrop uploads.

use std::io::{self, Read, Write};

use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression;

use crate::error::{Error, Result};
use crate::paths::archive_member_name;

const HEADER_LEN: usize = 110;

pub struct ArchiveEntry {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub fn pack_gzip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let raw = pack_newc(files)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw)?;
    Ok(encoder.finish()?)
}

#[allow(dead_code)]
pub fn pack_dvzip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let raw = pack_newc(files)?;
    let mut out = Vec::new();
    for chunk in raw.chunks(64 * 1024) {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(chunk)?;
        let compressed = encoder.finish()?;
        out.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
        out.extend_from_slice(&compressed);
    }
    out.extend_from_slice(&0u32.to_be_bytes());
    Ok(out)
}

pub fn unpack(body: &[u8], content_type: &str, max_bytes: u64) -> Result<Vec<ArchiveEntry>> {
    let plain = decode_container(body, content_type, max_bytes)?;
    if plain.len() as u64 > max_bytes {
        return Err(Error::TooLarge);
    }
    let mut entries = Vec::new();
    unpack_reader(&plain[..], max_bytes, |name, bytes| {
        entries.push(ArchiveEntry {
            name: name.to_string(),
            bytes: bytes.to_vec(),
        });
        Ok(())
    })?;
    Ok(entries)
}

pub fn unpack_reader<R, F>(mut reader: R, max_bytes: u64, mut on_file: F) -> Result<()>
where
    R: Read,
    F: FnMut(&str, &[u8]) -> Result<()>,
{
    let mut total = 0u64;
    loop {
        let mut header = [0u8; HEADER_LEN];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(Error::protocol("truncated cpio archive"));
            }
            Err(error) => return Err(error.into()),
        }
        if &header[..6] != b"070701" {
            return Err(Error::protocol("cpio archive is not newc"));
        }
        let namesize = hex_field(&header, 94)?;
        let filesize = hex_field(&header, 54)? as u64;
        if namesize == 0 || namesize > 1024 {
            return Err(Error::protocol("cpio name length"));
        }
        let mut name_buf = vec![0u8; namesize];
        reader.read_exact(&mut name_buf)?;
        skip_pad(&mut reader, HEADER_LEN + namesize)?;
        if filesize > max_bytes || total.saturating_add(filesize) > max_bytes {
            return Err(Error::TooLarge);
        }
        let mut data = vec![0u8; filesize as usize];
        if filesize > 0 {
            reader.read_exact(&mut data)?;
        }
        skip_pad(&mut reader, filesize as usize)?;
        let raw_name = std::str::from_utf8(name_buf.split(|&byte| byte == 0).next().unwrap_or(b""))
            .map_err(|_| Error::protocol("cpio name is not utf-8"))?;
        if raw_name == "TRAILER!!!" {
            break;
        }
        let name = archive_member_name(raw_name)?;
        total += filesize;
        on_file(&name, &data)?;
    }
    Ok(())
}

fn decode_container(body: &[u8], content_type: &str, max_bytes: u64) -> Result<Vec<u8>> {
    let kind = content_type.to_ascii_lowercase();
    if body.starts_with(&[0x1F, 0x8B]) || kind.contains("gzip") {
        return inflate(GzDecoder::new(body), max_bytes);
    }
    if body.starts_with(b"070701") {
        return Ok(body.to_vec());
    }
    if kind.contains("dvzip") || looks_like_dvzip(body) {
        return inflate(DvzipDecoder::new(body), max_bytes);
    }
    inflate(GzDecoder::new(body), max_bytes)
}

fn inflate<R: Read>(mut decoder: R, max_bytes: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = decoder.read(&mut buf)?;
        if read == 0 {
            break;
        }
        if out.len() as u64 + read as u64 > max_bytes {
            return Err(Error::TooLarge);
        }
        out.extend_from_slice(&buf[..read]);
    }
    Ok(out)
}

fn looks_like_dvzip(body: &[u8]) -> bool {
    if body.len() < 6 {
        return false;
    }
    let len = u32::from_be_bytes(body[..4].try_into().unwrap()) as usize;
    len > 0 && len < body.len() && (body[4] == 0x78 || body[4..].starts_with(&[0x1F, 0x8B]))
}

struct DvzipDecoder<'a> {
    input: &'a [u8],
    output: Vec<u8>,
    cursor: usize,
}

impl<'a> DvzipDecoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            output: Vec::new(),
            cursor: 0,
        }
    }

    fn pull(&mut self) -> Result<()> {
        while self.cursor < 4 || self.output.is_empty() {
            if self.input.len() < 4 {
                return Err(Error::protocol("truncated dvzip"));
            }
            let len = u32::from_be_bytes(self.input[..4].try_into().unwrap()) as usize;
            self.input = &self.input[4..];
            if len == 0 {
                return Ok(());
            }
            if self.input.len() < len || len > 16 * 1024 * 1024 {
                return Err(Error::protocol("bad dvzip chunk"));
            }
            let chunk = &self.input[..len];
            self.input = &self.input[len..];
            let decoded = if chunk.starts_with(&[0x1F, 0x8B]) {
                inflate(GzDecoder::new(chunk), u64::MAX / 4)?
            } else {
                inflate(ZlibDecoder::new(chunk), u64::MAX / 4)?
            };
            self.output.extend_from_slice(&decoded);
            if !self.output.is_empty() {
                return Ok(());
            }
        }
        Ok(())
    }
}

impl Read for DvzipDecoder<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cursor >= self.output.len() {
            self.output.clear();
            self.cursor = 0;
            if self.pull().is_err() && self.output.is_empty() {
                return Ok(0);
            }
        }
        if self.cursor >= self.output.len() {
            return Ok(0);
        }
        let n = (self.output.len() - self.cursor).min(buf.len());
        buf[..n].copy_from_slice(&self.output[self.cursor..self.cursor + n]);
        self.cursor += n;
        Ok(n)
    }
}

fn pack_newc(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (name, bytes) in files {
        let safe = archive_member_name(name).or_else(|_| crate::sanitize::safe_file_name(name))?;
        write_entry(&mut out, &format!("./{safe}"), bytes, 0o100644)?;
    }
    write_entry(&mut out, "TRAILER!!!", &[], 0)?;
    Ok(out)
}

fn write_entry(out: &mut Vec<u8>, name: &str, data: &[u8], mode: u32) -> Result<()> {
    let mut name_bytes = name.as_bytes().to_vec();
    name_bytes.push(0);
    if data.len() > u32::MAX as usize || name_bytes.len() > u32::MAX as usize {
        return Err(Error::TooLarge);
    }
    let header = format!(
        "070701{ino:08X}{mode:08X}{uid:08X}{gid:08X}{nlink:08X}{mtime:08X}{filesize:08X}{maj:08X}{min:08X}{rmaj:08X}{rmin:08X}{namesize:08X}{check:08X}",
        ino = 0,
        mode = mode,
        uid = 0,
        gid = 0,
        nlink = 1,
        mtime = 0,
        filesize = data.len(),
        maj = 0,
        min = 0,
        rmaj = 0,
        rmin = 0,
        namesize = name_bytes.len(),
        check = 0,
    );
    debug_assert_eq!(header.len(), HEADER_LEN);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&name_bytes);
    pad4(out, HEADER_LEN + name_bytes.len());
    out.extend_from_slice(data);
    pad4(out, data.len());
    Ok(())
}

fn pad4(out: &mut Vec<u8>, len: usize) {
    let pad = (4 - (len % 4)) % 4;
    out.extend(std::iter::repeat(0).take(pad));
}

fn skip_pad<R: Read>(reader: &mut R, len: usize) -> Result<()> {
    let pad = (4 - (len % 4)) % 4;
    if pad > 0 {
        let mut buf = [0u8; 3];
        reader.read_exact(&mut buf[..pad])?;
    }
    Ok(())
}

fn hex_field(header: &[u8], offset: usize) -> Result<usize> {
    let text = std::str::from_utf8(&header[offset..offset + 8])
        .map_err(|_| Error::protocol("cpio header"))?;
    usize::from_str_radix(text, 16).map_err(|_| Error::protocol("cpio header"))
}

#[cfg(test)]
mod tests {
    use super::{pack_dvzip, pack_gzip, unpack, write_entry};
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    #[test]
    fn gzip_cpio_roundtrips_and_rejects_parents() {
        let packed = pack_gzip(&[
            ("photo.jpg".into(), b"jpeg".to_vec()),
            ("album/clip.mp4".into(), b"mp4".to_vec()),
        ])
        .unwrap();
        let entries = unpack(&packed, "application/x-cpio", 1024).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "photo.jpg");
        assert_eq!(entries[1].name, "clip.mp4");
        assert_eq!(entries[1].bytes, b"mp4");

        let mut raw = Vec::new();
        write_entry(&mut raw, "./../../etc/passwd", b"no", 0o100644).unwrap();
        write_entry(&mut raw, "TRAILER!!!", b"", 0).unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&raw).unwrap();
        let packed = encoder.finish().unwrap();
        assert!(unpack(&packed, "application/x-cpio", 1024).is_err());
    }

    #[test]
    fn dvzip_roundtrips() {
        let packed = pack_dvzip(&[("clip.mov".into(), vec![7; 200_000])]).unwrap();
        let entries = unpack(&packed, "application/x-dvzip", 300_000).unwrap();
        assert_eq!(entries[0].name, "clip.mov");
        assert_eq!(entries[0].bytes.len(), 200_000);
    }

    #[test]
    fn decompression_is_capped() {
        let packed = pack_gzip(&[("big.bin".into(), vec![1; 1000])]).unwrap();
        assert!(unpack(&packed, "application/x-cpio", 100).is_err());
    }
}
