use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::names::normalize_file_name;
use crate::scan::{scan, ScanOptions};

#[derive(Debug, Serialize)]
pub struct BackupCheck {
    pub source_files: usize,
    pub dest_files: usize,
    /// Source files whose name is nowhere in the destination (paths relative to the source).
    pub missing: Vec<String>,
    /// Name found, but not in the folder it should be in: (source path, folder(s) it was found in).
    pub wrong_folder: Vec<(String, String)>,
}

/// Name-and-folder check, no hashing. Every file under `source` must have a same-named file under `dest`
/// (a copy renamed to `name (N).ext` counts). The expected folder is the source folder, or `YYYY-MM`
/// when `sorted` (icloudpd's `YYYY/MM/DD` becomes a month folder). A source outside that layout skips
/// the folder test. Content is not compared, so a different file sharing a name is not caught.
pub fn check_backup(source: &Path, dest: &Path, sorted: bool) -> io::Result<BackupCheck> {
    let opts = ScanOptions { recursive: true, modified_since: None, skip_junk: true, skip_icloudpd: true };
    let source_files = scan(source, &opts)?;
    let dest_files = scan(dest, &opts)?;
    let mut present: HashMap<String, Vec<&str>> = HashMap::with_capacity(dest_files.len());
    for rel in &dest_files {
        let name = file_name(rel);
        if name.ends_with(".sync-partial") {
            continue;
        }
        let dir = dir_of(rel);
        for key in [normalize_file_name(&name).to_lowercase(), name.to_lowercase()] {
            let dirs = present.entry(key).or_default();
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    let (mut missing, mut wrong_folder) = (Vec::new(), Vec::new());
    for rel in &source_files {
        let Some(dirs) = present.get(&file_name(rel).to_lowercase()) else {
            missing.push(rel.clone());
            continue;
        };
        let Some(expected) = expected_dir(rel, sorted) else {
            continue;
        };
        if !dirs.iter().any(|dir| dir.eq_ignore_ascii_case(&expected)) {
            wrong_folder.push((rel.clone(), dirs.join(", ")));
        }
    }
    Ok(BackupCheck { source_files: source_files.len(), dest_files: dest_files.len(), missing, wrong_folder })
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn expected_dir(rel: &str, sorted: bool) -> Option<String> {
    if !sorted {
        return Some(dir_of(rel).to_string());
    }
    let mut parts = rel.split('/');
    let (year, month) = (parts.next()?, parts.next()?);
    let numeric = |text: &str, len| text.len() == len && text.bytes().all(|byte| byte.is_ascii_digit());
    (numeric(year, 4) && numeric(month, 2)).then(|| format!("{year}-{month}"))
}

fn file_name(rel: &str) -> String {
    rel.rsplit('/').next().unwrap_or(rel).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reports_missing_names_and_wrong_folders() {
        let root = std::env::temp_dir().join(format!("verify-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (src, dst) = (root.join("src"), root.join("dst"));
        fs::create_dir_all(src.join("2024/03/15")).unwrap();
        fs::create_dir_all(dst.join("2024-03")).unwrap();
        fs::create_dir_all(dst.join("2024-04")).unwrap();
        for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg", ".DS_Store", "skip.part"] {
            fs::write(src.join("2024/03/15").join(name), "x").unwrap();
        }
        fs::write(dst.join("2024-03/A.JPG"), "x").unwrap(); // case differs
        fs::write(dst.join("2024-03/b (1).jpg"), "x").unwrap(); // renamed on collision
        fs::write(dst.join("2024-04/d.jpg"), "x").unwrap(); // filed in the wrong month
        let check = check_backup(&src, &dst, true).unwrap();
        assert_eq!(check.source_files, 4);
        assert_eq!(check.missing, vec!["2024/03/15/c.jpg".to_string()]);
        assert_eq!(check.wrong_folder, vec![("2024/03/15/d.jpg".to_string(), "2024-04".to_string())]);
        let _ = fs::remove_dir_all(&root);
    }
}
