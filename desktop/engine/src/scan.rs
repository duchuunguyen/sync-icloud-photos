use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, Local, NaiveDate};

#[derive(Clone, Debug)]
pub struct ScanOptions {
    pub recursive: bool,
    pub modified_since: Option<NaiveDate>,
    pub skip_junk: bool,
    /// Skip icloudpd's partial downloads and its prune bookkeeping.
    pub skip_icloudpd: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            modified_since: None,
            skip_junk: true,
            skip_icloudpd: false,
        }
    }
}

/// Snapshot of file paths relative to `root`, using `/` separators, sorted.
/// Directories are not included. A file created after this returns is not in the list.
pub fn scan(root: &Path, opts: &ScanOptions) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    walk(root, root, opts, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(root: &Path, dir: &Path, opts: &ScanOptions, out: &mut Vec<String>) -> io::Result<()> {
    let entries = fs::read_dir(dir).map_err(|err| {
        if dir == root && err.kind() == io::ErrorKind::PermissionDenied {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "macOS blocked access to this folder. Enable Full Disk Access for Sync iCloud Photos in System Settings → Privacy & Security.",
            )
        } else {
            err
        }
    })?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if meta.is_dir() {
            if opts.skip_icloudpd && name == ".icloudpd-pruned" {
                continue;
            }
            if opts.recursive {
                walk(root, &path, opts, out)?;
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if opts.skip_junk && is_junk(&name) {
            continue;
        }
        if opts.skip_icloudpd && is_icloudpd_runtime(&name) {
            continue;
        }
        if let Some(day) = opts.modified_since {
            let modified: DateTime<Local> = meta.modified()?.into();
            if modified.date_naive() < day {
                continue;
            }
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        out.push(rel_to_string(rel));
    }
    Ok(())
}

pub fn is_junk(name: &str) -> bool {
    name == ".DS_Store" || name.starts_with("._")
}

pub fn is_icloudpd_runtime(name: &str) -> bool {
    name.ends_with(".part") || name == ".icloudpd-prune.json" || name == ".icloudpd-prune.lock" || name == ".icloudpd-pruned"
}

fn rel_to_string(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::{Duration, SystemTime};

    fn touch(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(path).unwrap();
        write!(file, "{contents}").unwrap();
    }

    #[test]
    fn top_level_only_skips_nested_files() {
        let root = std::env::temp_dir().join(format!("scan-flat-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(&root.join("a.jpg"), "a");
        touch(&root.join("sub").join("b.jpg"), "b");
        let found = scan(&root, &ScanOptions::default()).unwrap();
        assert_eq!(found, vec!["a.jpg".to_string()]);
        let nested = scan(
            &root,
            &ScanOptions {
                recursive: true,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(nested, vec!["a.jpg".to_string(), "sub/b.jpg".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_ignores_files_created_later() {
        let root = std::env::temp_dir().join(format!("scan-later-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        touch(&root.join("a.jpg"), "a");
        let found = scan(&root, &ScanOptions::default()).unwrap();
        touch(&root.join("b.jpg"), "b");
        assert_eq!(found, vec!["a.jpg".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn modified_since_keeps_that_local_day() {
        let root = std::env::temp_dir().join(format!("scan-date-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(&root.join("new.jpg"), "n");
        touch(&root.join("old.jpg"), "o");
        let old = File::options().write(true).open(root.join("old.jpg")).unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs(10 * 24 * 3600))
            .unwrap();
        let today = Local::now().date_naive();
        let found = scan(
            &root,
            &ScanOptions {
                modified_since: Some(today),
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(found, vec!["new.jpg".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn junk_files_are_skipped_only_when_asked() {
        let root = std::env::temp_dir().join(format!("scan-junk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(&root.join("a.jpg"), "a");
        touch(&root.join(".DS_Store"), "d");
        touch(&root.join("._a.jpg"), "x");
        let skipped = scan(&root, &ScanOptions::default()).unwrap();
        assert_eq!(skipped, vec!["a.jpg".to_string()]);
        let kept = scan(
            &root,
            &ScanOptions {
                skip_junk: false,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            kept,
            vec![".DS_Store".to_string(), "._a.jpg".to_string(), "a.jpg".to_string()]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_folder_is_an_empty_snapshot() {
        let root = std::env::temp_dir().join(format!("scan-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(scan(&root, &ScanOptions::default()).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn icloudpd_partials_and_prune_files_are_skipped_when_asked() {
        let root = std::env::temp_dir().join(format!("scan-icloud-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        touch(&root.join("a.jpg"), "a");
        touch(&root.join("skip.part"), "part");
        touch(&root.join(".icloudpd-prune.json"), "{}");
        touch(&root.join(".icloudpd-pruned").join("old.txt"), "old");
        let found = scan(
            &root,
            &ScanOptions {
                recursive: true,
                skip_icloudpd: true,
                ..ScanOptions::default()
            },
        )
        .unwrap();
        assert_eq!(found, vec!["a.jpg".to_string()]);
        let _ = fs::remove_dir_all(&root);
    }
}
