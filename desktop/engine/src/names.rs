/// Strip a trailing ` (N)` or `(N)` before the extension.
/// `(1).jpg` stays unchanged, because stripping it would leave an empty name.
pub fn normalize_file_name(file_name: &str) -> String {
    if file_name.is_empty() {
        return String::new();
    }
    let (base, ext) = split_extension(file_name);
    let stripped = strip_trailing_copy_number(base);
    if stripped.is_empty() {
        return file_name.to_string();
    }
    match ext {
        Some(ext) => format!("{stripped}.{ext}"),
        None => stripped.to_string(),
    }
}

pub fn are_duplicate_names(left: &str, right: &str) -> bool {
    !left.is_empty() && normalize_file_name(left) == normalize_file_name(right)
}

fn split_extension(file_name: &str) -> (&str, Option<&str>) {
    match file_name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() && !ext.is_empty() => (base, Some(ext)),
        _ => (file_name, None),
    }
}

fn strip_trailing_copy_number(base: &str) -> &str {
    let bytes = base.as_bytes();
    if !bytes.last().is_some_and(|c| *c == b')') {
        return base;
    }
    let open = base.rfind('(');
    let Some(open) = open else {
        return base;
    };
    let inside = &base[open + 1..base.len() - 1];
    if inside.is_empty() || !inside.bytes().all(|c| c.is_ascii_digit()) {
        return base;
    }
    let trimmed = base[..open].trim_end();
    if trimmed.is_empty() {
        base
    } else {
        trimmed
    }
}

/// Prefer the name without ` (N)`, then the smallest N, then alphabetical order.
pub fn choose_keep(names: &[String]) -> String {
    let mut ranked = names.to_vec();
    ranked.sort_by(|a, b| keep_rank(a).cmp(&keep_rank(b)).then(a.cmp(b)));
    ranked.remove(0)
}

fn keep_rank(name: &str) -> (u32, u32) {
    let (base, _) = split_extension(name);
    let bytes = base.as_bytes();
    if bytes.last() != Some(&b')') {
        return (0, 0);
    }
    let Some(open) = base.rfind('(') else {
        return (0, 0);
    };
    let inside = &base[open + 1..base.len() - 1];
    if inside.is_empty() || !inside.bytes().all(|c| c.is_ascii_digit()) {
        return (0, 0);
    }
    if base[..open].trim_end().is_empty() {
        return (0, 0);
    }
    let n: u32 = inside.parse().unwrap_or(u32::MAX);
    (1, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_duplicate_cleaner_name_cases() {
        assert_eq!(normalize_file_name("IMG_0381(1).jpg"), "IMG_0381.jpg");
        assert_eq!(normalize_file_name("IMG_0381.jpg"), "IMG_0381.jpg");
        assert!(are_duplicate_names("IMG_0381(1).jpg", "IMG_0381.jpg"));
        assert_eq!(normalize_file_name("IMG_0381 (2).jpg"), "IMG_0381.jpg");
        assert!(are_duplicate_names("IMG_0381 (2).jpg", "IMG_0381.jpg"));
        assert_eq!(normalize_file_name("VID(1).mov"), "VID.mov");
        assert!(are_duplicate_names("VID(1).mov", "VID.mov"));
        assert!(!are_duplicate_names("IMG_0381.jpg", "IMG_0382.jpg"));
        assert!(!are_duplicate_names("IMG_0381(1).jpg", "IMG_0382(1).jpg"));
        assert!(!are_duplicate_names("IMG_0381.jpg", "IMG_0381.png"));
        assert_eq!(normalize_file_name("file(1)"), "file");
        assert_eq!(normalize_file_name("file"), "file");
        assert_eq!(normalize_file_name("file (10).txt"), "file.txt");
        assert_eq!(normalize_file_name(""), "");
        assert_eq!(normalize_file_name("(1).jpg"), "(1).jpg");
    }

    #[test]
    fn keep_prefers_plain_name_then_smallest_n() {
        let names = vec![
            "IMG_0381(2).jpg".to_string(),
            "IMG_0381 (1).jpg".to_string(),
            "IMG_0381.jpg".to_string(),
        ];
        assert_eq!(choose_keep(&names), "IMG_0381.jpg");
        let copies = vec!["VID (2).mov".to_string(), "VID(1).mov".to_string()];
        assert_eq!(choose_keep(&copies), "VID(1).mov");
    }
}
