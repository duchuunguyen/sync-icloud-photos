use serde::Serialize;

/// Counts read from icloudpd's stdout. The library total is items. Checked counts log lines.
#[derive(Clone, Debug)]
pub struct IcloudProgress {
    pub library_total: Option<u64>,
    pub checked: u64,
    pub saved: u64,
    pub already: u64,
    pub skipped: u64,
    pub eta_seconds: Option<u64>,
    pub phase: String,
    pub message: String,
    pub pass_complete: bool,
    pub failed: bool,
    pub needs_mfa: bool,
    pub waiting_since_ms: Option<u64>,
    /// Last lines icloudpd printed, newest last.
    log: Vec<String>,
    first_ms: Option<u64>,
    saw_complete: bool,
    saw_error_this_pass: bool,
}

impl Default for IcloudProgress {
    fn default() -> Self {
        Self {
            library_total: None,
            checked: 0,
            saved: 0,
            already: 0,
            skipped: 0,
            eta_seconds: None,
            phase: "starting".into(),
            message: String::new(),
            pass_complete: false,
            failed: false,
            needs_mfa: false,
            waiting_since_ms: None,
            log: Vec::new(),
            first_ms: None,
            saw_complete: false,
            saw_error_this_pass: false,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct IcloudStatus {
    pub library_total: Option<u64>,
    pub checked: u64,
    pub saved: u64,
    pub already: u64,
    pub skipped: u64,
    pub eta_seconds: Option<u64>,
    pub phase: String,
    pub message: String,
    pub needs_mfa: bool,
    pub failed: bool,
    pub waiting_since_ms: Option<u64>,
    pub log: Vec<String>,
}

impl IcloudProgress {
    pub fn status(&self) -> IcloudStatus {
        IcloudStatus {
            library_total: self.library_total,
            checked: self.checked,
            saved: self.saved,
            already: self.already,
            skipped: self.skipped,
            eta_seconds: self.eta_seconds,
            phase: self.phase.clone(),
            message: self.message.clone(),
            needs_mfa: self.needs_mfa,
            failed: self.failed,
            waiting_since_ms: self.waiting_since_ms,
            log: self.log.clone(),
        }
    }

    pub fn clear_pass(&mut self) {
        self.pass_complete = false;
    }
}

const LOG_LINES: usize = 100;

/// Arguments for one watch-mode run. `--auto-delete` is never included.
pub fn command_args(directory: &str, apple_id: &str, watch_secs: u32) -> Vec<String> {
    let seconds = watch_secs.max(60);
    vec![
        "--directory".into(),
        directory.into(),
        "--username".into(),
        apple_id.into(),
        "--password-provider".into(),
        "keyring".into(),
        "--password-provider".into(),
        "webui".into(),
        "--mfa-provider".into(),
        "webui".into(),
        "--no-progress-bar".into(),
        "--watch-with-interval".into(),
        seconds.to_string(),
    ]
}

pub fn note_line(progress: &mut IcloudProgress, line: &str, now_ms: u64) {
    if !line.trim().is_empty() {
        if progress.log.len() >= LOG_LINES {
            progress.log.remove(0);
        }
        progress.log.push(line.trim().to_string());
    }
    let (level, message) = split_log(line);
    if level == "ERROR" {
        progress.saw_error_this_pass = true;
        progress.failed = true;
        progress.pass_complete = false;
        progress.phase = "error".into();
        progress.message = message.to_string();
    }
    if message.contains("Two-factor authentication is required") || message.contains("Two-step authentication is required") {
        progress.needs_mfa = true;
        progress.phase = "needs_mfa".into();
        progress.message = "Apple needs a sign-in code.".into();
    }
    if let Some(total) = parse_downloading_total(message) {
        progress.library_total = total;
        progress.checked = 0;
        progress.saved = 0;
        progress.already = 0;
        progress.skipped = 0;
        progress.eta_seconds = None;
        progress.first_ms = None;
        progress.saw_complete = false;
        progress.saw_error_this_pass = false;
        progress.pass_complete = false;
        progress.failed = false;
        progress.needs_mfa = false;
        progress.phase = "downloading".into();
        progress.message = message.to_string();
        return;
    }
    if message.starts_with("Downloaded ") {
        progress.saved += 1;
        progress.checked += 1;
        bump_eta(progress, now_ms);
    } else if message.ends_with(" already exists") {
        progress.already += 1;
        progress.checked += 1;
        bump_eta(progress, now_ms);
    } else if message.starts_with("Skipping ") {
        progress.skipped += 1;
        progress.checked += 1;
        bump_eta(progress, now_ms);
    }
    if (message.contains("All photos") || message.contains("All videos")) && message.contains("have been downloaded") {
        progress.saw_complete = true;
        progress.message = message.to_string();
    }
    if message.contains("Iteration was cancelled") {
        progress.saw_complete = false;
        progress.pass_complete = false;
        progress.message = message.to_string();
    }
    if message.contains("Waiting") && message.contains("sec") {
        progress.phase = "waiting".into();
        progress.waiting_since_ms = Some(now_ms);
        if !progress.failed {
            progress.message = message.to_string();
        }
        if progress.saw_complete && !progress.saw_error_this_pass {
            progress.pass_complete = true;
            progress.failed = false;
        }
    }
}

pub fn note_exit(progress: &mut IcloudProgress, code: Option<i32>, stopping: bool) {
    if stopping {
        return;
    }
    if code == Some(0) {
        if progress.saw_complete && !progress.saw_error_this_pass {
            progress.pass_complete = true;
            progress.phase = "waiting".into();
            progress.failed = false;
        }
        return;
    }
    progress.failed = true;
    progress.pass_complete = false;
    progress.phase = "error".into();
    progress.message = match code {
        Some(code) => format!("icloudpd exited ({code})"),
        None => "icloudpd exited".into(),
    };
}

fn bump_eta(progress: &mut IcloudProgress, now_ms: u64) {
    if progress.first_ms.is_none() {
        progress.first_ms = Some(now_ms);
    }
    let Some(total) = progress.library_total else {
        progress.eta_seconds = None;
        return;
    };
    let Some(start) = progress.first_ms else {
        return;
    };
    let elapsed = now_ms.saturating_sub(start);
    if elapsed < 1000 || progress.checked == 0 || progress.checked >= total {
        progress.eta_seconds = None;
        return;
    }
    let remain = total - progress.checked;
    progress.eta_seconds = Some(remain.saturating_mul(elapsed) / progress.checked / 1000);
}

fn parse_downloading_total(message: &str) -> Option<Option<u64>> {
    let rest = message.strip_prefix("Downloading ")?;
    let token = rest.split_whitespace().next().unwrap_or("");
    if token == "???" {
        return Some(None);
    }
    if token == "the" {
        return Some(Some(1));
    }
    token.parse::<u64>().ok().map(Some)
}

fn split_log(line: &str) -> (&str, &str) {
    let line = line.trim();
    let bytes = line.as_bytes();
    let looks_like_log = bytes.len() > 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b' ')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && bytes.get(19) == Some(&b' ');
    if !looks_like_log {
        return ("INFO", line);
    }
    let rest = line[20..].trim_start();
    let Some((level, message)) = rest.split_once(' ') else {
        return ("INFO", line);
    };
    if matches!(level, "DEBUG" | "INFO" | "WARNING" | "WARN" | "ERROR") {
        return (level, message.trim());
    }
    ("INFO", line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(lines: &[&str]) -> IcloudProgress {
        let mut progress = IcloudProgress::default();
        for (index, line) in lines.iter().enumerate() {
            note_line(&mut progress, line, index as u64 * 1000);
        }
        progress
    }

    #[test]
    fn counts_library_saved_and_already_and_estimates_time() {
        let progress = feed(&[
            "2026-11-16 12:00:00 INFO     Downloading 10 original photos and videos to /library ...",
            "2026-11-16 12:00:01 DEBUG    IMG_0001.jpg already exists",
            "2026-11-16 12:00:02 INFO     Downloaded IMG_0002.HEIC",
        ]);
        assert_eq!(progress.library_total, Some(10));
        assert_eq!(progress.already, 1);
        assert_eq!(progress.saved, 1);
        assert_eq!(progress.checked, 2);
        assert_eq!(progress.eta_seconds, Some(4));
    }

    #[test]
    fn waiting_after_the_downloaded_line_is_a_quiet_pass() {
        let progress = feed(&[
            "2026-11-16 12:00:00 INFO     Downloading 2 original photos and videos to /library ...",
            "2026-11-16 12:00:01 INFO     All photos and videos have been downloaded",
            "2026-11-16 12:00:02 INFO     Waiting for 3600 sec...",
        ]);
        assert!(progress.pass_complete);
        assert!(!progress.failed);
        assert_eq!(progress.phase, "waiting");
    }

    #[test]
    fn an_error_before_waiting_is_not_a_quiet_pass() {
        let progress = feed(&[
            "2026-11-16 12:00:00 INFO     Downloading 2 original photos and videos to /library ...",
            "2026-11-16 12:00:01 INFO     All photos have been downloaded",
            "2026-11-16 12:00:02 ERROR    iCloud listed only 1 of 2 items in the library",
            "2026-11-16 12:00:03 INFO     Waiting for 3600 sec...",
        ]);
        assert!(!progress.pass_complete);
        assert!(progress.failed);
    }

    #[test]
    fn command_args_leave_auto_delete_off() {
        let args = command_args("/library", "me@icloud.com", 30).join(" ");
        assert!(args.contains("--password-provider keyring --password-provider webui"));
        assert!(args.contains("--mfa-provider webui"));
        assert!(args.contains("--watch-with-interval 60"));
        assert!(!args.contains("--auto-delete"));
    }

    #[test]
    fn unknown_total_and_sign_in() {
        let progress = feed(&[
            "Downloading ??? original photos and videos to /library ...",
            "Two-factor authentication is required (2fa)",
        ]);
        assert_eq!(progress.library_total, None);
        assert!(progress.needs_mfa);
        assert_eq!(progress.phase, "needs_mfa");
    }
}
