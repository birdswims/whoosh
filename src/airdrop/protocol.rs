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
            files.push(AskFile {
                name: name.to_string(),
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
    sender_id: &str,
    files: &[(String, String)],
) -> Result<Vec<u8>> {
    let mut root = Dictionary::new();
    root.insert(
        "SenderComputerName".into(),
        Value::String(sender_name.into()),
    );
    root.insert("BundleID".into(), Value::String("com.apple.finder".into()));
    root.insert("SenderModelName".into(), Value::String("Crossdrop".into()));
    root.insert("SenderID".into(), Value::String(sender_id.into()));
    root.insert("ConvertMediaFormats".into(), Value::Boolean(false));
    let entries = files
        .iter()
        .map(|(name, mime)| {
            let mut file = Dictionary::new();
            file.insert("FileName".into(), Value::String(name.clone()));
            file.insert(
                "FileType".into(),
                Value::String(mime::uniform_type(mime).to_string()),
            );
            file.insert("FileBomPath".into(), Value::String(format!("./{name}")));
            file.insert("FileIsDirectory".into(), Value::Boolean(false));
            file.insert("ConvertMediaFormats".into(), Value::Boolean(false));
            Value::Dictionary(file)
        })
        .collect();
    root.insert("Files".into(), Value::Array(entries));
    write_plist(root)
}

pub fn discover_request() -> Result<Vec<u8>> {
    write_plist(Dictionary::new())
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
        let body = discover_response("Harry", "Crossdrop").unwrap();
        assert!(body.starts_with(b"bplist"));
        let ask = ask_request(
            "iPhone",
            "abc",
            &[("photo.jpg".into(), "image/jpeg".into())],
        )
        .unwrap();
        let parsed = parse_ask(&ask).unwrap();
        assert_eq!(parsed.sender_name, "iPhone");
        assert_eq!(parsed.files[0].name, "photo.jpg");
        assert!(ask_response("Harry", "Crossdrop")
            .unwrap()
            .starts_with(b"bplist"));
    }
}
