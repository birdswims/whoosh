//! Whoosh native control messages.
//!
//! The client opens one bidirectional QUIC stream and writes the 8-byte magic
//! `WHOOSH01`, then length-prefixed messages. The server writes length-prefixed
//! messages on the same stream and does not repeat the magic. Each file is a
//! unidirectional stream: `u32 file_id`, `u64 size`, `size` bytes, then 32-byte
//! BLAKE3. Integers are little-endian.

use crate::error::{Error, Result};

pub const MAGIC: &[u8; 8] = b"WHOOSH01";
pub const MAX_NAME: usize = 1024;
pub const MAX_FILES: usize = 4096;
pub const MAX_MIME: usize = 128;
pub const MAX_REASON: usize = 512;

const HELLO: u8 = 1;
const OFFER: u8 = 2;
const ACCEPT: u8 = 3;
const REJECT: u8 = 4;
const DONE: u8 = 5;

const FLAG_PIN: u8 = 0b0000_0001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub name: String,
    pub device_id: [u8; 16],
    pub pin_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOffer {
    pub id: u32,
    pub size: u64,
    pub name: String,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub transfer_id: [u8; 16],
    pub pin: String,
    pub files: Vec<FileOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Hello(Hello),
    Offer(Offer),
    Accept {
        transfer_id: [u8; 16],
    },
    Reject {
        transfer_id: [u8; 16],
        reason: String,
    },
    Done {
        transfer_id: [u8; 16],
    },
}

pub fn encode(message: &Control) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    match message {
        Control::Hello(hello) => {
            body.push(HELLO);
            write_str(&mut body, &hello.name, MAX_NAME)?;
            body.extend_from_slice(&hello.device_id);
            body.push(if hello.pin_required { FLAG_PIN } else { 0 });
        }
        Control::Offer(offer) => {
            body.push(OFFER);
            body.extend_from_slice(&offer.transfer_id);
            write_str(&mut body, &offer.pin, 16)?;
            if offer.files.len() > MAX_FILES {
                return Err(Error::protocol("too many files"));
            }
            body.extend_from_slice(&(offer.files.len() as u32).to_le_bytes());
            for file in &offer.files {
                body.extend_from_slice(&file.id.to_le_bytes());
                body.extend_from_slice(&file.size.to_le_bytes());
                write_str(&mut body, &file.name, MAX_NAME)?;
                write_str(&mut body, &file.mime, MAX_MIME)?;
            }
        }
        Control::Accept { transfer_id } => {
            body.push(ACCEPT);
            body.extend_from_slice(transfer_id);
        }
        Control::Reject {
            transfer_id,
            reason,
        } => {
            body.push(REJECT);
            body.extend_from_slice(transfer_id);
            write_str(&mut body, reason, MAX_REASON)?;
        }
        Control::Done { transfer_id } => {
            body.push(DONE);
            body.extend_from_slice(transfer_id);
        }
    }
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

pub fn decode(mut bytes: &[u8]) -> Result<(Control, usize)> {
    if bytes.len() < 4 {
        return Err(Error::protocol("short control frame"));
    }
    let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    if bytes.len() < 4 + len || len == 0 {
        return Err(Error::protocol("truncated control frame"));
    }
    let body = &bytes[4..4 + len];
    bytes = body;
    let tag = take_u8(&mut bytes)?;
    let message = match tag {
        HELLO => {
            let name = take_str(&mut bytes, MAX_NAME)?;
            let mut device_id = [0u8; 16];
            take_bytes(&mut bytes, &mut device_id)?;
            let flags = take_u8(&mut bytes)?;
            Control::Hello(Hello {
                name,
                device_id,
                pin_required: flags & FLAG_PIN != 0,
            })
        }
        OFFER => {
            let mut transfer_id = [0u8; 16];
            take_bytes(&mut bytes, &mut transfer_id)?;
            let pin = take_str(&mut bytes, 16)?;
            let count = take_u32(&mut bytes)? as usize;
            if count > MAX_FILES {
                return Err(Error::protocol("too many files"));
            }
            let mut files = Vec::with_capacity(count);
            for _ in 0..count {
                files.push(FileOffer {
                    id: take_u32(&mut bytes)?,
                    size: take_u64(&mut bytes)?,
                    name: take_str(&mut bytes, MAX_NAME)?,
                    mime: take_str(&mut bytes, MAX_MIME)?,
                });
            }
            Control::Offer(Offer {
                transfer_id,
                pin,
                files,
            })
        }
        ACCEPT => {
            let mut transfer_id = [0u8; 16];
            take_bytes(&mut bytes, &mut transfer_id)?;
            Control::Accept { transfer_id }
        }
        REJECT => {
            let mut transfer_id = [0u8; 16];
            take_bytes(&mut bytes, &mut transfer_id)?;
            Control::Reject {
                transfer_id,
                reason: take_str(&mut bytes, MAX_REASON)?,
            }
        }
        DONE => {
            let mut transfer_id = [0u8; 16];
            take_bytes(&mut bytes, &mut transfer_id)?;
            Control::Done { transfer_id }
        }
        _ => return Err(Error::protocol(format!("unknown control tag {tag}"))),
    };
    if !bytes.is_empty() {
        return Err(Error::protocol("trailing control bytes"));
    }
    Ok((message, 4 + len))
}

fn write_str(out: &mut Vec<u8>, value: &str, max: usize) -> Result<()> {
    if value.len() > max || value.len() > u16::MAX as usize {
        return Err(Error::protocol("field too long"));
    }
    out.extend_from_slice(&(value.len() as u16).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn take_u8(bytes: &mut &[u8]) -> Result<u8> {
    let (&value, rest) = bytes
        .split_first()
        .ok_or_else(|| Error::protocol("truncated"))?;
    *bytes = rest;
    Ok(value)
}

fn take_bytes(bytes: &mut &[u8], dest: &mut [u8]) -> Result<()> {
    if bytes.len() < dest.len() {
        return Err(Error::protocol("truncated"));
    }
    dest.copy_from_slice(&bytes[..dest.len()]);
    *bytes = &bytes[dest.len()..];
    Ok(())
}

fn take_u16(bytes: &mut &[u8]) -> Result<u16> {
    let mut raw = [0u8; 2];
    take_bytes(bytes, &mut raw)?;
    Ok(u16::from_le_bytes(raw))
}

fn take_u32(bytes: &mut &[u8]) -> Result<u32> {
    let mut raw = [0u8; 4];
    take_bytes(bytes, &mut raw)?;
    Ok(u32::from_le_bytes(raw))
}

fn take_u64(bytes: &mut &[u8]) -> Result<u64> {
    let mut raw = [0u8; 8];
    take_bytes(bytes, &mut raw)?;
    Ok(u64::from_le_bytes(raw))
}

fn take_str(bytes: &mut &[u8], max: usize) -> Result<String> {
    let len = take_u16(bytes)? as usize;
    if len > max {
        return Err(Error::protocol("field too long"));
    }
    if bytes.len() < len {
        return Err(Error::protocol("truncated"));
    }
    let text =
        std::str::from_utf8(&bytes[..len]).map_err(|_| Error::protocol("name is not utf-8"))?;
    *bytes = &bytes[len..];
    Ok(text.to_string())
}

pub fn encode_file_header(file_id: u32, size: u64) -> [u8; 12] {
    let mut header = [0u8; 12];
    header[..4].copy_from_slice(&file_id.to_le_bytes());
    header[4..].copy_from_slice(&size.to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, Control, FileOffer, Hello, Offer};

    #[test]
    fn roundtrips_every_message() {
        let messages = [
            Control::Hello(Hello {
                name: "Mac".into(),
                device_id: [7; 16],
                pin_required: true,
            }),
            Control::Offer(Offer {
                transfer_id: [9; 16],
                pin: "0420".into(),
                files: vec![FileOffer {
                    id: 3,
                    size: 99,
                    name: "café.mp4".into(),
                    mime: "video/mp4".into(),
                }],
            }),
            Control::Accept {
                transfer_id: [1; 16],
            },
            Control::Reject {
                transfer_id: [2; 16],
                reason: "no".into(),
            },
            Control::Done {
                transfer_id: [4; 16],
            },
        ];
        for message in messages {
            let bytes = encode(&message).unwrap();
            let (decoded, used) = decode(&bytes).unwrap();
            assert_eq!(used, bytes.len());
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn rejects_a_truncated_frame() {
        assert!(decode(&[2, 0, 0, 0, 1]).is_err());
    }
}
