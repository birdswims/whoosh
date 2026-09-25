use crate::error::{Error, Result};

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Accept a single path component and reject traversal, separators, and Windows device names.
///
/// Trailing dots and spaces are removed because Windows silently strips them.
pub fn safe_file_name(raw: &str) -> Result<String> {
    if raw
        .chars()
        .any(|ch| ch.is_control() || ch == '/' || ch == '\\' || ch == ':')
    {
        return Err(Error::UnsafeName);
    }
    let trimmed = raw.trim().trim_end_matches(['.', ' ']);
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err(Error::UnsafeName);
    }
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    if !stem.is_empty() && RESERVED.iter().any(|name| stem.eq_ignore_ascii_case(name)) {
        return Err(Error::UnsafeName);
    }
    if trimmed.len() > 240 {
        return Err(Error::UnsafeName);
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::safe_file_name;

    #[test]
    fn accepts_ordinary_and_unicode_names() {
        assert_eq!(safe_file_name("photo.jpg").unwrap(), "photo.jpg");
        assert_eq!(safe_file_name("café.png").unwrap(), "café.png");
        assert_eq!(safe_file_name("clip.mp4").unwrap(), "clip.mp4");
        assert_eq!(safe_file_name(".hidden").unwrap(), ".hidden");
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        assert_eq!(safe_file_name("notes.txt. ").unwrap(), "notes.txt");
    }

    #[test]
    fn rejects_traversal_and_device_names() {
        for name in [
            "../x",
            "..\\x",
            "/etc/passwd",
            "C:foo",
            "..",
            ".",
            "",
            " ",
            "CON.txt",
            "aux",
            "nul.",
            "a/b",
            "a\\b",
            "bad\0name",
        ] {
            assert!(safe_file_name(name).is_err(), "{name}");
        }
    }
}
