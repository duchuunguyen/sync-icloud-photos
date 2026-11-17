use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeDelta};
use exif::{In, Reader, Tag};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Other,
}

pub fn media_kind(path: &Path) -> MediaKind {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "heic" | "png" => MediaKind::Image,
        "mov" | "mp4" => MediaKind::Video,
        _ => MediaKind::Other,
    }
}

/// Calendar date used for the `YYYY-MM` folder.
/// Images use Date/Time Original, videos use the QuickTime creation date, then last-modified time.
/// Anything else uses last-modified time. There is no `webp` bucket.
pub fn filing_date(path: &Path) -> std::io::Result<NaiveDate> {
    let modified = modified_date(path)?;
    let capture = match media_kind(path) {
        MediaKind::Other => None,
        MediaKind::Image | MediaKind::Video => read_capture_date(path),
    };
    Ok(capture.unwrap_or(modified))
}

pub fn modified_date(path: &Path) -> std::io::Result<NaiveDate> {
    let modified = fs::metadata(path)?.modified()?;
    let local: DateTime<Local> = modified.into();
    Ok(local.date_naive())
}

pub fn read_capture_date(path: &Path) -> Option<NaiveDate> {
    match media_kind(path) {
        MediaKind::Image => read_exif_date(path),
        MediaKind::Video => read_quicktime_creation(path),
        MediaKind::Other => None,
    }
}

fn read_exif_date(path: &Path) -> Option<NaiveDate> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(Tag::DateTimeOriginal, In::PRIMARY)?;
    parse_exif_date(&field.display_value().to_string())
}

fn parse_exif_date(value: &str) -> Option<NaiveDate> {
    let date = value.get(..10)?.replace(':', "-");
    NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()
}

/// QuickTime `mvhd` creation time is seconds since 1904-01-01 UTC.
/// The folder month uses that UTC date so it does not change with the machine timezone.
fn read_quicktime_creation(path: &Path) -> Option<NaiveDate> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let seconds = find_mvhd_creation(&bytes)?;
    if seconds == 0 {
        return None;
    }
    let base = NaiveDate::from_ymd_opt(1904, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let stamp = base.checked_add_signed(TimeDelta::seconds(seconds as i64))?;
    Some(stamp.date())
}

fn find_mvhd_creation(bytes: &[u8]) -> Option<u32> {
    find_in_boxes(bytes, false)
}

fn find_in_boxes(bytes: &[u8], inside_moov: bool) -> Option<u32> {
    let mut offset = 0;
    while offset + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
        let typ = &bytes[offset + 4..offset + 8];
        if size < 8 || offset + size > bytes.len() {
            return None;
        }
        let payload = &bytes[offset + 8..offset + size];
        if typ == b"mvhd" && inside_moov {
            return mvhd_creation(payload);
        }
        if typ == b"moov" {
            if let Some(found) = find_in_boxes(payload, true) {
                return Some(found);
            }
        }
        offset += size;
    }
    None
}

fn mvhd_creation(payload: &[u8]) -> Option<u32> {
    if payload.first() == Some(&0) && payload.len() >= 8 {
        Some(u32::from_be_bytes(payload[4..8].try_into().ok()?))
    } else if payload.first() == Some(&1) && payload.len() >= 12 {
        let wide = u64::from_be_bytes(payload[4..12].try_into().ok()?);
        u32::try_from(wide).ok()
    } else {
        None
    }
}

pub fn month_folder(day: NaiveDate) -> String {
    format!("{:04}-{:02}", day.year(), day.month())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jpeg_with_datetime(date_time: &str) -> Vec<u8> {
        assert_eq!(date_time.len(), 19);
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

    fn mp4_with_creation(seconds_since_1904: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 100];
        payload[4..8].copy_from_slice(&seconds_since_1904.to_be_bytes());
        payload[8..12].copy_from_slice(&seconds_since_1904.to_be_bytes());
        payload[12..16].copy_from_slice(&1000u32.to_be_bytes());
        payload[16..20].copy_from_slice(&1000u32.to_be_bytes());
        payload[20..24].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        payload[24..26].copy_from_slice(&0x0100u16.to_be_bytes());
        payload[96..100].copy_from_slice(&1u32.to_be_bytes());
        let mvhd = box_of(b"mvhd", &payload);
        let moov = box_of(b"moov", &mvhd);
        let ftyp = box_of(b"ftyp", b"isom\0\0\0\0isom");
        let mut file = ftyp;
        file.extend(moov);
        file
    }

    fn box_of(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (8 + payload.len()) as u32;
        let mut out = size.to_be_bytes().to_vec();
        out.extend(typ);
        out.extend(payload);
        out
    }

    #[test]
    fn exif_date_is_read_from_a_jpeg() {
        let root = std::env::temp_dir().join(format!("exif-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("shot.jpg");
        let mut file = File::create(&path).unwrap();
        file.write_all(&jpeg_with_datetime("2020:01:15 08:30:00")).unwrap();
        assert_eq!(read_capture_date(&path), NaiveDate::from_ymd_opt(2020, 1, 15));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn quicktime_creation_is_read_from_an_mp4() {
        let root = std::env::temp_dir().join(format!("qt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("clip.mov");
        // 2019-03-15 12:00:00 UTC. 1904-01-01 to 2019-03-15 is checked below via the reader.
        let expected = NaiveDate::from_ymd_opt(2019, 3, 15).unwrap();
        let base = NaiveDate::from_ymd_opt(1904, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
        let stamp = expected.and_hms_opt(12, 0, 0).unwrap();
        let seconds = (stamp - base).num_seconds() as u32;
        fs::write(&path, mp4_with_creation(seconds)).unwrap();
        assert_eq!(read_capture_date(&path), Some(expected));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn other_files_have_no_capture_date() {
        let root = std::env::temp_dir().join(format!("other-date-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("note.txt");
        fs::write(&path, b"hello").unwrap();
        assert_eq!(media_kind(&path), MediaKind::Other);
        assert_eq!(read_capture_date(&path), None);
        let _ = fs::remove_dir_all(&root);
    }
}
