use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sync_icloud_engine::{
    command_args, recover_interrupted, resolve_command, resolve_review, run_job, signal_interrupt, FileError, Flag, IcloudStatus,
    JobRecord, NewJob,
    ReviewItem, Store,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, State, WindowEvent};
#[cfg(not(target_os = "macos"))]
use tauri_plugin_dialog::DialogExt;

static PICKING_FOLDER: AtomicBool = AtomicBool::new(false);

struct AppState {
    store: Arc<Store>,
    flags: Mutex<HashMap<String, Arc<Flag>>>,
    gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pids: Mutex<HashMap<String, u32>>,
}

struct IcloudChild {
    pid: u32,
    done: Receiver<std::process::ExitStatus>,
}

#[derive(Serialize)]
struct JobListItem {
    #[serde(flatten)]
    job: JobRecord,
    done: u32,
    total: u32,
    arranged: u32,
    icloud_status: Option<IcloudStatus>,
}

#[derive(Serialize)]
struct JobView {
    job: JobRecord,
    done: u32,
    total: u32,
    arranged: u32,
    bytes_done: u64,
    bytes_total: u64,
    errors: Vec<FileError>,
    review: Vec<ReviewItem>,
    icloud: Option<IcloudStatus>,
}

#[derive(Serialize)]
struct SettingsView {
    login: bool,
    workers: u32,
    icloudpd: String,
}

fn db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Application Support/Sync iCloud Photos/jobs.db")
}

fn launch_agent_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/LaunchAgents/photos.sync.icloud.plist")
}

fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn apply_login(enabled: bool) {
    let path = launch_agent_path();
    if !enabled {
        let _ = fs::remove_file(&path);
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>photos.sync.icloud</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#,
        xml_escape(&exe.display().to_string())
    );
    let _ = fs::write(&path, body);
}

fn spawn(app: Arc<AppState>, id: String) {
    let gate = {
        let mut gates = app.gates.lock().expect("gates");
        gates.entry(id.clone()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = gate.lock().expect("job gate");
        let flag = {
            let mut flags = app.flags.lock().expect("flags");
            flags.entry(id.clone()).or_insert_with(|| Arc::new(Flag::default())).clone()
        };
        let job = app.store.get_job(&id).ok().flatten();
        let child = job.filter(|job| job.icloud).and_then(|job| start_icloud(&app, &id, &job, flag.clone()));
        loop {
            let _ = run_job(&app.store, &id, &flag);
            let state = app.store.get_job(&id).ok().flatten().map(|job| job.state).unwrap_or_default();
            if state == "needs_review" {
                while !flag.is_halted() {
                    let still = app.store.get_job(&id).ok().flatten().map(|job| job.state).unwrap_or_default();
                    if still != "needs_review" {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
                if flag.is_halted() {
                    break;
                }
                continue;
            }
            break;
        }
        if let Some(child) = child {
            flag.mark_stopping();
            interrupt(child.pid);
            let _ = child.done.recv_timeout(Duration::from_secs(8));
        }
        app.pids.lock().expect("pids").remove(&id);
        app.flags.lock().expect("flags").remove(&id);
    });
}

fn start_icloud(app: &AppState, id: &str, job: &JobRecord, flag: Arc<Flag>) -> Option<IcloudChild> {
    let binary = resolve_command(&app.store.icloudpd_command());
    let args = command_args(&job.source, &job.apple_id, job.watch_secs);
    let mut command = std::process::Command::new(&binary);
    command
        .args(&args)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Own process group, so pause and stop can signal the launcher and the downloader it spawns.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let message = format!("icloudpd failed to start ({err}). Install it, or set the command path in Settings.");
            flag.observe_icloud_line(&format!("2000-01-01 00:00:00 ERROR    {message}"));
            let _ = app.store.set_error(id, Some(&message));
            return None;
        }
    };
    let pid = child.id();
    app.pids.lock().expect("pids").insert(id.to_string(), pid);
    if let Some(output) = child.stdout.take() {
        let flag = flag.clone();
        let store = app.store.clone();
        let job_id = id.to_string();
        std::thread::spawn(move || pump(output, flag, store, job_id));
    }
    if let Some(output) = child.stderr.take() {
        let flag = flag.clone();
        let store = app.store.clone();
        let job_id = id.to_string();
        std::thread::spawn(move || pump(output, flag, store, job_id));
    }
    let (sender, done) = mpsc::channel();
    let flag = flag.clone();
    std::thread::spawn(move || {
        let status = child.wait();
        let code = status.as_ref().ok().and_then(|status| status.code());
        flag.observe_icloud_exit(code);
        if let Ok(status) = status {
            let _ = sender.send(status);
        }
    });
    Some(IcloudChild { pid, done })
}

fn pump(reader: impl std::io::Read, flag: Arc<Flag>, store: Arc<Store>, id: String) {
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else {
            break;
        };
        if line.contains("Downloading ") && line.contains(" to ") {
            let _ = store.set_error(&id, None);
        }
        flag.observe_icloud_line(&line);
    }
}

fn interrupt(pid: u32) {
    signal_interrupt(pid);
}

fn stop_children(state: &AppState) {
    if let Ok(flags) = state.flags.lock() {
        for flag in flags.values() {
            flag.mark_stopping();
            flag.request_stop();
        }
    }
    if let Ok(pids) = state.pids.lock() {
        for pid in pids.values() {
            interrupt(*pid);
        }
    }
}

fn view(state: &AppState, id: &str) -> Result<JobView, String> {
    let job = state.store.get_job(id).map_err(|err| err.to_string())?.ok_or_else(|| "job not found".to_string())?;
    let (done, total) = state.store.progress(id).map_err(|err| err.to_string())?;
    let arranged = state.store.arranged(id).map_err(|err| err.to_string())?;
    let errors = state.store.errors(id).map_err(|err| err.to_string())?;
    let review = state.store.review_items(id).map_err(|err| err.to_string())?;
    let (bytes_done, bytes_total) = byte_progress(state, id, job.current_file.as_deref());
    let icloud = if job.icloud {
        state.flags.lock().ok().and_then(|flags| flags.get(id).map(|flag| flag.icloud_status()))
    } else {
        None
    };
    Ok(JobView { job, done, total, arranged, bytes_done, bytes_total, errors, review, icloud })
}

fn byte_progress(state: &AppState, id: &str, current: Option<&str>) -> (u64, u64) {
    let Ok(flags) = state.flags.lock() else {
        return (0, 0);
    };
    let Some(flag) = flags.get(id) else {
        return (0, 0);
    };
    current.and_then(|name| flag.file_progress(name)).unwrap_or((0, 0))
}

#[tauri::command]
fn list_jobs(state: State<'_, Arc<AppState>>) -> Result<Vec<JobListItem>, String> {
    let jobs = state.store.list_jobs().map_err(|err| err.to_string())?;
    jobs.into_iter()
        .map(|job| {
            let (done, total) = state.store.progress(&job.id).map_err(|err| err.to_string())?;
            let arranged = state.store.arranged(&job.id).map_err(|err| err.to_string())?;
            let icloud_status = if job.icloud {
                state.flags.lock().ok().and_then(|flags| flags.get(&job.id).map(|flag| flag.icloud_status()))
            } else {
                None
            };
            Ok(JobListItem { job, done, total, arranged, icloud_status })
        })
        .collect()
}

#[tauri::command]
fn job_detail(state: State<'_, Arc<AppState>>, id: String) -> Result<JobView, String> {
    view(&state, &id)
}

#[tauri::command]
fn create_job(
    state: State<'_, Arc<AppState>>,
    source: String,
    dest: String,
    recursive: bool,
    modified_since: Option<String>,
    skip_junk: bool,
    sort_into_months: bool,
    review_duplicates: bool,
    remove_missing: bool,
    icloud: bool,
    apple_id: String,
    watch_secs: u32,
) -> Result<String, String> {
    if source.trim().is_empty() || dest.trim().is_empty() {
        return Err("Choose a download folder and a destination folder.".into());
    }
    if Path::new(&source) == Path::new(&dest) {
        return Err("Source and destination must be different folders.".into());
    }
    if icloud && apple_id.trim().is_empty() {
        return Err("Enter the Apple ID icloudpd should use.".into());
    }
    state
        .store
        .create_job(NewJob {
            source,
            dest,
            recursive: if icloud { true } else { recursive },
            modified_since: modified_since.filter(|day| !day.is_empty()),
            skip_junk,
            sort_into_months,
            review_duplicates,
            remove_missing: remove_missing && !sort_into_months,
            workers: state.store.worker_count(),
            icloud,
            apple_id: if icloud { apple_id.trim().to_string() } else { String::new() },
            watch_secs,
        })
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn start_job(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    ensure_idle(&state, Some(&id))?;
    let job = state.store.get_job(&id).map_err(|err| err.to_string())?.ok_or_else(|| "job not found".to_string())?;
    if !matches!(job.state.as_str(), "draft" | "paused" | "stopped") {
        return Err("This job cannot be started from its current state.".into());
    }
    state.store.set_state(&id, "running").map_err(|err| err.to_string())?;
    spawn(state.inner().clone(), id);
    Ok(())
}

#[tauri::command]
fn pause_job(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    signal(&state, &id, false)
}

#[tauri::command]
fn stop_job(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    signal(&state, &id, true)
}

fn signal(state: &AppState, id: &str, stop: bool) -> Result<(), String> {
    let Some(job) = state.store.get_job(id).map_err(|err| err.to_string())? else {
        return Ok(());
    };
    if job.state != "running" && !(job.icloud && job.state == "needs_review") {
        return Ok(());
    }
    let flag = {
        let mut flags = state.flags.lock().expect("flags");
        flags.entry(id.to_string()).or_insert_with(|| Arc::new(Flag::default())).clone()
    };
    if let Some(pid) = state.pids.lock().ok().and_then(|pids| pids.get(id).copied()) {
        flag.mark_stopping();
        interrupt(pid);
    }
    if stop {
        flag.request_stop();
        if job.state == "needs_review" {
            state.store.set_state(id, "stopped").map_err(|err| err.to_string())?;
        }
    } else {
        flag.request_pause();
    }
    Ok(())
}

#[tauri::command]
fn resume_job(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    ensure_idle(&state, Some(&id))?;
    let job = state.store.get_job(&id).map_err(|err| err.to_string())?.ok_or_else(|| "job not found".to_string())?;
    if !matches!(job.state.as_str(), "paused" | "stopped") {
        return Err("This job is not paused or stopped.".into());
    }
    state.store.set_state(&id, "running").map_err(|err| err.to_string())?;
    spawn(state.inner().clone(), id);
    Ok(())
}

#[tauri::command]
fn confirm_review(state: State<'_, Arc<AppState>>, id: String, confirm: bool) -> Result<(), String> {
    resolve_review(&state.store, &id, confirm).map_err(|err| err.to_string())?;
    let job = state.store.get_job(&id).map_err(|err| err.to_string())?.ok_or_else(|| "job not found".to_string())?;
    if job.state == "running" {
        let alive = state.flags.lock().expect("flags").contains_key(&id);
        if !alive {
            spawn(state.inner().clone(), id);
        }
    }
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<'_, Arc<AppState>>) -> SettingsView {
    SettingsView {
        login: state.store.start_at_login(),
        workers: state.store.worker_count(),
        icloudpd: state.store.icloudpd_command(),
    }
}

#[tauri::command]
fn set_settings(state: State<'_, Arc<AppState>>, login: bool, workers: u32, icloudpd: String) -> Result<(), String> {
    let workers = workers.clamp(1, 20);
    let command = icloudpd.trim();
    if command.is_empty() {
        return Err("Enter the icloudpd command.".into());
    }
    state.store.set_setting("login", if login { "true" } else { "false" }).map_err(|err| err.to_string())?;
    state.store.set_setting("workers", &workers.to_string()).map_err(|err| err.to_string())?;
    state.store.set_setting("icloudpd", command).map_err(|err| err.to_string())?;
    apply_login(login);
    Ok(())
}

#[tauri::command]
fn open_signin() {
    let _ = std::process::Command::new("open").arg("http://127.0.0.1:8080").spawn();
}

#[tauri::command]
fn pick_folder(app: AppHandle) -> Option<String> {
    PICKING_FOLDER.store(true, Ordering::SeqCst);
    let picked = pick_folder_inner(&app);
    PICKING_FOLDER.store(false, Ordering::SeqCst);
    picked
}

/// The in-process folder panel crashes this bundled app when the user cancels it.
/// `choose folder` runs in its own process, so cancel only dismisses the dialog.
#[cfg(target_os = "macos")]
fn pick_folder_inner(_app: &AppHandle) -> Option<String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg("POSIX path of (choose folder with prompt \"Choose a folder\")")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    let path = if path.len() > 1 { path.trim_end_matches('/') } else { path };
    if path.is_empty() { None } else { Some(path.to_string()) }
}

#[cfg(not(target_os = "macos"))]
fn pick_folder_inner(app: &AppHandle) -> Option<String> {
    let picked = app.dialog().file().blocking_pick_folder()?;
    picked.into_path().ok().map(|path| path.display().to_string())
}

fn ensure_idle(state: &AppState, except: Option<&str>) -> Result<(), String> {
    let jobs = state.store.list_jobs().map_err(|err| err.to_string())?;
    if jobs.iter().any(|job| {
        except != Some(job.id.as_str()) && (job.state == "running" || (job.icloud && job.state == "needs_review"))
    }) {
        return Err("Another job is already running.".into());
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let store = Arc::new(Store::open(&db_path()).map_err(|err| err.to_string())?);
            let state = Arc::new(AppState {
                store: store.clone(),
                flags: Mutex::new(HashMap::new()),
                gates: Mutex::new(HashMap::new()),
                pids: Mutex::new(HashMap::new()),
            });
            app.manage(state.clone());
            recover_interrupted(&store).ok();
            if let Ok(ids) = store.running_job_ids() {
                for id in ids {
                    spawn(state.clone(), id);
                }
            }
            if store.start_at_login() {
                apply_login(true);
            }
            let status = MenuItem::with_id(app, "status", "Idle", false, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            let pause = MenuItem::with_id(app, "pause", "Pause", true, None::<&str>)?;
            let resume = MenuItem::with_id(app, "resume", "Resume", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&status, &pause, &resume, &open, &quit])?;
            let handle = app.handle().clone();
            let tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Sync iCloud Photos")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "quit" => {
                        if let Some(state) = app.try_state::<Arc<AppState>>() {
                            stop_children(state.inner());
                        }
                        app.exit(0);
                    }
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "pause" => toggle_pause(&handle, true),
                    "resume" => toggle_pause(&handle, false),
                    _ => {}
                })
                .build(app)?;
            watch_tray(state.clone(), status, tray);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if !PICKING_FOLDER.load(Ordering::SeqCst) {
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_jobs,
            job_detail,
            create_job,
            start_job,
            pause_job,
            stop_job,
            resume_job,
            confirm_review,
            get_settings,
            set_settings,
            pick_folder,
            open_signin
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Sync iCloud Photos");
}

fn watch_tray(state: Arc<AppState>, status: tauri::menu::MenuItem<tauri::Wry>, tray: tauri::tray::TrayIcon) {
    std::thread::spawn(move || {
        let mut previous = String::new();
        loop {
            let line = status_line(&state);
            if line != previous {
                let _ = status.set_text(line.replace('&', "&&"));
                let tip = if line == "Idle" { "Sync iCloud Photos".to_string() } else { line.clone() };
                let _ = tray.set_tooltip(Some(tip));
                previous = line;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

fn status_line(state: &AppState) -> String {
    let Ok(jobs) = state.store.list_jobs() else {
        return "Idle".into();
    };
    if let Some(job) = jobs.iter().find(|job| job.state == "running") {
        let (done, total) = state.store.progress(&job.id).unwrap_or((0, 0));
        if job.icloud {
            let arranged = state.store.arranged(&job.id).unwrap_or(0);
            let local = if job.sort_into_months {
                format!("Arranged {arranged}/{total}")
            } else {
                format!("Copied {done}/{total}")
            };
            let sync = state
                .flags
                .lock()
                .ok()
                .and_then(|flags| flags.get(&job.id).map(|flag| flag.icloud_status()))
                .map(|status| match status.library_total {
                    Some(library) => format!("Syncing {}/{library}", status.checked),
                    None => format!("Syncing {}", status.checked),
                })
                .unwrap_or_else(|| "Syncing".into());
            return format!("{sync} · {local}");
        }
        let stage = match job.stage.as_str() {
            "scan" => "Scanning",
            "copy" => "Copying",
            "verify" => "Verifying",
            "arrange" => "Sorting",
            "sync" => "Syncing",
            _ => "Working",
        };
        let name = job.current_file.as_deref().unwrap_or("");
        if name.is_empty() {
            return format!("{stage} {done}/{total}");
        }
        return format!("{stage} {done}/{total} · {name}");
    }
    if jobs.iter().any(|job| job.state == "needs_review") {
        return "Waiting for review".into();
    }
    if let Some(job) = jobs.iter().find(|job| job.state == "paused" || job.state == "stopped") {
        return if job.state == "stopped" { "Stopped".into() } else { "Paused".into() };
    }
    "Idle".into()
}

fn toggle_pause(app: &AppHandle, pause: bool) {
    let state = app.state::<Arc<AppState>>();
    let Ok(jobs) = state.store.list_jobs() else {
        return;
    };
    if pause {
        for job in jobs.iter().filter(|job| job.state == "running") {
            let _ = signal(&state, &job.id, false);
        }
        return;
    }
    if jobs.iter().any(|job| job.state == "running") {
        return;
    }
    if let Some(job) = jobs.into_iter().find(|job| job.state == "paused" || job.state == "stopped") {
        let _ = state.store.set_state(&job.id, "running");
        spawn(state.inner().clone(), job.id);
    }
}
