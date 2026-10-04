//! AirDrop Discover / Ask / Upload bodies.
//!
//! Discover and Ask use Apple binary property lists. Upload is a gzip CPIO
//! archive, or the chunked DVZip container newer Apple senders use when the
//! receiver advertises that it understands it. This receiver does not advertise
//! DVZip and still accepts it.

use plist::{Dictionary, Value};

use crate::airdrop::cpio::{self, ArchiveEntry};
use crate::error::{Error, Result};
use crate::mime::{self, sniff};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskFile {
    pub name: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub sender_name: String,
    pub sender_id: String,
    pub files: Vec<AskFile>,
}

pub fn discover_response(name: &str, model: &str) -> Result<Vec<u8>> {
    let mut dict = Dictionary::new();
    dict.insert(
        "ReceiverComputerName".into(),
        Value::String(name.to_string()),
    );
    dict.insert("ReceiverModelName".into(), Value::String(model.to_string()));
    dict.insert(
        "ReceiverMediaCapabilities".into(),
        Value::Data(br#"{"Version":1}"#.to_vec()),
    );
    write_plist(dict)
}

pub fn ask_response(name: &str, model: &str) -> Result<Vec<u8>> {
    let mut dict = Dictionary::new();
    dict.insert(
        "ReceiverComputerName".into(),
        Value::String(name.to_string()),
    );
    dict.insert("ReceiverModelName".into(), Value::String(model.to_string()));
    write_plist(dict)
}

pub fn parse_ask(body: &[u8]) -> Result<Ask> {
    let value = Value::from_reader(std::io::Cursor::new(body))
        .map_err(|error| Error::protocol(format!("ask plist: {error}")))?;
    let dict = value
        .into_dictionary()
        .ok_or_else(|| Error::protocol("ask body is not a dictionary"))?;
    let sender_name = dict
        .get("SenderComputerName")
        .and_then(Value::as_string)
        .unwrap_or("Apple device")
        .to_string();
    let sender_id = dict
        .get("SenderID")
        .and_then(Value::as_string)
        .unwrap_or("")
        .to_string();
    let mut files = Vec::new();
    if let Some(list) = dict.get("Files").and_then(Value::as_array) {
        for item in list {
            let Some(file) = item.as_dictionary() else {
                continue;
            };
            let name = file
                .get("FileName")
                .and_then(Value::as_string)
                .ok_or_else(|| Error::protocol("ask file is missing FileName"))?;
            tracing::debug!(fields = ?file.keys().collect::<Vec<_>>(), size = ?file.get("FileSize").and_then(plist::Value::as_unsigned_integer), "AirDrop incoming file metadata");
            files.push(AskFile {
                name: name.to_string(),
                size: file.get("FileSize").and_then(Value::as_unsigned_integer),
            });
        }
    }
    if files.is_empty() {
        return Err(Error::protocol("ask request has no files"));
    }
    Ok(Ask {
        sender_name,
        sender_id,
        files,
    })
}

pub fn ask_request(
    sender_name: &str,
    sender_model: &str,
    sender_id: &str,
    files: &[(String, String, u64)],
) -> Result<Vec<u8>> {
    let model = if sender_model.trim().is_empty() {
        "Mac"
    } else {
        sender_model.trim()
    };
    let mut root = Dictionary::new();
    root.insert(
        "SenderComputerName".into(),
        Value::String(sender_name.into()),
    );
    root.insert("BundleID".into(), Value::String("com.apple.finder".into()));
    // iOS shows the accept prompt for a Mac model identifier. A made-up model
    // name is dropped and the phone never asks the person to accept.
    root.insert("SenderModelName".into(), Value::String(model.into()));
    root.insert("SenderID".into(), Value::String(sender_id.into()));
    root.insert("ConvertMediaFormats".into(), Value::Boolean(false));
    let entries = files
        .iter()
        .map(|(name, mime, size)| {
            let mut file = Dictionary::new();
            file.insert("FileName".into(), Value::String(name.clone()));
            file.insert(
                "FileType".into(),
                Value::String(mime::uniform_type(mime).to_string()),
            );
            file.insert("FileBomPath".into(), Value::String(format!("./{name}")));
            file.insert("FileIsDirectory".into(), Value::Boolean(false));
            // Apple's file record uses an integer here, not a boolean.
            file.insert("ConvertMediaFormats".into(), Value::Integer(0.into()));
            file.insert(
                "FileSize".into(),
                Value::Integer(i64::try_from(*size).unwrap_or(i64::MAX).into()),
            );
            Value::Dictionary(file)
        })
        .collect();
    root.insert("Files".into(), Value::Array(entries));
    write_plist(root)
}

/// What a receiver said in its Discover reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverReply {
    pub name: Option<String>,
    /// `IsAirDropable`. Absent when this iOS build does not send the key.
    pub accepts: Option<bool>,
}

pub fn discover_reply(body: &[u8]) -> Option<DiscoverReply> {
    let value = Value::from_reader(std::io::Cursor::new(body)).ok()?;
    let dict = value.as_dictionary()?;
    let name = dict
        .get("ReceiverComputerName")
        .and_then(Value::as_string)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let accepts = dict.get("IsAirDropable").and_then(plist_bool);
    Some(DiscoverReply { name, accepts })
}

fn plist_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Boolean(flag) => Some(*flag),
        Value::Integer(flag) => Some(flag.as_signed().unwrap_or(0) != 0),
        _ => None,
    }
}

pub fn discover_request() -> Result<Vec<u8>> {
    write_plist(Dictionary::new())
}

pub fn receiver_computer_name(body: &[u8]) -> Option<String> {
    discover_reply(body)?.name
}

pub fn extract_upload(
    body: &[u8],
    content_type: &str,
    max_bytes: u64,
) -> Result<Vec<ArchiveEntry>> {
    cpio::unpack(body, content_type, max_bytes)
}

pub fn build_upload(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    cpio::pack_gzip(files)
}

pub fn describe(name: &str, bytes: &[u8]) -> (String, String) {
    let (kind, mime) = sniff(bytes, name);
    let _ = kind;
    (mime.to_string(), mime::uniform_type(mime).to_string())
}

fn write_plist(dict: Dictionary) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    Value::Dictionary(dict)
        .to_writer_binary(&mut buf)
        .map_err(|error| Error::protocol(format!("plist: {error}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::{ask_request, ask_response, discover_response, parse_ask};

    #[test]
    fn discover_and_ask_plists_roundtrip() {
        let body = discover_response("Harry", "Whoosh").unwrap();
        assert!(body.starts_with(b"bplist"));
        let ask = ask_request(
            "iPhone",
            "Mac14,6",
            "abc",
            &[("photo.jpg".into(), "image/jpeg".into(), 4)],
        )
        .unwrap();
        let parsed = parse_ask(&ask).unwrap();
        assert_eq!(parsed.sender_name, "iPhone");
        assert_eq!(parsed.files[0].name, "photo.jpg");
        assert_eq!(parsed.files[0].size, Some(4));
        let dict = plist::Value::from_reader(std::io::Cursor::new(&ask))
            .unwrap()
            .into_dictionary()
            .unwrap();
        assert_eq!(
            dict.get("SenderModelName")
                .and_then(plist::Value::as_string),
            Some("Mac14,6")
        );
        let file = dict.get("Files").and_then(plist::Value::as_array).unwrap()[0]
            .as_dictionary()
            .unwrap();
        assert_eq!(
            file.get("FileSize")
                .and_then(plist::Value::as_signed_integer),
            Some(4)
        );
        assert_eq!(
            file.get("ConvertMediaFormats")
                .and_then(plist::Value::as_signed_integer),
            Some(0)
        );
        let refused = {
            let mut root = plist::Dictionary::new();
            root.insert("IsAirDropable".into(), plist::Value::Boolean(false));
            let mut buf = Vec::new();
            plist::Value::Dictionary(root)
                .to_writer_binary(&mut buf)
                .unwrap();
            buf
        };
        assert_eq!(
            super::discover_reply(&refused),
            Some(super::DiscoverReply {
                name: None,
                accepts: Some(false),
            })
        );
        assert!(ask_response("Harry", "Whoosh")
            .unwrap()
            .starts_with(b"bplist"));
        assert_eq!(
            super::receiver_computer_name(&body).as_deref(),
            Some("Harry")
        );
    }
}
