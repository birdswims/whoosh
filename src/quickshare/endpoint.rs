//! Quick Share Wi-Fi LAN endpoint info and mDNS service name.
//!
//! The record layout is the one documented by the Nearby Share protocol notes:
//! a bit field, 16 identity bytes, an optional length-prefixed name, then TLVs.

use std::collections::HashMap;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
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
        // A short or cut-off trailer must not throw away the device name.
        // Phones append vendor bytes that do not always fill a whole TLV.
        let mut tlvs = Vec::new();
        while rest.len() >= 2 {
            let kind = rest[0];
            let len = rest[1] as usize;
            if rest.len() < 2 + len {
                break;
            }
            tlvs.push((kind, rest[2..2 + len].to_vec()));
            rest = &rest[2 + len..];
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

/// Name to show for a Quick Share advertisement.
///
/// The `n` record is usually URL-safe base64 of an endpoint info blob. Some
/// phones put that blob in the TXT value as raw bytes, and some put the
/// device name in the value with no encoding at all. A broken trailer, or
/// the visibility bit, does not discard a plaintext name that is still there.
pub fn nearby_label(
    txt: &HashMap<String, String>,
    raw_n: Option<&[u8]>,
    instance: &str,
    hostname: &str,
) -> String {
    name_from_record(txt.get("n").map(String::as_str), raw_n)
        .or_else(|| plain_record_name(txt.get("n").map(String::as_str)))
        .or_else(|| {
            ["name", "dn", "dev", "device", "fn"]
                .iter()
                .find_map(|key| plain_record_name(txt.get(*key).map(String::as_str)))
        })
        .or_else(|| instance_label(instance))
        .or_else(|| hostname_label(hostname))
        .unwrap_or_else(|| "Quick Share device".into())
}

/// Device name carried in an endpoint-info blob, encoded or raw.
pub(crate) fn name_in_endpoint_info(bytes: &[u8]) -> Option<String> {
    name_from_endpoint_bytes(bytes)
}

fn name_from_record(encoded: Option<&str>, raw: Option<&[u8]>) -> Option<String> {
    // Base64 is itself UTF-8. Reading those letters as an endpoint blob
    // yields a nonsense name, so decode the text form first.
    if let Some(name) = encoded.and_then(name_from_encoded_text) {
        return Some(name);
    }
    let raw = raw?;
    if let Ok(text) = std::str::from_utf8(raw) {
        if looks_like_encoded_blob(text) {
            return None;
        }
    }
    name_from_endpoint_bytes(raw)
}

fn name_from_encoded_text(text: &str) -> Option<String> {
    let compact: String = text
        .trim()
        .trim_matches('"')
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if compact.is_empty() {
        return None;
    }
    let bytes = decode_any_b64(&compact).ok()?;
    name_from_endpoint_bytes(&bytes)
}

fn decode_any_b64(value: &str) -> Result<Vec<u8>> {
    let mut padded = value.to_string();
    if padded.len() % 4 != 1 {
        let extra = (4 - padded.len() % 4) % 4;
        padded.extend(std::iter::repeat('=').take(extra));
    }
    for engine in [&URL_SAFE_NO_PAD, &URL_SAFE, &STANDARD_NO_PAD, &STANDARD] {
        for candidate in [value, padded.as_str()] {
            if let Ok(bytes) = engine.decode(candidate) {
                if bytes.len() >= 17 {
                    return Ok(bytes);
                }
            }
        }
    }
    Err(Error::protocol("endpoint info is too short"))
}

fn name_from_endpoint_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 18 {
        return None;
    }
    if let Ok(info) = EndpointInfo::decode(bytes) {
        if let Some(name) = info.name.as_deref().and_then(human_device_name) {
            return Some(name);
        }
    }
    plaintext_name(bytes)
}

fn plaintext_name(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 18 {
        return None;
    }
    let len = bytes[17] as usize;
    if !(2..=128).contains(&len) || 18 + len > bytes.len() {
        return None;
    }
    let text = std::str::from_utf8(&bytes[18..18 + len]).ok()?;
    let name = human_device_name(text)?;
    if name.chars().count() < 2 {
        return None;
    }
    Some(name)
}

fn plain_record_name(value: Option<&str>) -> Option<String> {
    let value = value?.trim().trim_matches('"');
    if looks_like_encoded_blob(value) {
        return None;
    }
    human_device_name(value)
}

fn looks_like_encoded_blob(value: &str) -> bool {
    let compact: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    compact.len() >= 20
        && compact
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '-' | '_' | '='))
}

fn instance_label(instance: &str) -> Option<String> {
    let instance = instance.trim();
    if instance.is_empty() || parse_instance_name(instance).is_ok() || is_endpoint_token(instance) {
        return None;
    }
    human_device_name(instance)
}

fn is_endpoint_token(value: &str) -> bool {
    matches!(value.len(), 14 | 16)
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '='))
}

fn hostname_label(hostname: &str) -> Option<String> {
    let host = hostname.trim().trim_end_matches('.');
    let host = host.strip_suffix(".local").unwrap_or(host);
    let lower = host.to_ascii_lowercase();
    if lower.is_empty()
        || lower == "localhost"
        || lower == "android"
        || lower.starts_with("android-")
        || lower.starts_with("whoosh-")
        || lower.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return None;
    }
    human_device_name(host)
}

fn human_device_name(value: &str) -> Option<String> {
    let value = value.trim_matches('\0').trim();
    let count = value.chars().count();
    if !(1..=128).contains(&count) {
        return None;
    }
    if value.chars().any(|ch| ch.is_control()) {
        return None;
    }
    // Punctuation alone is not a device name. Letters, digits, and emoji are.
    if !value
        .chars()
        .any(|ch| !ch.is_whitespace() && !ch.is_ascii_punctuation())
    {
        return None;
    }
    Some(value.to_string())
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

    #[test]
    fn a_cut_off_trailer_keeps_the_device_name() {
        let mut info = EndpointInfo::visible("Pixel 8", DEVICE_LAPTOP);
        info.salt = [1, 2];
        info.metadata_key = [3; 14];
        info.tlvs.push((1, vec![9, 8, 7, 6]));
        let mut bytes = info.encode().unwrap();
        bytes.pop();
        let decoded = EndpointInfo::decode(&bytes).unwrap();
        assert_eq!(decoded.name.as_deref(), Some("Pixel 8"));
        assert!(decoded.tlvs.is_empty());
    }

    #[test]
    fn nearby_label_reads_raw_standard_and_plain_names() {
        use std::collections::HashMap;

        use base64::Engine;

        let mut info = EndpointInfo::visible("Pixel 8", DEVICE_LAPTOP);
        info.salt = [0xfb, 0xff];
        info.metadata_key = [0xef; 14];
        let raw = info.encode().unwrap();
        let standard = base64::engine::general_purpose::STANDARD.encode(&raw);
        assert!(standard.contains('+') || standard.contains('/'));

        let mut txt = HashMap::new();
        txt.insert("n".into(), standard);
        assert_eq!(super::nearby_label(&txt, None, "", ""), "Pixel 8");
        txt.insert("n".into(), info.encode_txt().unwrap());
        assert_eq!(super::nearby_label(&txt, None, "", ""), "Pixel 8");

        let mut raw_txt = HashMap::new();
        assert_eq!(super::nearby_label(&raw_txt, Some(&raw), "", ""), "Pixel 8");

        raw_txt.insert("n".into(), "Living Room".into());
        assert_eq!(super::nearby_label(&raw_txt, None, "", ""), "Living Room");

        let token = service_instance_name("Ab12").unwrap();
        assert_eq!(
            super::nearby_label(&HashMap::new(), None, &token, ""),
            "Quick Share device"
        );
        assert_eq!(
            super::nearby_label(&HashMap::new(), None, "Living Room", ""),
            "Living Room"
        );
        assert_eq!(
            super::nearby_label(&HashMap::new(), None, &token, "Harrys-Pixel.local."),
            "Harrys-Pixel"
        );
        assert_eq!(
            super::nearby_label(&HashMap::new(), None, &token, "android-deadbeef.local."),
            "Quick Share device"
        );
    }

    #[test]
    fn a_hidden_record_still_shows_a_plaintext_name() {
        let info = EndpointInfo {
            version: 1,
            hidden: true,
            device_type: DEVICE_LAPTOP,
            salt: [0; 2],
            metadata_key: [0; 14],
            name: None,
            tlvs: Vec::new(),
        };
        let mut bytes = info.encode().unwrap();
        bytes.push(6);
        bytes.extend_from_slice(b"Pixel8");
        assert_eq!(
            super::name_in_endpoint_info(&bytes).as_deref(),
            Some("Pixel8")
        );
    }
}
