//! Paths of subfolders inside a category.
//!
//! `RelPath` is the only way to address a folder below a category root: every
//! segment is validated here, so a value of this type can never contain `..`,
//! separators, NUL or other surprises, and joining it onto a category
//! directory cannot leave that directory. Symlinks are refused separately by
//! [`ensure_no_symlinks`], because they are a property of the disk, not of the
//! string.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Deepest allowed folder nesting below a category.
pub const MAX_DEPTH: usize = 8;
/// Limit for a single folder name (common filesystem limit).
pub const MAX_SEGMENT_BYTES: usize = 255;
/// Limit for the whole relative path.
pub const MAX_PATH_BYTES: usize = 1024;

/// Validate one folder name. Folder names are stricter than file names: they
/// may not start with `.` (reserved for service directories) and may not have
/// leading/trailing whitespace, which would make two folders look identical.
pub fn validate_segment(segment: &str) -> Result<&str, &'static str> {
    if segment.is_empty() {
        return Err("Пустое имя папки");
    }
    if segment.len() > MAX_SEGMENT_BYTES {
        return Err("Слишком длинное имя папки");
    }
    if segment.trim() != segment {
        return Err("Имя папки не может начинаться или заканчиваться пробелом");
    }
    if segment.starts_with('.') {
        return Err("Имя папки не может начинаться с точки");
    }
    if segment.contains(['/', '\\']) || segment.chars().any(char::is_control) {
        return Err("Недопустимые символы в имени папки");
    }
    Ok(segment)
}

/// Relative folder path below a category root; empty = the category root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RelPath(Vec<String>);

impl RelPath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    /// Parse `a/b/c` (no leading or trailing slash, no empty segments).
    /// An empty string is the category root.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        if raw.is_empty() {
            return Ok(Self::root());
        }
        if raw.len() > MAX_PATH_BYTES {
            return Err("Слишком длинный путь");
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            segments.push(validate_segment(segment)?.to_string());
        }
        if segments.len() > MAX_DEPTH {
            return Err("Слишком глубокая вложенность папок");
        }
        Ok(Self(segments))
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn segments(&self) -> &[String] {
        &self.0
    }

    /// Append one folder name, re-checking depth and length limits.
    pub fn join(&self, name: &str) -> Result<Self, &'static str> {
        let name = validate_segment(name)?;
        if self.0.len() + 1 > MAX_DEPTH {
            return Err("Слишком глубокая вложенность папок");
        }
        let mut segments = self.0.clone();
        segments.push(name.to_string());
        let joined = Self(segments);
        if joined.to_string().len() > MAX_PATH_BYTES {
            return Err("Слишком длинный путь");
        }
        Ok(joined)
    }

    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            return None;
        }
        Some(Self(self.0[..self.0.len() - 1].to_vec()))
    }

    pub fn name(&self) -> Option<&str> {
        self.0.last().map(String::as_str)
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.iter().collect()
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("/"))
    }
}

/// Error payload of [`ensure_no_symlinks`], so callers can tell a refused
/// symlink from an ordinary permission error.
#[derive(Debug)]
pub struct SymlinkRefused;

impl fmt::Display for SymlinkRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("symbolic links are not followed")
    }
}

impl Error for SymlinkRefused {}

/// Refuse to follow symlinks below `base`: walks every existing component of
/// `rel` (and the optional final `name`) and fails if one is a symlink. A
/// symlink created over SSH inside a zone could otherwise point anywhere on
/// the server. Missing components are fine — they will be created as real
/// directories.
pub fn ensure_no_symlinks(base: &Path, rel: &RelPath, name: Option<&str>) -> io::Result<()> {
    let mut current = base.to_path_buf();
    let components = rel.segments().iter().map(String::as_str).chain(name);
    for component in components {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    SymlinkRefused,
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_paths() {
        let path = RelPath::parse("Ремонт кухни/Чеки").unwrap();
        assert_eq!(path.segments(), ["Ремонт кухни", "Чеки"]);
        assert_eq!(path.to_string(), "Ремонт кухни/Чеки");
        assert_eq!(path.name(), Some("Чеки"));
        assert_eq!(path.parent().unwrap().to_string(), "Ремонт кухни");
        assert!(RelPath::parse("").unwrap().is_root());
    }

    #[test]
    fn rejects_traversal_and_odd_segments() {
        for bad in [
            "..",
            "a/../b",
            ".",
            "a//b",
            "/a",
            "a/",
            ".hidden",
            "a/.staging",
            "a\\b",
            "a\0b",
            "a\nb",
            " leading",
            "trailing ",
        ] {
            assert!(RelPath::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn enforces_depth_and_length() {
        let eight = ["d"; MAX_DEPTH].join("/");
        let nine = ["d"; MAX_DEPTH + 1].join("/");
        assert!(RelPath::parse(&eight).is_ok());
        assert!(RelPath::parse(&nine).is_err());
        assert!(RelPath::parse(&eight).unwrap().join("x").is_err());
        assert!(RelPath::parse(&"я".repeat(127)).is_ok()); // 254 bytes
        assert!(validate_segment(&"я".repeat(128)).is_err()); // 256 bytes > 255
        assert!(validate_segment(&"a".repeat(255)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused_at_any_level() {
        let dir = std::env::temp_dir().join(format!("rfm-paths-{}", rand::random::<u64>()));
        std::fs::create_dir_all(dir.join("real/inner")).unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), dir.join("link")).unwrap();
        std::os::unix::fs::symlink(dir.join("real/inner"), dir.join("real/alias")).unwrap();

        let ok = RelPath::parse("real/inner").unwrap();
        assert!(ensure_no_symlinks(&dir, &ok, None).is_ok());
        assert!(ensure_no_symlinks(&dir, &ok, Some("missing.txt")).is_ok());
        assert!(ensure_no_symlinks(&dir, &RelPath::parse("real/missing/x").unwrap(), None).is_ok());
        assert!(ensure_no_symlinks(&dir, &RelPath::parse("link").unwrap(), None).is_err());
        assert!(ensure_no_symlinks(&dir, &RelPath::parse("link/x").unwrap(), None).is_err());
        assert!(ensure_no_symlinks(&dir, &RelPath::parse("real").unwrap(), Some("alias")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
