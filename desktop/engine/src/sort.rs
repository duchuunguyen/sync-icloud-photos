use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::dates::{filing_date, month_folder};
use crate::hash::md5_file;

#[derive(Debug, PartialEq, Eq)]
pub enum PlaceResult {
    Moved(String),
    AlreadyPresent(String),
    Disambiguated(String),
}

/// Move `src` into `dest_root/YYYY-MM/`. Same name and same MD5 keeps the file already there.
/// Same name and a different MD5 writes `name (N).ext` and keeps the original.
pub fn place_file(src: &Path, dest_root: &Path) -> io::Result<PlaceResult> {
    let day = filing_date(src)?;
    place_file_on_day(src, dest_root, day)
}

pub fn place_file_on_day(src: &Path, dest_root: &Path, day: NaiveDate) -> io::Result<PlaceResult> {
    let file_name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?;
    let folder = dest_root.join(month_folder(day));
    fs::create_dir_all(&folder)?;
    let target = folder.join(file_name);
    if src == target {
        return Ok(PlaceResult::Moved(rel_string(dest_root, &target)));
    }
    if target.is_file() {
        if md5_file(src)? == md5_file(&target)? {
            let _ = fs::remove_file(src);
            return Ok(PlaceResult::AlreadyPresent(rel_string(dest_root, &target)));
        }
        let free = next_copy_name(&folder, &file_name.to_string_lossy());
        let dest = folder.join(&free);
        fs::rename(src, &dest)?;
        return Ok(PlaceResult::Disambiguated(rel_string(dest_root, &dest)));
    }
    fs::rename(src, &target)?;
    Ok(PlaceResult::Moved(rel_string(dest_root, &target)))
}

fn next_copy_name(folder: &Path, file_name: &str) -> String {
    let (base, ext) = split_name(file_name);
    let mut n = 1u32;
    loop {
        let candidate = match ext {
            Some(ext) => format!("{base} ({n}).{ext}"),
            None => format!("{base} ({n})"),
        };
        if !folder.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

fn split_name(file_name: &str) -> (&str, Option<&str>) {
    match file_name.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() && !ext.is_empty() => (base, Some(ext)),
        _ => (file_name, None),
    }
}

fn rel_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn safe_join(root: &Path, rel: &str) -> io::Result<PathBuf> {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') || rel.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path escapes the destination"));
    }
    Ok(root.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dates::{filing_date, month_folder};
    use chrono::{Local, TimeZone};
    use std::fs::File;
    use std::time::SystemTime;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn jpeg(date_time: &str) -> Vec<u8> {
        let mut text = date_time.as_bytes().to_vec();
        text.push(0);
        let mut tiff = Vec::new();
        tiff.extend(b"II");
        tiff.extend(&42u16.to_le_bytes());
        tiff.extend(&8u32.to_le_bytes());
        tiff.extend(&1u16.to_le_bytes());
        tiff.extend(&0x8769u16.to_le_bytes());
        tiff.extend(&4u16.to_le_bytes());
        tiff.extend(&1u32.to_le_bytes());
        tiff.extend(&26u32.to_le_bytes());
        tiff.extend(&0u32.to_le_bytes());
        tiff.extend(&1u16.to_le_bytes());
        tiff.extend(&0x9003u16.to_le_bytes());
        tiff.extend(&2u16.to_le_bytes());
        tiff.extend(&20u32.to_le_bytes());
        tiff.extend(&44u32.to_le_bytes());
        tiff.extend(&0u32.to_le_bytes());
        tiff.extend(&text);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(len.to_be_bytes());
        jpeg.extend(&app1);
        jpeg.extend([0xFF, 0xD9]);
        jpeg
    }

    fn mp4(seconds: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 100];
        payload[4..8].copy_from_slice(&seconds.to_be_bytes());
        payload[8..12].copy_from_slice(&seconds.to_be_bytes());
        payload[12..16].copy_from_slice(&1000u32.to_be_bytes());
        payload[16..20].copy_from_slice(&1000u32.to_be_bytes());
        payload[20..24].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        payload[24..26].copy_from_slice(&0x0100u16.to_be_bytes());
        payload[96..100].copy_from_slice(&1u32.to_be_bytes());
        let size = (8 + payload.len()) as u32;
        let mut mvhd = size.to_be_bytes().to_vec();
        mvhd.extend(b"mvhd");
        mvhd.extend(&payload);
        let moov_size = (8 + mvhd.len()) as u32;
        let mut moov = moov_size.to_be_bytes().to_vec();
        moov.extend(b"moov");
        moov.extend(&mvhd);
        moov
    }

    #[test]
    fn image_uses_date_time_original_not_mtime() {
        let root = scratch("sort-exif");
        let src = root.join("staging").join("shot.jpg");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, jpeg("2020:01:15 08:30:00")).unwrap();
        let local = Local.with_ymd_and_hms(2024, 8, 15, 12, 0, 0).unwrap();
        File::options().write(true).open(&src).unwrap().set_modified(SystemTime::from(local)).unwrap();
        assert_eq!(filing_date(&src).unwrap(), NaiveDate::from_ymd_opt(2020, 1, 15).unwrap());
        let dest = root.join("archive");
        let placed = place_file(&src, &dest).unwrap();
        assert_eq!(placed, PlaceResult::Moved("2020-01/shot.jpg".into()));
        assert!(dest.join("2020-01").join("shot.jpg").is_file());
        assert!(!dest.join("webp").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn video_uses_creation_date() {
        let root = scratch("sort-mov");
        let src = root.join("clip.mov");
        let expected = NaiveDate::from_ymd_opt(2019, 3, 15).unwrap();
        let base = NaiveDate::from_ymd_opt(1904, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
        let seconds = (expected.and_hms_opt(12, 0, 0).unwrap() - base).num_seconds() as u32;
        fs::write(&src, mp4(seconds)).unwrap();
        let dest = root.join("archive");
        assert_eq!(place_file(&src, &dest).unwrap(), PlaceResult::Moved("2019-03/clip.mov".into()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_capture_date_uses_mtime_and_other_files_are_not_webp() {
        let root = scratch("sort-mtime");
        let src = root.join("note.txt");
        fs::write(&src, b"plain").unwrap();
        let local = Local.with_ymd_and_hms(2024, 8, 15, 12, 0, 0).unwrap();
        File::options().write(true).open(&src).unwrap().set_modified(SystemTime::from(local)).unwrap();
        let dest = root.join("archive");
        let day = filing_date(&src).unwrap();
        assert_eq!(place_file(&src, &dest).unwrap(), PlaceResult::Moved(format!("{}/note.txt", month_folder(day))));
        assert!(!dest.join("webp").exists());
        assert!(dest.join(month_folder(day)).join("note.txt").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn same_md5_does_not_write_a_second_file() {
        let root = scratch("sort-same");
        let dest = root.join("archive");
        fs::create_dir_all(dest.join("2020-01")).unwrap();
        fs::write(dest.join("2020-01").join("shot.jpg"), jpeg("2020:01:15 08:30:00")).unwrap();
        let src = root.join("shot.jpg");
        fs::write(&src, jpeg("2020:01:15 08:30:00")).unwrap();
        assert_eq!(
            place_file(&src, &dest).unwrap(),
            PlaceResult::AlreadyPresent("2020-01/shot.jpg".into())
        );
        assert!(!src.exists());
        assert_eq!(fs::read_dir(dest.join("2020-01")).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn different_md5_writes_a_numbered_name_and_keeps_the_original() {
        let root = scratch("sort-diff");
        let dest = root.join("archive");
        fs::create_dir_all(dest.join("2020-01")).unwrap();
        let original = jpeg("2020:01:15 08:30:00");
        fs::write(dest.join("2020-01").join("shot.jpg"), &original).unwrap();
        let mut incoming = original.clone();
        incoming.push(b'x');
        let src = root.join("shot.jpg");
        fs::write(&src, &incoming).unwrap();
        assert_eq!(
            place_file(&src, &dest).unwrap(),
            PlaceResult::Disambiguated("2020-01/shot (1).jpg".into())
        );
        assert_eq!(fs::read(dest.join("2020-01").join("shot.jpg")).unwrap(), original);
        assert!(dest.join("2020-01").join("shot (1).jpg").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn safe_join_rejects_escape() {
        let root = Path::new("/tmp/archive");
        assert!(safe_join(root, "../secret").is_err());
        assert!(safe_join(root, "/etc/passwd").is_err());
        assert_eq!(safe_join(root, "2020-01/a.jpg").unwrap(), root.join("2020-01/a.jpg"));
    }
}
