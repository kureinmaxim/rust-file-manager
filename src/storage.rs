//! Storage layer shared by the web UI, the Mini App API and the internal API.
//!
//! Layout on disk (unchanged from 1.x):
//! `UPLOAD_DIR/shared/<category>/…` and `UPLOAD_DIR/home/<user>/<category>/…`,
//! plus service directories at the upload root that never appear in zones
//! (`.staging/` for chunked uploads). Since 1.5 categories may contain
//! subfolders; every folder path goes through [`RelPath`].

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use actix_web::http::StatusCode;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::categories::{
    category_rel_dir, sanitize_file_name, BACKUP_FOLDERS, BACKUP_PARENT, FILE_CATEGORIES,
};
use crate::config::AppConfig;
use crate::paths::{ensure_no_symlinks, RelPath, SymlinkRefused, MAX_DEPTH};

pub const SHARED_DIR: &str = "shared";
pub const HOME_DIR: &str = "home";
/// Chunked uploads in progress; same filesystem as the zones, so finishing an
/// upload is an atomic link/rename.
pub const STAGING_DIR: &str = ".staging";

/// Upper bound for recursive walks (search, recent files, folder statistics),
/// so one request cannot keep a small VPS busy indefinitely.
pub const WALK_LIMIT: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    My,
    Shared,
}

impl Zone {
    pub fn parse(scope: &str) -> Option<Self> {
        match scope {
            "my" => Some(Self::My),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::My => "my",
            Self::Shared => "shared",
        }
    }
}

/// Root directory of a zone: `shared/` is visible to everyone, `my` maps to
/// the per-user `home/<username>/` directory nobody else can reach — the
/// username always comes from the authenticated session or token, never from
/// the request.
pub fn zone_root(config: &AppConfig, zone: Zone, username: &str) -> PathBuf {
    match zone {
        Zone::Shared => config.upload_dir.join(SHARED_DIR),
        Zone::My => config.upload_dir.join(HOME_DIR).join(username),
    }
}

#[derive(Debug)]
pub enum StorageError {
    Zone,
    Category,
    Name,
    Path(&'static str),
    Symlink,
    NotFound,
    Exists,
    NotEmpty,
    Io(io::Error),
}

impl StorageError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Exists | Self::NotEmpty => StatusCode::CONFLICT,
            Self::Symlink => StatusCode::FORBIDDEN,
            Self::Io(e) if e.kind() == io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
            Self::Io(e) if is_out_of_space(e) => StatusCode::INSUFFICIENT_STORAGE,
            Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Exists => "exists",
            Self::NotEmpty => "not_empty",
            Self::Io(e) if is_out_of_space(e) => "no_space",
            _ => "bad_request",
        }
    }

    /// User-facing text; never includes server paths.
    pub fn message(&self) -> String {
        match self {
            Self::Zone => "Недопустимая зона".into(),
            Self::Category => "Недопустимая категория".into(),
            Self::Name => "Недопустимое имя файла".into(),
            Self::Path(reason) => (*reason).into(),
            Self::Symlink => "Символьные ссылки не поддерживаются".into(),
            Self::NotFound => "Файл или папка не найдены".into(),
            Self::Exists => "Файл или папка с таким именем уже существует".into(),
            Self::NotEmpty => "Папка не пуста: сначала удалите или перенесите файлы".into(),
            Self::Io(e) if is_out_of_space(e) => "Недостаточно места на сервере".into(),
            Self::Io(e) if e.kind() == io::ErrorKind::NotFound => {
                "Файл или папка не найдены".into()
            }
            Self::Io(_) => "Ошибка файловой системы".into(),
        }
    }
}

impl From<io::Error> for StorageError {
    fn from(e: io::Error) -> Self {
        if e.get_ref()
            .is_some_and(|inner| inner.is::<SymlinkRefused>())
        {
            return Self::Symlink;
        }
        Self::Io(e)
    }
}

fn is_out_of_space(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::StorageFull
}

/// Directory of a category inside a zone (not created).
pub fn category_dir(
    config: &AppConfig,
    zone: Zone,
    username: &str,
    category: &str,
) -> Result<PathBuf, StorageError> {
    let rel = category_rel_dir(category).ok_or(StorageError::Category)?;
    Ok(zone_root(config, zone, username).join(rel))
}

/// Directory of a folder inside a category; symlinks on the way are refused.
pub fn folder_dir(
    config: &AppConfig,
    zone: Zone,
    username: &str,
    category: &str,
    rel: &RelPath,
) -> Result<PathBuf, StorageError> {
    let base = category_dir(config, zone, username, category)?;
    ensure_no_symlinks(&base, rel, None)?;
    Ok(base.join(rel.to_path_buf()))
}

/// Exact file name check for the API: unlike the legacy routes, a name that
/// `sanitize_file_name` would have to shorten (a path) is rejected outright.
pub fn exact_file_name(name: &str) -> Result<&str, StorageError> {
    match sanitize_file_name(name) {
        Some(clean) if clean == name => Ok(name),
        _ => Err(StorageError::Name),
    }
}

/// Path of a file inside a folder; neither the folders nor the file may be symlinks.
pub fn file_path(
    config: &AppConfig,
    zone: Zone,
    username: &str,
    category: &str,
    rel: &RelPath,
    name: &str,
) -> Result<PathBuf, StorageError> {
    let name = exact_file_name(name)?;
    let base = category_dir(config, zone, username, category)?;
    ensure_no_symlinks(&base, rel, Some(name))?;
    Ok(base.join(rel.to_path_buf()).join(name))
}

fn exists_error(e: io::Error) -> StorageError {
    if e.kind() == io::ErrorKind::AlreadyExists {
        StorageError::Exists
    } else {
        StorageError::from(e)
    }
}

/// Create folder `name` inside `parent` (the category directory itself is
/// created when missing). Fails instead of reusing an existing name.
/// Shared by the Mini App API and the web UI.
pub fn create_folder(
    config: &AppConfig,
    zone: Zone,
    username: &str,
    category: &str,
    parent: &RelPath,
    name: &str,
) -> Result<(RelPath, PathBuf), StorageError> {
    let folder = parent.join(name).map_err(StorageError::Path)?;
    let base = category_dir(config, zone, username, category)?;
    fs::create_dir_all(&base)?;
    let parent_dir = folder_dir(config, zone, username, category, parent)?;
    if !parent_dir.is_dir() {
        return Err(StorageError::NotFound);
    }
    let dir = folder_dir(config, zone, username, category, &folder)?;
    fs::create_dir(&dir).map_err(exists_error)?;
    Ok((folder, dir))
}

/// Rename the last segment of a folder; refuses to replace an existing entry.
pub fn rename_folder(
    config: &AppConfig,
    zone: Zone,
    username: &str,
    category: &str,
    rel: &RelPath,
    new_name: &str,
) -> Result<(RelPath, PathBuf), StorageError> {
    let parent = rel.parent().ok_or(StorageError::Path("Выберите папку"))?;
    let new_rel = parent.join(new_name).map_err(StorageError::Path)?;
    let old_path = folder_dir(config, zone, username, category, rel)?;
    let new_path = folder_dir(config, zone, username, category, &new_rel)?;
    if !old_path.is_dir() {
        return Err(StorageError::NotFound);
    }
    if fs::symlink_metadata(&new_path).is_ok() {
        return Err(StorageError::Exists);
    }
    fs::rename(&old_path, &new_path)?;
    Ok((new_rel, new_path))
}

/// Delete a folder only when it is empty: until a trash exists, a folder
/// operation never removes files.
pub fn remove_empty_folder(path: &Path) -> Result<(), StorageError> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => Err(StorageError::NotEmpty),
        Err(e) => Err(e.into()),
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let i = ((bytes as f64).log(1024.0).floor() as usize).min(units.len() - 1);
    format!("{:.2} {}", bytes as f64 / 1024f64.powi(i as i32), units[i])
}

pub fn folder_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let p = entry.path();
            if p.is_dir() {
                folder_size(&p)
            } else {
                fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Total and available bytes of the filesystem holding `path`.
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // field widths differ between Linux and macOS
pub fn disk_usage(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs only writes into the zeroed struct we pass and reads the
    // NUL-terminated path; both outlive the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    Some((stat.f_blocks as u64 * block, stat.f_bavail as u64 * block))
}

#[cfg(not(unix))]
pub fn disk_usage(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// Category ids paired with their UI titles: regular categories first, then
/// the backup folders shown as «Бэкапы — <folder>».
pub fn category_listing() -> Vec<(String, String)> {
    FILE_CATEGORIES
        .iter()
        .map(|(c, _)| (c.to_string(), c.to_string()))
        .chain(
            BACKUP_FOLDERS
                .iter()
                .map(|f| (f.to_string(), format!("💾 {BACKUP_PARENT} — {f}"))),
        )
        .collect()
}

/// Encode each URL segment independently: filenames may contain #, ?, % or +.
pub fn encode_url_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("writing to a String");
        }
    }
    encoded
}

fn unix_mtime(meta: &fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

/// One directory entry as seen by the listing APIs.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u64>,
}

/// Read a directory without following symlinks: symlinks, non-UTF-8 names and
/// special files are skipped. A missing directory is an empty listing.
pub fn read_dir_entries(dir: &Path) -> io::Result<Vec<DirEntry>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() || !(file_type.is_dir() || file_type.is_file()) {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        result.push(DirEntry {
            name,
            is_dir: file_type.is_dir(),
            size: if file_type.is_file() { meta.len() } else { 0 },
            mtime: unix_mtime(&meta),
        });
    }
    Ok(result)
}

/// Files, bytes and subfolders below a folder (recursive, bounded, no symlinks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DirStats {
    pub files: u64,
    pub bytes: u64,
    pub folders: u64,
}

pub fn dir_stats(dir: &Path) -> DirStats {
    let mut stats = DirStats::default();
    let mut budget = WALK_LIMIT;
    walk(dir, 0, &mut budget, &mut |_, entry| {
        if entry.is_dir {
            stats.folders += 1;
        } else {
            stats.files += 1;
            stats.bytes += entry.size;
        }
    });
    stats
}

/// Depth-first walk below `dir` (not following symlinks). The callback gets
/// the folder path relative to `dir` and each entry inside it.
pub fn walk(
    dir: &Path,
    depth: usize,
    budget: &mut usize,
    visit: &mut dyn FnMut(&[String], &DirEntry),
) {
    fn inner(
        dir: &Path,
        rel: &mut Vec<String>,
        depth: usize,
        budget: &mut usize,
        visit: &mut dyn FnMut(&[String], &DirEntry),
    ) {
        let Ok(entries) = read_dir_entries(dir) else {
            return;
        };
        for entry in entries {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            visit(rel, &entry);
            if entry.is_dir && depth < MAX_DEPTH {
                rel.push(entry.name.clone());
                inner(&dir.join(&entry.name), rel, depth + 1, budget, visit);
                rel.pop();
            }
        }
    }
    inner(dir, &mut Vec::new(), depth, budget, visit);
}

/// Opaque-looking but not secret id of a file or folder: base64url of
/// `zone \0 category \0 path \0 name`. NUL cannot occur in any component, so
/// decoding is unambiguous; every component is re-validated on decode, and
/// access is still checked against the authenticated user.
pub fn encode_id(zone: Zone, category: &str, rel: &RelPath, name: Option<&str>) -> String {
    let raw = format!(
        "{}\0{}\0{}\0{}",
        zone.as_str(),
        category,
        rel,
        name.unwrap_or("")
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRef {
    pub zone: Zone,
    pub category: String,
    pub path: RelPath,
    /// `None` for a folder (then `path` is the folder itself).
    pub name: Option<String>,
}

pub fn decode_id(id: &str) -> Result<ItemRef, StorageError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(id)
        .map_err(|_| StorageError::Path("Недопустимый идентификатор"))?;
    let text =
        String::from_utf8(bytes).map_err(|_| StorageError::Path("Недопустимый идентификатор"))?;
    let parts: Vec<&str> = text.split('\0').collect();
    let [zone, category, path, name] = parts[..] else {
        return Err(StorageError::Path("Недопустимый идентификатор"));
    };
    let zone = Zone::parse(zone).ok_or(StorageError::Zone)?;
    category_rel_dir(category).ok_or(StorageError::Category)?;
    let path = RelPath::parse(path).map_err(StorageError::Path)?;
    let name = if name.is_empty() {
        if path.is_root() {
            return Err(StorageError::Path("Недопустимый идентификатор"));
        }
        None
    } else {
        Some(exact_file_name(name)?.to_string())
    };
    Ok(ItemRef {
        zone,
        category: category.to_string(),
        path,
        name,
    })
}

/// An interrupted write must not leave a truncated file behind: the file is
/// created with `create_new` (atomic name reservation, also across concurrent
/// uploads) and removed on drop unless `finish` was called.
pub struct PendingUpload {
    pub path: PathBuf,
    pub file: Option<fs::File>,
    complete: bool,
}

/// Candidate names for a file that may collide: `name`, `stem(1).ext`, `stem(2).ext`, …
pub fn name_candidates(name: &str) -> impl Iterator<Item = String> + '_ {
    let original = Path::new(name);
    let stem = original
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let extension = original.extension().and_then(|s| s.to_str()).unwrap_or("");
    (0u64..).map(move |counter| {
        if counter == 0 {
            name.to_string()
        } else if extension.is_empty() {
            format!("{stem}({counter})")
        } else {
            format!("{stem}({counter}).{extension}")
        }
    })
}

impl PendingUpload {
    pub fn create(dir: &Path, name: &str) -> io::Result<Self> {
        for candidate in name_candidates(name) {
            let path = dir.join(candidate);
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        complete: false,
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        unreachable!("exhausted filename suffixes")
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        self.file.take();
        self.complete = true;
        Ok(())
    }
}

impl Drop for PendingUpload {
    fn drop(&mut self) {
        self.file.take();
        if !self.complete {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Move a finished file into `dir` without ever overwriting: a hard link to
/// the first free candidate name is atomic and fails if the name is taken,
/// then the source is removed. Returns the final path.
pub fn publish_file(source: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    for candidate in name_candidates(name) {
        let target = dir.join(candidate);
        match fs::hard_link(source, &target) {
            Ok(()) => {
                fs::remove_file(source)?;
                return Ok(target);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("exhausted filename suffixes")
}

#[cfg(test)]
pub(crate) fn test_directory(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "rfm-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_segments_preserve_special_and_unicode_names() {
        assert_eq!(encode_url_segment("a #?%+.txt"), "a%20%23%3F%25%2B.txt");
        assert_eq!(
            encode_url_segment("тест.txt"),
            "%D1%82%D0%B5%D1%81%D1%82.txt"
        );
        assert_eq!(encode_url_segment("../a"), "..%2Fa");
    }

    #[test]
    fn simultaneous_uploads_reserve_distinct_names() {
        let dir = test_directory("upload");
        let mut first = PendingUpload::create(&dir, "report.txt").unwrap();
        let mut second = PendingUpload::create(&dir, "report.txt").unwrap();
        assert_ne!(first.path, second.path);
        first.file.as_mut().unwrap().write_all(b"first").unwrap();
        second.file.as_mut().unwrap().write_all(b"second").unwrap();
        first.finish().unwrap();
        second.finish().unwrap();
        assert_eq!(fs::read(&first.path).unwrap(), b"first");
        assert_eq!(fs::read(&second.path).unwrap(), b"second");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn incomplete_uploads_are_removed_but_completed_files_survive() {
        let dir = test_directory("upload");
        let partial = PendingUpload::create(&dir, "incomplete.txt").unwrap();
        let path = partial.path.clone();
        drop(partial);
        assert!(!path.exists());
        let mut complete = PendingUpload::create(&dir, "complete.txt").unwrap();
        complete.finish().unwrap();
        let path = complete.path.clone();
        drop(complete);
        assert!(path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn publish_never_overwrites() {
        let dir = test_directory("publish");
        fs::write(dir.join("a.txt"), b"old").unwrap();
        let staged = dir.join("staged.part");
        fs::write(&staged, b"new").unwrap();
        let target = publish_file(&staged, &dir, "a.txt").unwrap();
        assert_eq!(target.file_name().unwrap(), "a(1).txt");
        assert_eq!(fs::read(dir.join("a.txt")).unwrap(), b"old");
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ids_roundtrip_and_reject_garbage() {
        let rel = RelPath::parse("Ремонт кухни/Чеки").unwrap();
        let id = encode_id(Zone::My, "Документы", &rel, Some("чек #1.pdf"));
        let item = decode_id(&id).unwrap();
        assert_eq!(item.zone, Zone::My);
        assert_eq!(item.category, "Документы");
        assert_eq!(item.path, rel);
        assert_eq!(item.name.as_deref(), Some("чек #1.pdf"));

        let folder = decode_id(&encode_id(Zone::Shared, "Фото", &rel, None)).unwrap();
        assert_eq!(folder.name, None);

        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        for bad in [
            "!!!".to_string(),
            b64("my\0Документы\0..\0x"),
            b64("my\0Бэкапы\0\0x"),
            b64("other\0Фото\0\0x"),
            b64("my\0Фото\0\0../x"),
            b64("my\0Фото\0\0"), // folder id must name a folder
            b64("my\0Фото\0a\0b\0c"),
        ] {
            assert!(decode_id(&bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn listing_skips_symlinks_and_counts_recursively() {
        let dir = test_directory("list");
        fs::create_dir_all(dir.join("A/B")).unwrap();
        fs::write(dir.join("top.txt"), b"12345").unwrap();
        fs::write(dir.join("A/one.txt"), b"1").unwrap();
        fs::write(dir.join("A/B/two.txt"), b"22").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", dir.join("escape")).unwrap();

        let mut names: Vec<_> = read_dir_entries(&dir)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [("A".to_string(), true), ("top.txt".to_string(), false)]
        );
        assert_eq!(
            dir_stats(&dir),
            DirStats {
                files: 3,
                bytes: 8,
                folders: 2
            }
        );
        assert!(read_dir_entries(&dir.join("missing")).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
