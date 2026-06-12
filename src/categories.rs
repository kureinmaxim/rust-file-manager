/// File categories shown in the UI, with the extensions routed into each one.
pub const FILE_CATEGORIES: &[(&str, &[&str])] = &[
    ("Фото", &["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg"]),
    ("Документы", &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "odt"]),
    ("Программы", &["exe", "msi", "apk", "dmg", "deb", "zip", "tar", "gz", "rar", "7z", "c", "h", "py", "rs"]),
    ("Видео", &["mp4", "avi", "mkv", "mov", "wmv", "webm", "raw", "mpeg", "m4v"]),
    ("Другие", &[]),
];

pub fn category_for_extension(extension: &str) -> &'static str {
    let extension = extension.to_lowercase();
    FILE_CATEGORIES
        .iter()
        .find(|(_, exts)| exts.contains(&extension.as_str()))
        .map(|(category, _)| *category)
        .unwrap_or("Другие")
}

pub fn is_valid_category(category: &str) -> bool {
    FILE_CATEGORIES.iter().any(|(c, _)| *c == category)
}

/// Reduce an untrusted file name to a safe final path component.
///
/// Takes only the last path component (clients may send full paths) and
/// rejects names that are empty, dot-only, or contain separators / NUL —
/// this is what prevents path traversal on every route that accepts a name.
pub fn sanitize_file_name(raw: &str) -> Option<String> {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('\0') {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_known_extensions() {
        assert_eq!(category_for_extension("JPG"), "Фото");
        assert_eq!(category_for_extension("pdf"), "Документы");
        assert_eq!(category_for_extension("rs"), "Программы");
        assert_eq!(category_for_extension("mp4"), "Видео");
        assert_eq!(category_for_extension("xyz"), "Другие");
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert_eq!(sanitize_file_name(".."), None);
        assert_eq!(sanitize_file_name("."), None);
        assert_eq!(sanitize_file_name(""), None);
        assert_eq!(sanitize_file_name("   "), None);
        assert_eq!(sanitize_file_name("a\0b"), None);
        // A path is reduced to its final component, not rejected outright.
        assert_eq!(sanitize_file_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(sanitize_file_name("C:\\Users\\x\\doc.pdf").as_deref(), Some("doc.pdf"));
        assert_eq!(sanitize_file_name("отчёт 2025.pdf").as_deref(), Some("отчёт 2025.pdf"));
    }

    #[test]
    fn category_validation() {
        assert!(is_valid_category("Фото"));
        assert!(!is_valid_category("../секрет"));
    }
}
