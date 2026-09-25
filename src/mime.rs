/// Coarse kind used to sort an inbox and to label Quick Share / AirDrop metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Photo,
    Video,
    Audio,
    Other,
}

impl MediaKind {
    pub fn subdir(self) -> Option<&'static str> {
        match self {
            Self::Photo => Some("Photos"),
            Self::Video => Some("Videos"),
            Self::Audio => Some("Audio"),
            Self::Other => None,
        }
    }

    /// Quick Share `FileMetadata.Type` value.
    pub fn quickshare_type(self) -> i32 {
        match self {
            Self::Photo => 1,
            Self::Video => 2,
            Self::Audio => 4,
            Self::Other => 5,
        }
    }
}

/// Identify a photo, video, or other file from a short header and the file name.
pub fn sniff(header: &[u8], file_name: &str) -> (MediaKind, &'static str) {
    if let Some(found) = sniff_magic(header) {
        return found;
    }
    sniff_extension(file_name).unwrap_or((MediaKind::Other, "application/octet-stream"))
}

fn sniff_magic(header: &[u8]) -> Option<(MediaKind, &'static str)> {
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some((MediaKind::Photo, "image/jpeg"));
    }
    if header.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some((MediaKind::Photo, "image/png"));
    }
    if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        return Some((MediaKind::Photo, "image/gif"));
    }
    if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        return Some((MediaKind::Photo, "image/webp"));
    }
    if header.len() >= 12 && &header[0..4] == b"RIFF" && &header[8..12] == b"AVI " {
        return Some((MediaKind::Video, "video/x-msvideo"));
    }
    if header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some((MediaKind::Video, "video/webm"));
    }
    if header.starts_with(b"ID3") || header.starts_with(&[0xFF, 0xFB]) {
        return Some((MediaKind::Audio, "audio/mpeg"));
    }
    if header.starts_with(b"fLaC") {
        return Some((MediaKind::Audio, "audio/flac"));
    }
    if header.starts_with(b"OggS") {
        return Some((MediaKind::Audio, "audio/ogg"));
    }
    if header.len() >= 12 && &header[4..8] == b"ftyp" {
        let brand = &header[8..12];
        return Some(match brand {
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => {
                (MediaKind::Photo, "image/heic")
            }
            b"avif" | b"avis" => (MediaKind::Photo, "image/avif"),
            b"qt  " => (MediaKind::Video, "video/quicktime"),
            _ => (MediaKind::Video, "video/mp4"),
        });
    }
    None
}

fn sniff_extension(file_name: &str) -> Option<(MediaKind, &'static str)> {
    let ext = file_name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => (MediaKind::Photo, "image/jpeg"),
        "png" => (MediaKind::Photo, "image/png"),
        "gif" => (MediaKind::Photo, "image/gif"),
        "webp" => (MediaKind::Photo, "image/webp"),
        "heic" | "heif" => (MediaKind::Photo, "image/heic"),
        "mp4" | "m4v" => (MediaKind::Video, "video/mp4"),
        "mov" => (MediaKind::Video, "video/quicktime"),
        "webm" => (MediaKind::Video, "video/webm"),
        "mkv" => (MediaKind::Video, "video/x-matroska"),
        "avi" => (MediaKind::Video, "video/x-msvideo"),
        "mp3" => (MediaKind::Audio, "audio/mpeg"),
        "m4a" => (MediaKind::Audio, "audio/mp4"),
        "flac" => (MediaKind::Audio, "audio/flac"),
        "wav" => (MediaKind::Audio, "audio/wav"),
        _ => return None,
    })
}

pub fn kind_of_mime(mime: &str) -> MediaKind {
    if mime.starts_with("image/") {
        MediaKind::Photo
    } else if mime.starts_with("video/") {
        MediaKind::Video
    } else if mime.starts_with("audio/") {
        MediaKind::Audio
    } else {
        MediaKind::Other
    }
}

/// Apple Uniform Type Identifier used in an AirDrop Ask record.
pub fn uniform_type(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "public.jpeg",
        "image/png" => "public.png",
        "image/gif" => "com.compuserve.gif",
        "image/heic" => "public.heic",
        "image/webp" => "org.webmproject.webp",
        "video/mp4" => "public.mpeg-4",
        "video/quicktime" => "com.apple.quicktime-movie",
        "video/webm" => "org.webmproject.webm",
        "audio/mpeg" => "public.mp3",
        "audio/mp4" => "public.mpeg-4-audio",
        _ => "public.data",
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{sniff, uniform_type, MediaKind};

    #[test]
    fn sniffs_photos_and_videos() {
        assert_eq!(
            sniff(&[0xFF, 0xD8, 0xFF, 0xE0], "x.bin"),
            (MediaKind::Photo, "image/jpeg")
        );
        assert_eq!(
            sniff(b"\x89PNG\r\n\x1a\n", "x.bin"),
            (MediaKind::Photo, "image/png")
        );
        let mut mp4 = vec![0, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypisom");
        assert_eq!(sniff(&mp4, "x.bin"), (MediaKind::Video, "video/mp4"));
        let mut mov = vec![0, 0, 0, 0x14];
        mov.extend_from_slice(b"ftypqt  ");
        assert_eq!(sniff(&mov, "x.bin"), (MediaKind::Video, "video/quicktime"));
        assert_eq!(sniff(&[], "clip.webm"), (MediaKind::Video, "video/webm"));
        assert_eq!(
            sniff(&[], "note.txt"),
            (MediaKind::Other, "application/octet-stream")
        );
    }

    #[test]
    fn maps_uti() {
        assert_eq!(uniform_type("image/jpeg"), "public.jpeg");
        assert_eq!(uniform_type("video/mp4"), "public.mpeg-4");
        assert_eq!(uniform_type("application/zip"), "public.data");
    }
}
