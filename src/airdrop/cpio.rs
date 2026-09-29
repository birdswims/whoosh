//! CPIO newc/odc archives, gzip, and the length-prefixed DVZip wrapper used by AirDrop uploads.

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
        let mut magic = [0u8; 6];
        match reader.read_exact(&mut magic) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(Error::protocol("truncated cpio archive"));
            }
            Err(error) => return Err(error.into()),
        }
        let (header_len, aligned) = match &magic {
            b"070701" => (HEADER_LEN, true),
            b"070707" => (76, false),
            _ => return Err(Error::protocol("unsupported cpio archive format")),
        };
        let mut header = vec![0u8; header_len];
        header[..6].copy_from_slice(&magic);
        reader.read_exact(&mut header[6..])?;
        let (namesize, filesize, mode) = if aligned {
            (
                hex_field(&header, 94)?,
                hex_field(&header, 54)?,
                hex_field(&header, 14)?,
            )
        } else {
            // POSIX odc has octal fields and no padding between names/data.
            (
                number_field(&header, 59, 6, 8)?,
                number_field(&header, 65, 11, 8)?,
                number_field(&header, 18, 6, 8)?,
            )
        };
        let filesize = filesize as u64;
        if namesize == 0 || namesize > 1024 {
            return Err(Error::protocol("cpio name length"));
        }
        let mut name_buf = vec![0u8; namesize];
        reader.read_exact(&mut name_buf)?;
        if aligned {
            skip_pad(&mut reader, header_len + namesize)?;
        }
        if filesize > max_bytes || total.saturating_add(filesize) > max_bytes {
            return Err(Error::TooLarge);
        }
        let mut data = vec![0u8; filesize as usize];
        if filesize > 0 {
            reader.read_exact(&mut data)?;
        }
        if aligned {
            skip_pad(&mut reader, filesize as usize)?;
        }
        if name_buf.last() != Some(&0) || name_buf[..namesize - 1].contains(&0) {
            return Err(Error::protocol("cpio name is not NUL terminated"));
        }
        let raw_name = std::str::from_utf8(&name_buf[..namesize - 1])
            .map_err(|_| Error::protocol("cpio name is not utf-8"))?;
        if raw_name == "TRAILER!!!" {
            break;
        }
        if mode & 0o170000 == 0o040000 {
            // Apple/libarchive may include the archive root. We flatten file
            // paths and do not create archive directories.
            if filesize != 0 {
                return Err(Error::protocol("cpio directory contains data"));
            }
            if raw_name != "." && raw_name != "./" {
                archive_member_name(raw_name)?;
            }
            continue;
        }
        if mode & 0o170000 != 0o100000 {
            return Err(Error::protocol("cpio entry is not a regular file"));
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
    if body.starts_with(b"070701") || body.starts_with(b"070707") {
        return Ok(body.to_vec());
    }
    if kind.contains("dvzip") || looks_like_dvzip(body) {
        return decode_dvzip(body, max_bytes);
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
    let header = u32::from_be_bytes(body[..4].try_into().unwrap());
    let len = (header & 0x7fff_ffff) as usize;
    len > 0
        && len <= body.len() - 4
        && (header & 0x8000_0000 != 0 || body[4] == 0x78 || body[4..].starts_with(&[0x1F, 0x8B]))
}

fn decode_dvzip(mut input: &[u8], max_bytes: u64) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    // Apple uploads can end at a block boundary without a zero-length marker.
    while !input.is_empty() {
        if input.len() < 4 {
            return Err(Error::protocol("truncated dvzip header"));
        }
        let header = u32::from_be_bytes(input[..4].try_into().unwrap());
        let stored = header & 0x8000_0000 != 0;
        let len = (header & 0x7fff_ffff) as usize;
        input = &input[4..];
        if len == 0 {
            if !input.is_empty() {
                return Err(Error::protocol("data after dvzip terminator"));
            }
            break;
        }
        if len > input.len() || len > 16 * 1024 * 1024 {
            return Err(Error::protocol("bad dvzip chunk length"));
        }
        let (chunk, rest) = input.split_at(len);
        input = rest;
        let remaining = max_bytes.saturating_sub(output.len() as u64);
        if stored {
            if len as u64 > remaining {
                return Err(Error::TooLarge);
            }
            output.extend_from_slice(chunk);
        } else {
            let decoded = if chunk.starts_with(&[0x1F, 0x8B]) {
                inflate(GzDecoder::new(chunk), remaining)?
            } else {
                inflate(ZlibDecoder::new(chunk), remaining)?
            };
            output.extend_from_slice(&decoded);
        }
    }
    Ok(output)
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
    number_field(header, offset, 8, 16)
}

fn number_field(header: &[u8], offset: usize, len: usize, radix: u32) -> Result<usize> {
    let text = std::str::from_utf8(&header[offset..offset + len])
        .map_err(|_| Error::protocol("cpio header"))?;
    usize::from_str_radix(text, radix).map_err(|_| Error::protocol("cpio header"))
}

#[cfg(test)]
mod tests {
    use super::{pack_dvzip, pack_gzip, unpack, write_entry};
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    const ODC: &[u8] = include_bytes!("../../tests/fixtures/airdrop-odc.cpio");

    #[test]
    fn reads_odc_archive_from_bsdtar() {
        let entries = unpack(ODC, "application/x-cpio", 4096).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "IMG_9457.jpg");
        assert_eq!(entries[0].bytes, b"\xff\xd8\xff\xe0whoosh-fixture\xff\xd9");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(ODC).unwrap();
        let entries = unpack(&encoder.finish().unwrap(), "application/x-cpio", 4096).unwrap();
        assert_eq!(entries[0].name, "IMG_9457.jpg");
    }

    #[test]
    fn odc_rejects_traversal_links_and_oversized_files() {
        let name = b"./IMG_9457.jpg";
        let start = ODC
            .windows(name.len())
            .position(|window| window == name)
            .unwrap();
        let mut traversal = ODC.to_vec();
        traversal[start..start + name.len()].copy_from_slice(b"../IMG9457.jpg");
        assert!(unpack(&traversal, "application/x-cpio", 4096).is_err());
        let mut link = ODC.to_vec();
        link[start - 76 + 18..start - 76 + 24].copy_from_slice(b"120777");
        assert!(unpack(&link, "application/x-cpio", 4096).is_err());
        let mut oversized = ODC.to_vec();
        oversized[start - 11..start].copy_from_slice(b"77777777777");
        assert!(unpack(&oversized, "application/x-cpio", 4096).is_err());
    }

    #[test]
    fn reads_mixed_stored_and_compressed_dvzip_without_terminator() {
        let mut body = Vec::new();
        for (index, chunk) in ODC.chunks(37).enumerate() {
            if index % 2 == 0 {
                body.extend_from_slice(&(0x8000_0000 | chunk.len() as u32).to_be_bytes());
                body.extend_from_slice(chunk);
            } else {
                let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(chunk).unwrap();
                let compressed = encoder.finish().unwrap();
                body.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
                body.extend_from_slice(&compressed);
            }
        }
        let entries = unpack(&body, "application/x-dvzip", 4096).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].bytes, b"\xff\xd8\xff\xe0whoosh-fixture\xff\xd9");
        assert!(unpack(&body, "application/x-dvzip", 64).is_err());
    }

    #[test]
    fn dvzip_does_not_hide_corrupt_or_truncated_blocks() {
        let mut body = pack_dvzip(&[("a.txt".into(), b"hello".to_vec())]).unwrap();
        body.truncate(body.len() - 4); // replace the terminator with a broken block
        for bad in [&[0, 0][..], &[0, 0, 0, 8, 1, 2], &[0, 0, 0, 2, 1, 2]] {
            let mut broken = body.clone();
            broken.extend_from_slice(bad);
            assert!(unpack(&broken, "application/x-dvzip", 4096).is_err());
        }
    }

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
