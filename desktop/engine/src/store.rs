use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

static IDS: AtomicU64 = AtomicU64::new(1);

fn new_id() -> String {
    // Millisecond clock first, so an ID is never reused after a restart or after a sync is deleted.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    format!("{:x}-{:x}", millis, IDS.fetch_add(1, Ordering::Relaxed))
}

#[derive(Clone, Debug)]
pub struct NewJob {
    pub source: String,
    pub dest: String,
    pub recursive: bool,
    pub modified_since: Option<String>,
    pub skip_junk: bool,
    pub sort_into_months: bool,
    pub review_duplicates: bool,
    pub remove_missing: bool,
    pub workers: u32,
    pub icloud: bool,
    pub apple_id: String,
    pub watch_secs: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub source: String,
    pub dest: String,
    pub recursive: bool,
    pub modified_since: Option<String>,
    pub skip_junk: bool,
    pub sort_into_months: bool,
    pub review_duplicates: bool,
    pub remove_missing: bool,
    pub state: String,
    pub stage: String,
    pub workers: u32,
    pub current_file: Option<String>,
    pub error: Option<String>,
    pub icloud: bool,
    pub apple_id: String,
    pub watch_secs: u32,
}

#[derive(Clone, Debug)]
pub struct FileRow {
    pub rel_path: String,
    pub state: String,
    pub attempts: u32,
    pub error: Option<String>,
    pub dest_rel: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewItem {
    pub kind: String,
    pub keep_path: Option<String>,
    pub remove_path: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileError {
    pub rel_path: String,
    pub attempts: u32,
    pub error: Option<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        // WAL + NORMAL: a per-file state update no longer costs an fsync. A crash can lose the last
        // updates, and recover_interrupted / the next scan redo that work.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                dest TEXT NOT NULL,
                recursive INTEGER NOT NULL,
                modified_since TEXT,
                skip_junk INTEGER NOT NULL,
                sort_into_months INTEGER NOT NULL,
                review_duplicates INTEGER NOT NULL,
                remove_missing INTEGER NOT NULL,
                state TEXT NOT NULL,
                stage TEXT NOT NULL,
                workers INTEGER NOT NULL,
                current_file TEXT,
                error TEXT
            );
            CREATE TABLE IF NOT EXISTS files (
                job_id TEXT NOT NULL,
                rel_path TEXT NOT NULL,
                state TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                dest_rel TEXT,
                PRIMARY KEY (job_id, rel_path)
            );
            CREATE TABLE IF NOT EXISTS recorded (
                dest_root TEXT NOT NULL,
                rel_path TEXT NOT NULL,
                job_id TEXT NOT NULL,
                PRIMARY KEY (dest_root, rel_path)
            );
            CREATE TABLE IF NOT EXISTS review_items (
                job_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                keep_path TEXT,
                remove_path TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS files_by_state ON files (job_id, state, rel_path);
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        ensure_job_columns(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let dir = std::env::temp_dir().join(format!("sync-db-{}-{}", std::process::id(), new_id()));
        std::fs::create_dir_all(&dir).ok();
        Self::open(&dir.join("jobs.db"))
    }

    pub fn fresh_id() -> String {
        new_id()
    }

    pub fn create_job(&self, job: NewJob) -> rusqlite::Result<String> {
        let id = new_id();
        let workers = job.workers.clamp(1, 20);
        let stage = if job.icloud { "sync" } else { "scan" };
        let watch_secs = if job.watch_secs == 0 { 3600 } else { job.watch_secs.max(60) };
        self.conn.lock().expect("db").execute(
            "INSERT INTO jobs (id, source, dest, recursive, modified_since, skip_junk, sort_into_months, review_duplicates, remove_missing, state, stage, workers, icloud, apple_id, watch_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'draft', ?10, ?11, ?12, ?13, ?14)",
            params![
                id,
                job.source,
                job.dest,
                job.recursive as i64,
                job.modified_since,
                job.skip_junk as i64,
                job.sort_into_months as i64,
                job.review_duplicates as i64,
                job.remove_missing as i64,
                stage,
                workers,
                job.icloud as i64,
                job.apple_id,
                watch_secs,
            ],
        )?;
        Ok(id)
    }

    pub fn get_job(&self, id: &str) -> rusqlite::Result<Option<JobRecord>> {
        self.conn.lock().expect("db").query_row(
            "SELECT id, source, dest, recursive, modified_since, skip_junk, sort_into_months, review_duplicates, remove_missing, state, stage, workers, current_file, error, icloud, apple_id, watch_secs FROM jobs WHERE id = ?1",
            [id],
            map_job,
        ).optional()
    }

    pub fn list_jobs(&self) -> rusqlite::Result<Vec<JobRecord>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare(
            "SELECT id, source, dest, recursive, modified_since, skip_junk, sort_into_months, review_duplicates, remove_missing, state, stage, workers, current_file, error, icloud, apple_id, watch_secs FROM jobs ORDER BY rowid DESC",
        )?;
        let rows = stmt.query_map([], map_job)?;
        rows.collect()
    }

    pub fn set_state(&self, id: &str, state: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute("UPDATE jobs SET state = ?1 WHERE id = ?2", params![state, id])?;
        Ok(())
    }

    /// Forget a job and its bookkeeping rows. Photo files on disk are never touched.
    pub fn delete_job(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db");
        for table in ["files", "review_items", "recorded"] {
            conn.execute(&format!("DELETE FROM {table} WHERE job_id = ?1"), [id])?;
        }
        conn.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_stage(&self, id: &str, stage: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute("UPDATE jobs SET stage = ?1 WHERE id = ?2", params![stage, id])?;
        Ok(())
    }

    pub fn set_current(&self, id: &str, file: Option<&str>) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute("UPDATE jobs SET current_file = ?1 WHERE id = ?2", params![file, id])?;
        Ok(())
    }

    pub fn set_error(&self, id: &str, error: Option<&str>) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute("UPDATE jobs SET error = ?1 WHERE id = ?2", params![error, id])?;
        Ok(())
    }

    pub fn replace_snapshot(&self, id: &str, rels: &[String]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db");
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM files WHERE job_id = ?1", [id])?;
        {
            let mut insert = tx.prepare_cached("INSERT INTO files (job_id, rel_path, state, attempts) VALUES (?1, ?2, 'pending', 0)")?;
            for rel in rels {
                insert.execute(params![id, rel])?;
            }
        }
        tx.commit()
    }

    pub fn add_missing_files(&self, id: &str, rels: &[String]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db");
        let tx = conn.unchecked_transaction()?;
        {
            let mut insert = tx.prepare_cached("INSERT OR IGNORE INTO files (job_id, rel_path, state, attempts) VALUES (?1, ?2, 'pending', 0)")?;
            for rel in rels {
                insert.execute(params![id, rel])?;
            }
        }
        tx.commit()
    }

    pub fn has_open_work(&self, id: &str, sort: bool) -> rusqlite::Result<bool> {
        let sql = if sort {
            "SELECT COUNT(*) FROM files WHERE job_id = ?1 AND state NOT IN ('arranged', 'failed')"
        } else {
            "SELECT COUNT(*) FROM files WHERE job_id = ?1 AND state NOT IN ('verified', 'arranged', 'failed')"
        };
        let count: u32 = self.conn.lock().expect("db").query_row(sql, [id], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// A live job runs for days, so a file that used up its attempts (disk unplugged, locked file) is tried again.
    pub fn retry_failed(&self, id: &str) -> rusqlite::Result<usize> {
        self.conn.lock().expect("db").execute(
            "UPDATE files SET state = 'pending', attempts = 0, error = NULL WHERE job_id = ?1 AND state = 'failed'",
            [id],
        )
    }

    pub fn file_count(&self, id: &str) -> rusqlite::Result<u32> {
        self.conn.lock().expect("db").query_row("SELECT COUNT(*) FROM files WHERE job_id = ?1", [id], |row| row.get(0))
    }

    pub fn progress(&self, id: &str) -> rusqlite::Result<(u32, u32)> {
        let conn = self.conn.lock().expect("db");
        let total: u32 = conn.query_row("SELECT COUNT(*) FROM files WHERE job_id = ?1", [id], |row| row.get(0))?;
        let done: u32 = conn.query_row(
            "SELECT COUNT(*) FROM files WHERE job_id = ?1 AND state IN ('verified', 'arranged', 'failed')",
            [id],
            |row| row.get(0),
        )?;
        Ok((done, total))
    }

    /// Files placed in a month folder. Failed copies stay out of this count.
    pub fn arranged(&self, id: &str) -> rusqlite::Result<u32> {
        self.conn.lock().expect("db").query_row(
            "SELECT COUNT(*) FROM files WHERE job_id = ?1 AND state = 'arranged'",
            [id],
            |row| row.get(0),
        )
    }

    pub fn claim(&self, id: &str, from: &[&str], to: &str) -> rusqlite::Result<Option<FileRow>> {
        let conn = self.conn.lock().expect("db");
        let placeholders = from.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT rel_path, state, attempts, error, dest_rel FROM files
             WHERE job_id = ?1 AND state IN ({placeholders}) AND attempts < 3
             ORDER BY rel_path LIMIT 1"
        );
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&id];
        for state in from {
            values.push(state);
        }
        let row = conn.query_row(&sql, values.as_slice(), map_file).optional()?;
        let Some(row) = row else {
            return Ok(None);
        };
        conn.execute(
            "UPDATE files SET state = ?1 WHERE job_id = ?2 AND rel_path = ?3",
            params![to, id, row.rel_path],
        )?;
        Ok(Some(row))
    }

    pub fn set_file(
        &self,
        id: &str,
        rel: &str,
        state: &str,
        attempts: u32,
        error: Option<&str>,
        dest_rel: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute(
            "UPDATE files SET state = ?1, attempts = ?2, error = ?3, dest_rel = COALESCE(?4, dest_rel) WHERE job_id = ?5 AND rel_path = ?6",
            params![state, attempts, error, dest_rel, id, rel],
        )?;
        Ok(())
    }

    pub fn errors(&self, id: &str) -> rusqlite::Result<Vec<FileError>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare(
            "SELECT rel_path, attempts, error FROM files WHERE job_id = ?1 AND state = 'failed' ORDER BY rel_path",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(FileError {
                rel_path: row.get(0)?,
                attempts: row.get(1)?,
                error: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn snapshot_paths(&self, id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare("SELECT rel_path FROM files WHERE job_id = ?1 ORDER BY rel_path")?;
        let rows = stmt.query_map([id], |row| row.get(0))?;
        rows.collect()
    }

    pub fn record(&self, dest_root: &str, rel_path: &str, job_id: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute(
            "INSERT INTO recorded (dest_root, rel_path, job_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(dest_root, rel_path) DO UPDATE SET job_id = excluded.job_id",
            params![dest_root, rel_path, job_id],
        )?;
        Ok(())
    }

    /// (rel_path, dest_rel) of files the database believes are in the archive.
    pub fn filed(&self, id: &str) -> rusqlite::Result<Vec<(String, String)>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare(
            "SELECT rel_path, COALESCE(dest_rel, rel_path) FROM files WHERE job_id = ?1 AND state IN ('verified', 'arranged')",
        )?;
        let rows = stmt.query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn drop_file(&self, id: &str, rel: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute("DELETE FROM files WHERE job_id = ?1 AND rel_path = ?2", params![id, rel])?;
        Ok(())
    }

    /// Put a file back in the queue so it is copied again.
    pub fn requeue(&self, id: &str, rel: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute(
            "UPDATE files SET state = 'pending', attempts = 0, error = NULL, dest_rel = NULL WHERE job_id = ?1 AND rel_path = ?2",
            params![id, rel],
        )?;
        Ok(())
    }

    pub fn forget(&self, dest_root: &str, rel_path: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute(
            "DELETE FROM recorded WHERE dest_root = ?1 AND rel_path = ?2",
            params![dest_root, rel_path],
        )?;
        Ok(())
    }

    pub fn recorded_before(&self, dest_root: &str, job_id: &str) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare(
            "SELECT rel_path FROM recorded WHERE dest_root = ?1 AND job_id != ?2 ORDER BY rel_path",
        )?;
        let rows = stmt.query_map(params![dest_root, job_id], |row| row.get(0))?;
        rows.collect()
    }

    pub fn copying(&self) -> rusqlite::Result<Vec<(String, String, String)>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare(
            "SELECT f.job_id, j.dest, COALESCE(f.dest_rel, f.rel_path) FROM files f JOIN jobs j ON j.id = f.job_id WHERE f.state = 'copying'",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    }

    pub fn reset_interrupted_files(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db");
        conn.execute("UPDATE files SET state = 'pending' WHERE state = 'copying'", [])?;
        conn.execute("UPDATE files SET state = 'copied' WHERE state = 'verifying'", [])?;
        conn.execute("UPDATE files SET state = 'verified' WHERE state = 'arranging'", [])?;
        Ok(())
    }

    pub fn running_job_ids(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare("SELECT id FROM jobs WHERE state = 'running' ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    pub fn replace_review(&self, id: &str, items: &[ReviewItem]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db");
        conn.execute("DELETE FROM review_items WHERE job_id = ?1", [id])?;
        for item in items {
            conn.execute(
                "INSERT INTO review_items (job_id, kind, keep_path, remove_path) VALUES (?1, ?2, ?3, ?4)",
                params![id, item.kind, item.keep_path, item.remove_path],
            )?;
        }
        Ok(())
    }

    pub fn review_items(&self, id: &str) -> rusqlite::Result<Vec<ReviewItem>> {
        let conn = self.conn.lock().expect("db");
        let mut stmt = conn.prepare(
            "SELECT kind, keep_path, remove_path FROM review_items WHERE job_id = ?1 ORDER BY remove_path",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(ReviewItem {
                kind: row.get(0)?,
                keep_path: row.get(1)?,
                remove_path: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn clear_review(&self, id: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute("DELETE FROM review_items WHERE job_id = ?1", [id])?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn.lock().expect("db").query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0)).optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.lock().expect("db").execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn worker_count(&self) -> u32 {
        self.setting("workers")
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(20)
            .clamp(1, 20)
    }

    pub fn start_at_login(&self) -> bool {
        self.setting("login").ok().flatten().map(|value| value != "false").unwrap_or(true)
    }

    pub fn restart_on_crash(&self) -> bool {
        self.setting("restart_on_crash").ok().flatten().map(|value| value != "false").unwrap_or(true)
    }

    pub fn icloudpd_command(&self) -> String {
        self.setting("icloudpd")
            .ok()
            .flatten()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "icloudpd".into())
    }
}

fn ensure_job_columns(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(jobs)")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if !names.iter().any(|name| name == "icloud") {
        conn.execute("ALTER TABLE jobs ADD COLUMN icloud INTEGER NOT NULL DEFAULT 0", [])?;
    }
    if !names.iter().any(|name| name == "apple_id") {
        conn.execute("ALTER TABLE jobs ADD COLUMN apple_id TEXT NOT NULL DEFAULT ''", [])?;
    }
    if !names.iter().any(|name| name == "watch_secs") {
        conn.execute("ALTER TABLE jobs ADD COLUMN watch_secs INTEGER NOT NULL DEFAULT 3600", [])?;
    }
    Ok(())
}

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
    Ok(JobRecord {
        id: row.get(0)?,
        source: row.get(1)?,
        dest: row.get(2)?,
        recursive: row.get::<_, i64>(3)? != 0,
        modified_since: row.get(4)?,
        skip_junk: row.get::<_, i64>(5)? != 0,
        sort_into_months: row.get::<_, i64>(6)? != 0,
        review_duplicates: row.get::<_, i64>(7)? != 0,
        remove_missing: row.get::<_, i64>(8)? != 0,
        state: row.get(9)?,
        stage: row.get(10)?,
        workers: row.get::<_, i64>(11)? as u32,
        current_file: row.get(12)?,
        error: row.get(13)?,
        icloud: row.get::<_, i64>(14)? != 0,
        apple_id: row.get(15)?,
        watch_secs: row.get::<_, i64>(16)? as u32,
    })
}

fn map_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileRow> {
    Ok(FileRow {
        rel_path: row.get(0)?,
        state: row.get(1)?,
        attempts: row.get(2)?,
        error: row.get(3)?,
        dest_rel: row.get(4)?,
    })
}
