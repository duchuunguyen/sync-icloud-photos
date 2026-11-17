use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use chrono::NaiveDate;

use crate::copy::{copy_reporting, partial_path, CopyAction, CopyError};
use crate::hash::md5_file;
use crate::icloud_log::{self, IcloudProgress, IcloudStatus};
use crate::review::{duplicate_groups, removal_candidates};
use crate::scan::{scan, ScanOptions};
use crate::sort::{place_file, safe_join, PlaceResult};
use crate::store::{FileRow, ReviewItem, Store};

const MAX_ATTEMPTS: u32 = 3;

pub struct Flag {
    pause: AtomicBool,
    stop: AtomicBool,
    pub pause_after: AtomicUsize,
    finished: AtomicUsize,
    stopping: AtomicBool,
    files: Mutex<HashMap<String, (u64, u64)>>,
    icloud: Mutex<IcloudProgress>,
}

impl Default for Flag {
    fn default() -> Self {
        Self {
            pause: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            pause_after: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            files: Mutex::new(HashMap::new()),
            icloud: Mutex::new(IcloudProgress::default()),
        }
    }
}

impl Flag {
    pub fn request_pause(&self) {
        self.pause.store(true, Ordering::Relaxed);
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        self.pause.store(false, Ordering::Relaxed);
        self.stop.store(false, Ordering::Relaxed);
    }

    pub fn set_file_progress(&self, rel: &str, done: u64, total: u64) {
        if let Ok(mut files) = self.files.lock() {
            files.insert(rel.to_string(), (done, total));
        }
    }

    pub fn clear_file_progress(&self, rel: &str) {
        if let Ok(mut files) = self.files.lock() {
            files.remove(rel);
        }
    }

    pub fn file_progress(&self, rel: &str) -> Option<(u64, u64)> {
        self.files.lock().ok()?.get(rel).copied()
    }

    pub fn observe_icloud_line(&self, line: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        icloud_log::note_line(&mut self.icloud.lock().expect("icloud"), line, now);
    }

    pub fn observe_icloud_exit(&self, code: Option<i32>) {
        icloud_log::note_exit(&mut self.icloud.lock().expect("icloud"), code, self.stopping.load(Ordering::Relaxed));
    }

    pub fn icloud_status(&self) -> IcloudStatus {
        self.icloud.lock().expect("icloud").status()
    }

    pub fn mark_stopping(&self) {
        self.stopping.store(true, Ordering::Relaxed);
    }

    pub fn is_halted(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.pause.load(Ordering::Relaxed)
    }

    fn halt(&self) -> Option<&'static str> {
        if self.stop.load(Ordering::Relaxed) {
            return Some("stopped");
        }
        let after = self.pause_after.load(Ordering::Relaxed);
        if self.pause.load(Ordering::Relaxed) || (after > 0 && self.finished.load(Ordering::Relaxed) >= after) {
            return Some("paused");
        }
        None
    }
}

pub fn boot(store: &Store, flag: &Flag) -> rusqlite::Result<()> {
    recover_interrupted(store)?;
    for id in store.running_job_ids()? {
        let _ = run_job(store, &id, flag);
    }
    Ok(())
}

pub fn recover_interrupted(store: &Store) -> rusqlite::Result<()> {
    for (_job, dest_root, rel) in store.copying()? {
        if let Ok(dest) = safe_join(Path::new(&dest_root), &rel) {
            let _ = fs::remove_file(partial_path(&dest));
        }
    }
    store.reset_interrupted_files()?;
    Ok(())
}

pub fn run_job(store: &Store, job_id: &str, flag: &Flag) -> rusqlite::Result<()> {
    if store.get_job(job_id)?.is_some_and(|job| job.icloud) {
        return run_live(store, job_id, flag);
    }
    loop {
        let Some(job) = store.get_job(job_id)? else {
            return Ok(());
        };
        if job.state == "needs_review" || job.state == "completed" || job.state == "completed_with_errors" {
            return Ok(());
        }
        if let Some(state) = flag.halt() {
            store.set_state(job_id, state)?;
            store.set_current(job_id, None)?;
            return Ok(());
        }
        match job.stage.as_str() {
            "scan" => stage_scan(store, &job)?,
            "copy" => {
                if !copy_batch(store, &job, flag)? {
                    store.set_stage(job_id, "verify")?;
                }
            }
            "verify" => {
                if !verify_batch(store, &job)? {
                    let next = if job.sort_into_months {
                        "arrange"
                    } else if job.review_duplicates {
                        "duplicate_review"
                    } else if job.remove_missing {
                        "mirror_review"
                    } else {
                        "done"
                    };
                    store.set_stage(job_id, next)?;
                }
            }
            "arrange" => {
                if !arrange_one(store, &job)? {
                    let next = if job.review_duplicates { "duplicate_review" } else { "done" };
                    store.set_stage(job_id, next)?;
                }
            }
            "duplicate_review" => enter_duplicate_review(store, &job)?,
            "mirror_review" => enter_mirror_review(store, &job)?,
            _ => {
                finish(store, job_id)?;
                return Ok(());
            }
        }
    }
}

pub fn resolve_review(store: &Store, job_id: &str, confirm: bool) -> rusqlite::Result<()> {
    let Some(job) = store.get_job(job_id)? else {
        return Ok(());
    };
    if job.state != "needs_review" {
        return Ok(());
    }
    let items = store.review_items(job_id)?;
    if confirm {
        for item in &items {
            if let Ok(path) = safe_join(Path::new(&job.dest), &item.remove_path) {
                let _ = fs::remove_file(path);
            }
            store.forget(&job.dest, &item.remove_path)?;
        }
    }
    store.clear_review(job_id)?;
    if job.icloud {
        if job.stage == "duplicate_review" && job.remove_missing && !job.sort_into_months {
            store.set_stage(job_id, "mirror_review")?;
            store.set_state(job_id, "running")?;
            let job = store.get_job(job_id)?.expect("job");
            enter_mirror_review(store, &job)?;
            if store.get_job(job_id)?.is_some_and(|job| job.state == "needs_review") {
                return Ok(());
            }
        }
        store.set_stage(job_id, "sync")?;
        store.set_state(job_id, "running")?;
        store.set_current(job_id, None)?;
        return Ok(());
    }
    if job.stage == "duplicate_review" && job.remove_missing && !job.sort_into_months {
        store.set_stage(job_id, "mirror_review")?;
        store.set_state(job_id, "running")?;
        let job = store.get_job(job_id)?.expect("job");
        enter_mirror_review(store, &job)?;
        if store.get_job(job_id)?.is_some_and(|job| job.stage == "done") {
            finish(store, job_id)?;
        }
        return Ok(());
    }
    finish(store, job_id)?;
    Ok(())
}

fn stage_scan(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<()> {
    let opts = ScanOptions {
        recursive: job.recursive,
        modified_since: job.modified_since.as_deref().and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()),
        skip_junk: job.skip_junk,
        skip_icloudpd: false,
    };
    match scan(Path::new(&job.source), &opts) {
        Ok(rels) => {
            store.replace_snapshot(&job.id, &rels)?;
            store.set_error(&job.id, None)?;
            if rels.is_empty() {
                store.set_stage(&job.id, "done")?;
            } else {
                store.set_stage(&job.id, "copy")?;
            }
        }
        Err(err) => {
            store.set_error(&job.id, Some(&err.to_string()))?;
            store.set_stage(&job.id, "done")?;
            store.set_state(&job.id, "completed_with_errors")?;
        }
    }
    Ok(())
}

fn copy_batch(store: &Store, job: &crate::store::JobRecord, flag: &Flag) -> rusqlite::Result<bool> {
    let mut batch = Vec::new();
    for _ in 0..job.workers.max(1) {
        match store.claim(&job.id, &["pending", "copy_failed"], "copying")? {
            Some(file) => batch.push(file),
            None => break,
        }
    }
    if batch.is_empty() {
        return Ok(false);
    }
    std::thread::scope(|scope| {
        for file in &batch {
            scope.spawn(|| {
                let _ = copy_claimed(store, job, file, flag);
            });
        }
    });
    Ok(true)
}

fn verify_batch(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<bool> {
    let mut batch = Vec::new();
    for _ in 0..job.workers.max(1) {
        match store.claim(&job.id, &["copied", "verify_failed"], "verifying")? {
            Some(file) => batch.push(file),
            None => break,
        }
    }
    if batch.is_empty() {
        return Ok(false);
    }
    std::thread::scope(|scope| {
        for file in &batch {
            scope.spawn(|| {
                let _ = verify_claimed(store, job, file);
            });
        }
    });
    Ok(true)
}

fn copy_claimed(store: &Store, job: &crate::store::JobRecord, file: &FileRow, flag: &Flag) -> rusqlite::Result<()> {
    let src = match safe_join(Path::new(&job.source), &file.rel_path) {
        Ok(path) => path,
        Err(err) => {
            fail_attempt(store, job, &file, &err.to_string(), "copy_failed")?;
            return Ok(());
        }
    };
    let dest = match safe_join(Path::new(&job.dest), &file.rel_path) {
        Ok(path) => path,
        Err(err) => {
            fail_attempt(store, job, &file, &err.to_string(), "copy_failed")?;
            return Ok(());
        }
    };
    store.set_file(&job.id, &file.rel_path, "copying", file.attempts, None, Some(&file.rel_path))?;
    store.set_current(&job.id, Some(&file.rel_path))?;
    let reported = std::cell::Cell::new(u64::MAX);
    let outcome = copy_reporting(&src, &dest, &|| flag.halt().is_some(), &|done, total| {
        let previous = reported.get();
        if previous == u64::MAX || done == total || done.saturating_sub(previous) >= 1024 * 1024 {
            reported.set(done);
            flag.set_file_progress(&file.rel_path, done, total);
        }
    });
    flag.clear_file_progress(&file.rel_path);
    match outcome {
        Ok(CopyAction::SkippedIdentical) => {
            store.set_file(&job.id, &file.rel_path, "verified", file.attempts, None, Some(&file.rel_path))?;
            store.record(&job.dest, &file.rel_path, &job.id)?;
            flag.finished.fetch_add(1, Ordering::Relaxed);
        }
        Ok(CopyAction::Copied) => {
            // copy_reporting already matched the MD5 of source and copy before renaming; hashing both again adds nothing.
            store.set_file(&job.id, &file.rel_path, "verified", file.attempts, None, Some(&file.rel_path))?;
            store.record(&job.dest, &file.rel_path, &job.id)?;
            flag.finished.fetch_add(1, Ordering::Relaxed);
        }
        Err(CopyError::Aborted) => {
            store.set_file(&job.id, &file.rel_path, "pending", file.attempts, None, Some(&file.rel_path))?;
        }
        Err(CopyError::HashMismatch) => {
            fail_attempt(store, job, &file, "checksum did not match after copy", "copy_failed")?;
        }
        Err(CopyError::Io(_)) if !src.is_file() => {
            // The download disappeared (icloudpd renamed or removed it), so there is nothing left to copy.
            store.drop_file(&job.id, &file.rel_path)?;
        }
        Err(CopyError::Io(err)) => {
            fail_attempt(store, job, &file, &err.to_string(), "copy_failed")?;
        }
    }
    Ok(())
}

fn verify_claimed(store: &Store, job: &crate::store::JobRecord, file: &FileRow) -> rusqlite::Result<()> {
    let src = safe_join(Path::new(&job.source), &file.rel_path);
    let dest_rel = file.dest_rel.clone().unwrap_or_else(|| file.rel_path.clone());
    let dest = safe_join(Path::new(&job.dest), &dest_rel);
    let status = match (src, dest) {
        (Ok(src), Ok(dest)) => verify_status(&src, &dest),
        (Err(err), _) | (_, Err(err)) => Err(err.to_string()),
    };
    match status {
        Ok(()) => {
            store.set_file(&job.id, &file.rel_path, "verified", file.attempts, None, Some(&dest_rel))?;
            store.record(&job.dest, &dest_rel, &job.id)?;
        }
        Err(message) => {
            let attempts = file.attempts + 1;
            if attempts >= MAX_ATTEMPTS {
                store.set_file(&job.id, &file.rel_path, "failed", attempts, Some(&message), Some(&dest_rel))?;
            } else {
                store.set_file(&job.id, &file.rel_path, "verify_failed", attempts, Some(&message), Some(&dest_rel))?;
            }
        }
    }
    Ok(())
}

fn verify_status(src: &Path, dest: &Path) -> Result<(), String> {
    if !dest.is_file() {
        return Err("destination file is missing".into());
    }
    let left = md5_file(src).map_err(|err| err.to_string())?;
    let right = md5_file(dest).map_err(|err| err.to_string())?;
    if left == right {
        Ok(())
    } else {
        Err("checksum does not match".into())
    }
}

fn arrange_one(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<bool> {
    let Some(file) = store.claim(&job.id, &["verified"], "arranging")? else {
        return Ok(false);
    };
    let dest_rel = file.dest_rel.clone().unwrap_or_else(|| file.rel_path.clone());
    let src = match safe_join(Path::new(&job.dest), &dest_rel) {
        Ok(path) => path,
        Err(err) => {
            store.set_file(&job.id, &file.rel_path, "failed", MAX_ATTEMPTS, Some(&err.to_string()), Some(&dest_rel))?;
            return Ok(true);
        }
    };
    match place_file(&src, Path::new(&job.dest)) {
        Ok(PlaceResult::Moved(rel) | PlaceResult::AlreadyPresent(rel) | PlaceResult::Disambiguated(rel)) => {
            if rel != dest_rel {
                store.forget(&job.dest, &dest_rel)?;
            }
            store.record(&job.dest, &rel, &job.id)?;
            store.set_file(&job.id, &file.rel_path, "arranged", file.attempts, None, Some(&rel))?;
            // The copy landed in YYYY/MM/DD first; drop those folders once empty (remove_dir refuses a non-empty one).
            for dir in src.ancestors().skip(1).take_while(|dir| *dir != Path::new(&job.dest)) {
                if fs::remove_dir(dir).is_err() {
                    break;
                }
            }
        }
        Err(err) => {
            store.set_file(&job.id, &file.rel_path, "failed", MAX_ATTEMPTS, Some(&err.to_string()), Some(&dest_rel))?;
        }
    }
    Ok(true)
}

fn enter_duplicate_review(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<()> {
    let files = media_with_md5(Path::new(&job.dest));
    let groups = duplicate_groups(&files);
    if groups.is_empty() {
        let next = if job.remove_missing && !job.sort_into_months { "mirror_review" } else { "done" };
        store.set_stage(&job.id, next)?;
        return Ok(());
    }
    let items = groups
        .into_iter()
        .flat_map(|group| {
            group.remove.into_iter().map(move |remove| ReviewItem {
                kind: "duplicate".into(),
                keep_path: Some(group.keep.clone()),
                remove_path: remove,
            })
        })
        .collect::<Vec<_>>();
    store.replace_review(&job.id, &items)?;
    store.set_state(&job.id, "needs_review")?;
    store.set_current(&job.id, None)?;
    Ok(())
}

fn enter_mirror_review(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<()> {
    if job.sort_into_months || !job.remove_missing {
        store.set_stage(&job.id, "done")?;
        return Ok(());
    }
    let recorded = store.recorded_before(&job.dest, &job.id)?;
    let snapshot = store.snapshot_paths(&job.id)?;
    let missing = removal_candidates(&recorded, &snapshot);
    if missing.is_empty() {
        store.set_stage(&job.id, "done")?;
        return Ok(());
    }
    let items = missing
        .into_iter()
        .map(|path| ReviewItem {
            kind: "removal".into(),
            keep_path: None,
            remove_path: path,
        })
        .collect::<Vec<_>>();
    store.replace_review(&job.id, &items)?;
    store.set_state(&job.id, "needs_review")?;
    store.set_current(&job.id, None)?;
    Ok(())
}

const RETRY_FAILED_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

fn run_live(store: &Store, job_id: &str, flag: &Flag) -> rusqlite::Result<()> {
    store.set_stage(job_id, "sync")?;
    let mut last_scan = std::time::Instant::now() - std::time::Duration::from_secs(10);
    let mut last_retry = std::time::Instant::now();
    let mut scan_every = std::time::Duration::from_secs(1);
    if let Some(job) = store.get_job(job_id)? {
        reconcile(store, &job)?;
    }
    loop {
        let Some(job) = store.get_job(job_id)? else {
            return Ok(());
        };
        if job.state == "needs_review" {
            return Ok(());
        }
        if let Some(state) = flag.halt() {
            store.set_state(job_id, state)?;
            store.set_current(job_id, None)?;
            return Ok(());
        }
        if last_scan.elapsed() >= scan_every {
            let started = std::time::Instant::now();
            ingest(store, &job)?;
            last_scan = std::time::Instant::now();
            // A 100k-file tree takes seconds to walk: wait ten times as long as the last scan took.
            scan_every = scan_every.max(started.elapsed() * 10);
        }
        if last_retry.elapsed() >= RETRY_FAILED_EVERY {
            store.retry_failed(job_id)?;
            reconcile(store, &job)?;
            last_retry = std::time::Instant::now();
        }
        let copied = copy_batch(store, &job, flag)?;
        let verified = verify_batch(store, &job)?;
        let mut arranged = false;
        if job.sort_into_months {
            for _ in 0..job.workers.max(1) {
                if !arrange_one(store, &job)? {
                    break;
                }
                arranged = true;
            }
        }
        if open_live_review(store, &job, flag)? {
            return Ok(());
        }
        if !copied && !verified && !arranged {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}

/// The database says a file is filed, but someone may have moved or deleted it since. A stat per file is cheap,
/// so check the disk and queue anything gone to be copied again from the download folder.
fn reconcile(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<usize> {
    let mut requeued = 0;
    for (rel, dest_rel) in store.filed(&job.id)? {
        let present = safe_join(Path::new(&job.dest), &dest_rel).is_ok_and(|path| path.is_file());
        if !present {
            store.forget(&job.dest, &dest_rel)?;
            // Source gone too (icloudpd renamed or removed it): nothing to copy, so forget the row.
            if safe_join(Path::new(&job.source), &rel).is_ok_and(|path| path.is_file()) {
                store.requeue(&job.id, &rel)?;
                requeued += 1;
            } else {
                store.drop_file(&job.id, &rel)?;
            }
        }
    }
    Ok(requeued)
}

fn ingest(store: &Store, job: &crate::store::JobRecord) -> rusqlite::Result<()> {
    let opts = ScanOptions {
        recursive: true,
        modified_since: job.modified_since.as_deref().and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()),
        skip_junk: job.skip_junk,
        skip_icloudpd: true,
    };
    match scan(Path::new(&job.source), &opts) {
        Ok(rels) => {
            store.add_missing_files(&job.id, &rels)?;
            store.set_error(&job.id, None)
        }
        Err(err) => {
            store.set_error(&job.id, Some(&err.to_string()))?;
            Ok(())
        }
    }
}

fn open_live_review(store: &Store, job: &crate::store::JobRecord, flag: &Flag) -> rusqlite::Result<bool> {
    let pass_complete = flag.icloud.lock().expect("icloud").pass_complete;
    if !pass_complete {
        return Ok(false);
    }
    let wants_removal = job.remove_missing && !job.sort_into_months;
    if !job.review_duplicates && !wants_removal {
        flag.icloud.lock().expect("icloud").clear_pass();
        return Ok(false);
    }
    if store.has_open_work(&job.id, job.sort_into_months)? {
        return Ok(false);
    }
    if job.review_duplicates {
        store.set_stage(&job.id, "duplicate_review")?;
        enter_duplicate_review(store, job)?;
        if store.get_job(&job.id)?.is_some_and(|job| job.state == "needs_review") {
            return Ok(true);
        }
    }
    if wants_removal {
        let Some(job) = store.get_job(&job.id)? else {
            return Ok(false);
        };
        store.set_stage(&job.id, "mirror_review")?;
        enter_mirror_review(store, &job)?;
        if store.get_job(&job.id)?.is_some_and(|job| job.state == "needs_review") {
            return Ok(true);
        }
    }
    store.set_stage(&job.id, "sync")?;
    flag.icloud.lock().expect("icloud").clear_pass();
    Ok(false)
}

fn finish(store: &Store, job_id: &str) -> rusqlite::Result<()> {
    let errors = store.errors(job_id)?;
    let state = if errors.is_empty() && store.get_job(job_id)?.and_then(|job| job.error).is_none() {
        "completed"
    } else {
        "completed_with_errors"
    };
    store.set_stage(job_id, "done")?;
    store.set_state(job_id, state)?;
    store.set_current(job_id, None)?;
    Ok(())
}

fn fail_attempt(store: &Store, job: &crate::store::JobRecord, file: &FileRow, message: &str, retry_state: &str) -> rusqlite::Result<()> {
    let attempts = file.attempts + 1;
    if attempts >= MAX_ATTEMPTS {
        store.set_file(&job.id, &file.rel_path, "failed", attempts, Some(message), Some(&file.rel_path))?;
    } else {
        store.set_file(&job.id, &file.rel_path, retry_state, attempts, Some(message), Some(&file.rel_path))?;
    }
    Ok(())
}

/// MD5 of every media file that shares a folder and normalized name with another file.
/// A file with a unique name cannot be a duplicate, so it is never read.
fn media_with_md5(root: &Path) -> Vec<(String, String)> {
    let mut by_name: HashMap<(String, String), Vec<String>> = HashMap::new();
    walk_media(root, root, &mut by_name);
    let candidates: Vec<String> = by_name.into_values().filter(|group| group.len() > 1).flatten().collect();
    candidates
        .into_iter()
        .filter_map(|rel| md5_file(&root.join(&rel)).ok().map(|md5| (rel, md5)))
        .collect()
}

fn walk_media(root: &Path, dir: &Path, out: &mut HashMap<(String, String), Vec<String>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_media(root, &path, out);
            continue;
        }
        if !is_media(&path) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let name = entry.file_name().to_string_lossy().into_owned();
        let folder = rel.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
        out.entry((folder, crate::names::normalize_file_name(&name))).or_default().push(rel);
    }
}

fn is_media(path: &Path) -> bool {
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "heic" | "png" | "webp" | "mov" | "mp4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewJob;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{label}-{}-{}", std::process::id(), Store::fresh_id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn job(source: &Path, dest: &Path, review: bool, remove: bool, sort: bool) -> NewJob {
        NewJob {
            source: source.to_string_lossy().into_owned(),
            dest: dest.to_string_lossy().into_owned(),
            recursive: false,
            modified_since: None,
            skip_junk: true,
            sort_into_months: sort,
            review_duplicates: review,
            remove_missing: remove,
            workers: 1,
            icloud: false,
            apple_id: String::new(),
            watch_secs: 3600,
        }
    }

    fn contains_file(root: &Path, name: &str) -> bool {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|item| item.to_str()) == Some(name) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn live_job_copies_files_as_they_appear_and_skips_partials() {
        let store = Store::open_in_memory().unwrap();
        let source = scratch("live-src");
        let dest = scratch("live-dst");
        fs::write(source.join("a.txt"), "a").unwrap();
        fs::write(source.join("skip.part"), "nope").unwrap();
        fs::create_dir_all(source.join(".icloudpd-pruned")).unwrap();
        fs::write(source.join(".icloudpd-pruned").join("old.txt"), "old").unwrap();
        fs::write(source.join(".icloudpd-prune.json"), "{}").unwrap();
        let mut created = job(&source, &dest, false, false, true);
        created.icloud = true;
        created.recursive = true;
        created.apple_id = "me@icloud.com".into();
        let id = store.create_job(created).unwrap();
        store.set_state(&id, "running").unwrap();
        let flag = Flag::default();
        flag.pause_after.store(2, Ordering::Relaxed);
        let later = source.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(700));
            fs::write(later.join("b.txt"), "b").unwrap();
        });
        run_job(&store, &id, &flag).unwrap();
        assert!(contains_file(&dest, "a.txt"));
        assert!(contains_file(&dest, "b.txt"));
        assert!(!contains_file(&dest, "old.txt"));
        assert!(!contains_file(&dest, "skip.part"));
        assert_eq!(store.arranged(&id).unwrap(), 2);
        assert_eq!(store.progress(&id).unwrap(), (2, 2));
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "paused");
    }

    #[test]
    fn one_file_error_does_not_cancel_the_other() {
        let root = scratch("err");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("good.jpg"), b"good").unwrap();
        fs::write(source.join("bad.jpg"), b"bad").unwrap();
        let mut perms = fs::metadata(source.join("bad.jpg")).unwrap().permissions();
        perms.set_mode(0);
        fs::set_permissions(source.join("bad.jpg"), perms).unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, false, false, false)).unwrap();
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &Flag::default()).unwrap();
        let mut perms = fs::metadata(source.join("bad.jpg")).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(source.join("bad.jpg"), perms).unwrap();
        assert_eq!(fs::read(dest.join("good.jpg")).unwrap(), b"good");
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "completed_with_errors");
        let errors = store.errors(&id).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].rel_path, "bad.jpg");
        assert_eq!(errors[0].attempts, 3);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_source_completes() {
        let root = scratch("empty");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, false, false, false)).unwrap();
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &Flag::default()).unwrap();
        let done = store.get_job(&id).unwrap().unwrap();
        assert_eq!(done.state, "completed");
        assert_eq!(store.progress(&id).unwrap(), (0, 0));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pause_then_resume_keeps_the_snapshot() {
        let root = scratch("pause");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a.jpg"), b"a").unwrap();
        fs::write(source.join("b.jpg"), b"b").unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, false, false, false)).unwrap();
        store.set_state(&id, "running").unwrap();
        let flag = Flag::default();
        flag.pause_after.store(1, Ordering::Relaxed);
        run_job(&store, &id, &flag).unwrap();
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "paused");
        assert_eq!(store.file_count(&id).unwrap(), 2);
        fs::write(source.join("c.jpg"), b"c").unwrap();
        flag.clear();
        flag.pause_after.store(0, Ordering::Relaxed);
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &flag).unwrap();
        assert_eq!(store.file_count(&id).unwrap(), 2);
        assert_eq!(fs::read(dest.join("a.jpg")).unwrap(), b"a");
        assert_eq!(fs::read(dest.join("b.jpg")).unwrap(), b"b");
        assert!(!dest.join("c.jpg").exists());
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "completed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_does_not_auto_resume_and_resume_continues() {
        let root = scratch("stop");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a.jpg"), b"a").unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, false, false, false)).unwrap();
        store.set_state(&id, "running").unwrap();
        let flag = Flag::default();
        flag.request_stop();
        run_job(&store, &id, &flag).unwrap();
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "stopped");
        assert!(store.running_job_ids().unwrap().is_empty());
        flag.clear();
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &flag).unwrap();
        assert_eq!(fs::read(dest.join("a.jpg")).unwrap(), b"a");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn boot_continues_running_and_leaves_paused_and_review() {
        let root = scratch("boot");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a.jpg"), b"from-source").unwrap();
        fs::write(dest.join("a.jpg"), b"old").unwrap();
        let store = Store::open_in_memory().unwrap();
        let running = store.create_job(job(&source, &dest, false, false, false)).unwrap();
        store.replace_snapshot(&running, &["a.jpg".into()]).unwrap();
        store.set_file(&running, "a.jpg", "copying", 0, None, Some("a.jpg")).unwrap();
        store.set_stage(&running, "copy").unwrap();
        store.set_state(&running, "running").unwrap();
        fs::write(partial_path(&dest.join("a.jpg")), b"partial").unwrap();

        let paused_source = root.join("paused-src");
        fs::create_dir_all(&paused_source).unwrap();
        fs::write(paused_source.join("p.jpg"), b"p").unwrap();
        let paused = store.create_job(job(&paused_source, &root.join("paused-dst"), false, false, false)).unwrap();
        store.replace_snapshot(&paused, &["p.jpg".into()]).unwrap();
        store.set_stage(&paused, "copy").unwrap();
        store.set_state(&paused, "paused").unwrap();

        let review = store.create_job(job(&source, &root.join("review-dst"), true, false, false)).unwrap();
        store.set_stage(&review, "duplicate_review").unwrap();
        store.set_state(&review, "needs_review").unwrap();

        boot(&store, &Flag::default()).unwrap();
        assert!(!partial_path(&dest.join("a.jpg")).exists());
        assert_eq!(fs::read(dest.join("a.jpg")).unwrap(), b"from-source");
        assert_eq!(store.get_job(&running).unwrap().unwrap().state, "completed");
        assert_eq!(store.get_job(&paused).unwrap().unwrap().state, "paused");
        assert!(!root.join("paused-dst").join("p.jpg").exists());
        assert_eq!(store.get_job(&review).unwrap().unwrap().state, "needs_review");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_mismatch_keeps_the_destination_after_three_attempts() {
        let root = scratch("verify");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a.jpg"), b"source").unwrap();
        fs::write(dest.join("a.jpg"), b"different").unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, false, false, false)).unwrap();
        store.replace_snapshot(&id, &["a.jpg".into()]).unwrap();
        store.set_file(&id, "a.jpg", "copied", 0, None, Some("a.jpg")).unwrap();
        store.set_stage(&id, "verify").unwrap();
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &Flag::default()).unwrap();
        assert_eq!(fs::read(dest.join("a.jpg")).unwrap(), b"different");
        let errors = store.errors(&id).unwrap();
        assert_eq!(errors[0].attempts, 3);
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "completed_with_errors");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn retry_failed_puts_exhausted_files_back_to_pending() {
        let store = Store::open_in_memory().unwrap();
        let root = scratch("retry");
        let id = store.create_job(job(&root, &root, false, false, false)).unwrap();
        store.replace_snapshot(&id, &["a.jpg".into()]).unwrap();
        store.set_file(&id, "a.jpg", "failed", 3, Some("disk gone"), None).unwrap();
        assert_eq!(store.retry_failed(&id).unwrap(), 1);
        assert!(store.errors(&id).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_confirm_deletes_and_skip_does_not() {
        let root = scratch("dups");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("IMG_0381.jpg"), b"same").unwrap();
        fs::write(source.join("IMG_0381(1).jpg"), b"same").unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, true, false, false)).unwrap();
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &Flag::default()).unwrap();
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "needs_review");
        let items = store.review_items(&id).unwrap();
        assert_eq!(items[0].keep_path.as_deref(), Some("IMG_0381.jpg"));
        assert_eq!(items[0].remove_path, "IMG_0381(1).jpg");
        resolve_review(&store, &id, false).unwrap();
        assert!(dest.join("IMG_0381(1).jpg").is_file());

        let source2 = root.join("src2");
        let dest2 = root.join("dst2");
        fs::create_dir_all(&source2).unwrap();
        fs::write(source2.join("IMG_0381.jpg"), b"same").unwrap();
        fs::write(source2.join("IMG_0381(1).jpg"), b"same").unwrap();
        let id2 = store.create_job(job(&source2, &dest2, true, false, false)).unwrap();
        store.set_state(&id2, "running").unwrap();
        run_job(&store, &id2, &Flag::default()).unwrap();
        resolve_review(&store, &id2, true).unwrap();
        assert!(dest2.join("IMG_0381.jpg").is_file());
        assert!(!dest2.join("IMG_0381(1).jpg").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn removal_is_empty_on_the_first_run_and_confirms_only_recorded_files() {
        let root = scratch("removal");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a.jpg"), b"a").unwrap();
        fs::write(dest.join("stray.jpg"), b"stray").unwrap();
        let store = Store::open_in_memory().unwrap();
        let first = store.create_job(job(&source, &dest, false, true, false)).unwrap();
        store.set_state(&first, "running").unwrap();
        run_job(&store, &first, &Flag::default()).unwrap();
        assert_eq!(store.get_job(&first).unwrap().unwrap().state, "completed");
        assert!(dest.join("stray.jpg").is_file());
        assert!(dest.join("a.jpg").is_file());

        let source2 = root.join("src2");
        fs::create_dir_all(&source2).unwrap();
        fs::write(source2.join("b.jpg"), b"b").unwrap();
        let second = store.create_job(job(&source2, &dest, false, true, false)).unwrap();
        store.set_state(&second, "running").unwrap();
        run_job(&store, &second, &Flag::default()).unwrap();
        assert_eq!(store.get_job(&second).unwrap().unwrap().state, "needs_review");
        let items = store.review_items(&second).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].remove_path, "a.jpg");
        resolve_review(&store, &second, true).unwrap();
        assert!(!dest.join("a.jpg").exists());
        assert!(dest.join("stray.jpg").is_file());
        assert!(dest.join("b.jpg").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sort_puts_an_exif_jpeg_in_its_capture_month() {
        let root = scratch("sort-job");
        let source = root.join("src");
        let dest = root.join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("shot.jpg"), jpeg_bytes("2020:01:15 08:30:00")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let id = store.create_job(job(&source, &dest, false, false, true)).unwrap();
        store.set_state(&id, "running").unwrap();
        run_job(&store, &id, &Flag::default()).unwrap();
        assert!(dest.join("2020-01").join("shot.jpg").is_file());
        assert!(!dest.join("shot.jpg").exists());
        assert!(!dest.join("webp").exists());
        assert_eq!(store.get_job(&id).unwrap().unwrap().state, "completed");
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
fn jpeg_bytes(date_time: &str) -> Vec<u8> {
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
