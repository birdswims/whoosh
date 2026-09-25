use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::mime::MediaKind;
use crate::sanitize::safe_file_name;

/// Choose a destination that does not collide with a file already in `dir`.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let ext = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let name = match ext {
            Some(ext) => format!("{stem} ({index}).{ext}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}", std::process::id()))
}

pub fn partial_path(final_path: &Path) -> PathBuf {
    let name = final_path.file_name().unwrap_or_default();
    let mut partial = std::ffi::OsString::from(".");
    partial.push(name);
    partial.push(".partial");
    final_path.with_file_name(partial)
}

/// Place `file_name` inside `dir`, optionally under a Photos/Videos/Audio folder.
pub fn destination(
    dir: &Path,
    file_name: &str,
    kind: MediaKind,
    sort_media: bool,
) -> Result<PathBuf> {
    let safe = safe_file_name(file_name)?;
    let folder = if sort_media { kind.subdir() } else { None };
    let dir = match folder {
        Some(sub) => {
            let nested = dir.join(sub);
            std::fs::create_dir_all(&nested)?;
            nested
        }
        None => {
            std::fs::create_dir_all(dir)?;
            dir.to_path_buf()
        }
    };
    Ok(unique_path(&dir, &safe))
}

/// Archive member names may contain `./` and a single relative folder. Parent directories are rejected.
pub fn archive_member_name(raw: &str) -> Result<String> {
    let mut normals = 0usize;
    for component in Path::new(raw).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                if part.to_str().is_none() {
                    return Err(Error::UnsafeName);
                }
                normals += 1;
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::UnsafeName);
            }
        }
    }
    if normals == 0 {
        return Err(Error::UnsafeName);
    }
    let base = Path::new(raw)
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(Error::UnsafeName)?;
    safe_file_name(base)
}

#[cfg(test)]
mod tests {
    use super::{archive_member_name, destination, unique_path};
    use crate::mime::MediaKind;
    use std::fs;

    #[test]
    fn unique_path_adds_a_suffix() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"x").unwrap();
        let path = unique_path(dir.path(), "a.txt");
        assert_eq!(path.file_name().unwrap(), "a (1).txt");
    }

    #[test]
    fn archive_names_reject_parents_and_keep_the_leaf() {
        assert_eq!(archive_member_name("./photo.jpg").unwrap(), "photo.jpg");
        assert_eq!(archive_member_name("album/cover.png").unwrap(), "cover.png");
        assert!(archive_member_name("./../../etc/passwd").is_err());
        assert!(archive_member_name("/etc/passwd").is_err());
    }

    #[test]
    fn sort_media_uses_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let path = destination(dir.path(), "pic.jpg", MediaKind::Photo, true).unwrap();
        assert!(path.ends_with("Photos/pic.jpg"));
        assert!(dir.path().join("Photos").is_dir());
    }
}
