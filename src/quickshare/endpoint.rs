//! Quick Share Wi-Fi LAN endpoint info and mDNS service name.
//!
//! The record layout is the one documented by the Nearby Share protocol notes:
//! a bit field, 16 identity bytes, an optional length-prefixed name, then TLVs.

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub const SERVICE_TYPE: &str = "_FC9F5ED42C8A._tcp.local.";
pub const PCP: u8 = 0x23;
pub const DEVICE_PHONE: u8 = 1;
pub const DEVICE_LAPTOP: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointInfo {
    pub version: u8,
    pub hidden: bool,
    pub device_type: u8,
    pub salt: [u8; 2],
    pub metadata_key: [u8; 14],
    pub name: Option<String>,
    pub tlvs: Vec<(u8, Vec<u8>)>,
}

impl EndpointInfo {
    pub fn visible(name: &str, device_type: u8) -> Self {
        let mut salt = [0u8; 2];
        let mut metadata_key = [0u8; 14];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut salt);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut metadata_key);
        Self {
            version: 1,
            hidden: false,
            device_type,
            salt,
            metadata_key,
            name: Some(truncate_utf8(name, 64)),
            tlvs: Vec::new(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(64);
        out.push(pack(self.version, self.hidden, self.device_type));
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&self.metadata_key);
        if !self.hidden {
            let name = self
                .name
                .as_deref()
                .ok_or_else(|| Error::protocol("visible endpoint is missing a name"))?;
            let name = truncate_utf8(name, 255);
            out.push(name.len() as u8);
            out.extend_from_slice(name.as_bytes());
        }
        for (kind, value) in &self.tlvs {
            if value.len() > 255 {
                return Err(Error::protocol("endpoint tlv is too long"));
            }
            out.push(*kind);
            out.push(value.len() as u8);
            out.extend_from_slice(value);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 17 {
            return Err(Error::protocol("endpoint info is too short"));
        }
        let (version, hidden, device_type) = unpack(bytes[0]);
        let mut salt = [0u8; 2];
        let mut metadata_key = [0u8; 14];
        salt.copy_from_slice(&bytes[1..3]);
        metadata_key.copy_from_slice(&bytes[3..17]);
        let mut rest = &bytes[17..];
        let name = if hidden {
            None
        } else {
            if rest.is_empty() {
                return Err(Error::protocol("endpoint name is missing"));
            }
            let len = rest[0] as usize;
            rest = &rest[1..];
            if rest.len() < len {
                return Err(Error::protocol("endpoint name is truncated"));
            }
            let name = std::str::from_utf8(&rest[..len])
                .map_err(|_| Error::protocol("endpoint name is not utf-8"))?
                .to_string();
            rest = &rest[len..];
            Some(name)
        };
        let mut tlvs = Vec::new();
        while !rest.is_empty() {
            if rest.len() < 2 {
                return Err(Error::protocol("truncated endpoint tlv"));
            }
            let kind = rest[0];
            let len = rest[1] as usize;
            rest = &rest[2..];
            if rest.len() < len {
                return Err(Error::protocol("truncated endpoint tlv"));
            }
            tlvs.push((kind, rest[..len].to_vec()));
            rest = &rest[len..];
        }
        Ok(Self {
            version,
            hidden,
            device_type,
            salt,
            metadata_key,
            name,
            tlvs,
        })
    }

    pub fn encode_txt(&self) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(self.encode()?))
    }
}

pub fn service_instance_name(endpoint_id: &str) -> Result<String> {
    if endpoint_id.len() != 4 || !endpoint_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(Error::protocol(
            "endpoint id must be 4 alphanumeric characters",
        ));
    }
    let mut raw = [0u8; 10];
    raw[0] = PCP;
    raw[1..5].copy_from_slice(endpoint_id.as_bytes());
    raw[5..8].copy_from_slice(&nearby_service_prefix());
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

pub fn parse_instance_name(name: &str) -> Result<String> {
    let label = name.split('.').next().unwrap_or(name);
    let bytes = decode_b64(label)?;
    if bytes.len() != 10 || bytes[0] != PCP || bytes[5..8] != nearby_service_prefix() {
        return Err(Error::protocol("not a quick share service name"));
    }
    let id = std::str::from_utf8(&bytes[1..5])
        .map_err(|_| Error::protocol("endpoint id is not utf-8"))?;
    Ok(id.to_string())
}

pub fn nearby_service_prefix() -> [u8; 3] {
    let digest = Sha256::digest(b"NearbySharing");
    [digest[0], digest[1], digest[2]]
}

pub fn random_endpoint_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut raw = [0u8; 4];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
    raw.into_iter()
        .map(|byte| ALPHABET[(byte as usize) % ALPHABET.len()] as char)
        .collect()
}

fn pack(version: u8, hidden: bool, device_type: u8) -> u8 {
    ((version & 0b111) << 5) | ((u8::from(hidden)) << 4) | ((device_type & 0b111) << 1)
}

fn unpack(byte: u8) -> (u8, bool, u8) {
    let version = (byte >> 5) & 0b111;
    let hidden = ((byte >> 4) & 0b1) == 1;
    let device_type = (byte >> 1) & 0b111;
    (version, hidden, device_type)
}

fn truncate_utf8(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub fn decode_b64(value: &str) -> Result<Vec<u8>> {
    let mut padded = value.to_string();
    let extra = (4 - padded.len() % 4) % 4;
    padded.extend(std::iter::repeat('=').take(extra));
    URL_SAFE
        .decode(padded)
        .map_err(|error| Error::protocol(format!("base64: {error}")))
}

#[cfg(test)]
mod tests {
    use super::{
        nearby_service_prefix, parse_instance_name, service_instance_name, EndpointInfo,
        DEVICE_LAPTOP,
    };

    #[test]
    fn service_prefix_is_the_nearby_sharing_digest() {
        assert_eq!(nearby_service_prefix(), [0xFC, 0x9F, 0x5E]);
        let digest = <sha2::Sha256 as sha2::Digest>::digest(b"NearbySharing");
        assert_eq!(&digest[..6], &[0xFC, 0x9F, 0x5E, 0xD4, 0x2C, 0x8A]);
    }

    #[test]
    fn endpoint_info_roundtrips_and_packs_bits() {
        let mut info = EndpointInfo::visible("Harry's Laptop", DEVICE_LAPTOP);
        info.salt = [1, 2];
        info.metadata_key = [3; 14];
        info.tlvs.push((2, vec![0]));
        let bytes = info.encode().unwrap();
        assert_eq!(bytes[0], 0b0010_0110);
        let decoded = EndpointInfo::decode(&bytes).unwrap();
        assert_eq!(decoded, info);
        assert_eq!(
            EndpointInfo::decode(&decode_txt(&info.encode_txt().unwrap())).unwrap(),
            info
        );
    }

    #[test]
    fn hidden_endpoint_omits_the_name() {
        let info = EndpointInfo {
            version: 1,
            hidden: true,
            device_type: DEVICE_LAPTOP,
            salt: [0; 2],
            metadata_key: [0; 14],
            name: None,
            tlvs: vec![(1, vec![9, 9])],
        };
        let decoded = EndpointInfo::decode(&info.encode().unwrap()).unwrap();
        assert!(decoded.name.is_none());
        assert_eq!(decoded.tlvs, vec![(1, vec![9, 9])]);
    }

    #[test]
    fn instance_name_roundtrips() {
        let name = service_instance_name("Ab12").unwrap();
        assert!(!name.contains('='));
        assert_eq!(parse_instance_name(&name).unwrap(), "Ab12");
        assert_eq!(
            parse_instance_name(&format!("{name}._FC9F5ED42C8A._tcp.local.")).unwrap(),
            "Ab12"
        );
    }

    fn decode_txt(value: &str) -> Vec<u8> {
        super::decode_b64(value).unwrap()
    }
}
