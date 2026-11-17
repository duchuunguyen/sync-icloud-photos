use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use md5::Digest;

use crate::hash::md5_file;

#[derive(Debug, PartialEq, Eq)]
pub enum CopyAction {
    SkippedIdentical,
    Copied,
}

#[derive(Debug)]
pub enum CopyError {
    Aborted,
    HashMismatch,
    Io(io::Error),
}

impl From<io::Error> for CopyError {
    fn from(err: io::Error) -> Self {
        CopyError::Io(err)
    }
}

pub fn partial_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    dest.with_file_name(format!(".{name}.sync-partial"))
}

/// Copy `src` onto `dest`. An identical destination is left untouched, including its modified time.
/// New bytes are written to `.<name>.sync-partial` and renamed only after the MD5 matches.
pub fn copy_one(src: &Path, dest: &Path, should_abort: &dyn Fn() -> bool) -> Result<CopyAction, CopyError> {
    copy_reporting(src, dest, should_abort, &|_, _| {})
}

/// Same as [`copy_one`]. `on_progress` receives bytes written and the source size.
pub fn copy_reporting(
    src: &Path,
    dest: &Path,
    should_abort: &dyn Fn() -> bool,
    on_progress: &dyn Fn(u64, u64),
) -> Result<CopyAction, CopyError> {
    let same_size = fs::metadata(dest).is_ok_and(|d| d.is_file() && fs::metadata(src).is_ok_and(|s| s.len() == d.len()));
    if same_size && md5_file(src)? == md5_file(dest)? {
        return Ok(CopyAction::SkippedIdentical);
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = partial_path(dest);
    let copy_result = (|| {
        if should_abort() {
            return Err(CopyError::Aborted);
        }
        let source_md5 = write_temp(src, &temp, should_abort, on_progress)?;
        if source_md5 != md5_file(&temp)? {
            return Err(CopyError::HashMismatch);
        }
        fs::rename(&temp, dest)?;
        Ok(CopyAction::Copied)
    })();
    if copy_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    copy_result
}

fn write_temp(
    src: &Path,
    temp: &Path,
    should_abort: &dyn Fn() -> bool,
    on_progress: &dyn Fn(u64, u64),
) -> Result<String, CopyError> {
    let total = fs::metadata(src)?.len();
    let mut hasher = md5::Md5::new();
    let mut input = File::open(src)?;
    let mut output = File::create(temp)?;
    let mut buf = [0u8; 64 * 1024];
    let mut done = 0u64;
    on_progress(0, total);
    loop {
        if should_abort() {
            return Err(CopyError::Aborted);
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n])?;
        done += n as u64;
        on_progress(done, total);
    }
    // The filing date falls back to modified time, so the copy must keep the source's.
    output.set_modified(fs::metadata(src)?.modified()?)?;
    output.sync_all()?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::md5_bytes;
    use std::time::{Duration, SystemTime};

    fn dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn identical_destination_is_not_rewritten() {
        let root = dir("copy-same");
        let src = root.join("src.jpg");
        let dest = root.join("dest.jpg");
        fs::write(&src, b"same-bytes").unwrap();
        fs::write(&dest, b"same-bytes").unwrap();
        let stamp = SystemTime::now() - Duration::from_secs(3600);
        File::options().write(true).open(&dest).unwrap().set_modified(stamp).unwrap();
        let before = fs::metadata(&dest).unwrap().modified().unwrap();
        let action = copy_one(&src, &dest, &|| false).unwrap();
        assert_eq!(action, CopyAction::SkippedIdentical);
        assert_eq!(fs::metadata(&dest).unwrap().modified().unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn different_destination_is_replaced_and_matches() {
        let root = dir("copy-diff");
        let src = root.join("src.jpg");
        let dest = root.join("dest.jpg");
        fs::write(&src, b"new").unwrap();
        fs::write(&dest, b"old").unwrap();
        assert_eq!(copy_one(&src, &dest, &|| false).unwrap(), CopyAction::Copied);
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert_eq!(md5_file(&dest).unwrap(), md5_bytes(b"new"));
        assert!(!partial_path(&dest).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_destination_is_created() {
        let root = dir("copy-miss");
        let src = root.join("src.jpg");
        let dest = root.join("nested").join("dest.jpg");
        fs::write(&src, b"created").unwrap();
        assert_eq!(copy_one(&src, &dest, &|| false).unwrap(), CopyAction::Copied);
        assert_eq!(fs::read(&dest).unwrap(), b"created");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn abort_leaves_the_previous_destination_and_removes_the_temp_file() {
        let root = dir("copy-abort");
        let src = root.join("src.jpg");
        let dest = root.join("dest.jpg");
        fs::write(&src, b"incoming").unwrap();
        fs::write(&dest, b"old").unwrap();
        let err = copy_one(&src, &dest, &|| true).unwrap_err();
        assert!(matches!(err, CopyError::Aborted));
        assert_eq!(fs::read(&dest).unwrap(), b"old");
        assert!(!partial_path(&dest).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_bytes_as_it_writes() {
        let root = dir("copy-progress");
        let src = root.join("src.jpg");
        let dest = root.join("dest.jpg");
        let body = vec![7u8; 200_000];
        fs::write(&src, &body).unwrap();
        let seen = std::cell::Cell::new((0u64, 0u64));
        copy_reporting(&src, &dest, &|| false, &|done, total| seen.set((done, total))).unwrap();
        assert_eq!(seen.get(), (body.len() as u64, body.len() as u64));
        let _ = fs::remove_dir_all(&root);
    }
}
