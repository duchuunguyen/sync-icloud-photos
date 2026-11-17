import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type Job = {
  id: string;
  source: string;
  dest: string;
  recursive: boolean;
  modified_since: string | null;
  skip_junk: boolean;
  sort_into_months: boolean;
  review_duplicates: boolean;
  remove_missing: boolean;
  state: string;
  stage: string;
  workers: number;
  current_file: string | null;
  error: string | null;
  icloud: boolean;
  apple_id: string;
  watch_secs: number;
};

type IcloudStatus = {
  library_total: number | null;
  checked: number;
  saved: number;
  already: number;
  skipped: number;
  eta_seconds: number | null;
  phase: string;
  message: string;
  needs_mfa: boolean;
  failed: boolean;
  waiting_since_ms: number | null;
  log: string[];
};

type ReviewItem = { kind: string; keep_path: string | null; remove_path: string };
type FileError = { rel_path: string; attempts: number; error: string | null };
type ListedJob = Job & { done: number; total: number; arranged: number; failed: number; icloud_status: IcloudStatus | null };
type JobView = {
  job: Job;
  done: number;
  total: number;
  arranged: number;
  bytes_done: number;
  bytes_total: number;
  errors: FileError[];
  review: ReviewItem[];
  icloud: IcloudStatus | null;
};
type Settings = { login: boolean; restart_on_crash: boolean; workers: number; icloudpd: string };
type Recovery = { phase: string; missing: number; wrong_folder: number };
type Stray = { group: number; command: string };

const app = document.querySelector("#app") as HTMLElement;
let screen: "home" | "new" | "job" | "settings" = "home";
let selected = "";
let notice = "";
let logOpen: boolean | null = null; // set once the user toggles the log; survives re-renders
let confirmDelete = "";
let drawn = "";
type BackupCheck = { source_files: number; dest_files: number; missing: string[]; wrong_folder: [string, string][] };
let check: { id: string; busy: boolean; result?: BackupCheck; error?: string } | null = null;

function baseName(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

function pill(state: string): string {
  if (state === "needs_review") return "wait";
  if (state === "completed") return "done";
  if (state === "completed_with_errors") return "bad";
  return "";
}

function formatEta(seconds: number): string {
  if (seconds < 90) return `about ${Math.max(1, Math.round(seconds))} sec`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 90) return `about ${minutes} min`;
  const hours = seconds / 3600;
  return `about ${hours >= 10 ? hours.toFixed(0) : hours.toFixed(1)} h`;
}

function percent(done: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.round((done / total) * 100));
}

function progressBar(pct: number, arrange: boolean): string {
  return `<div class="progress${arrange ? " arrange" : ""}"><div style="width:${pct}%"></div></div>`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}

async function render(): Promise<void> {
  const jobs = await invoke<ListedJob[]>("list_jobs").catch(() => [] as ListedJob[]);
  const detail = selected ? await invoke<JobView>("job_detail", { id: selected }).catch(() => null) : null;
  const settings = screen === "settings" ? await invoke<Settings>("get_settings") : null;
  const stray = screen === "settings" ? await invoke<Stray[]>("list_stray").catch(() => [] as Stray[]) : [];
  const recovery = await invoke<Recovery | null>("app_status").catch(() => null);
  const html = `
    <div class="shell">
      <nav class="side">
        <button type="button" data-go="home" class="${screen === "settings" ? "" : "on"}">Syncs</button>
        <button type="button" data-go="settings" class="${screen === "settings" ? "on" : ""}">Settings</button>
        <div class="hint">Closing this window keeps syncing in the menu bar. Quit from the menu-bar icon.</div>
      </nav>
      <main>${banner(recovery)}${body(jobs, detail, settings, stray)}</main>
    </div>`;
  if (html === drawn) return;
  drawn = html;
  app.innerHTML = html;
}

// `toggle` does not bubble, so listen in the capture phase.
document.addEventListener("toggle", (event) => {
  const el = event.target as HTMLElement;
  if (el.classList?.contains("log")) logOpen = (el as HTMLDetailsElement).open;
}, true);

function body(jobs: ListedJob[], detail: JobView | null, settings: Settings | null, stray: Stray[]): string {
  if (screen === "settings" && settings) return settingsView(settings, stray);
  if (screen === "new") return newView();
  if (screen === "job" && detail) return jobView(detail);
  return homeView(jobs);
}

type Tone = "ok" | "busy" | "wait" | "off" | "bad";
type Headline = { tone: Tone; title: string; detail: string };

function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function every(secs: number): string {
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  const hours = secs / 3600;
  return `${Number.isInteger(hours) ? hours : hours.toFixed(1)} h`;
}

// One plain-language answer to "what is this sync doing right now?".
function headline(job: Job, done: number, total: number, review: number, icloud: IcloudStatus | null, failed = 0): Headline {
  if (job.state === "draft") return { tone: "off", title: "Not started", detail: "Press Start to begin." };
  if (job.state === "paused" || job.state === "stopped") {
    return { tone: "off", title: "Paused", detail: job.icloud ? "Not checking iCloud. Press Resume to start again." : "Press Resume to continue copying." };
  }
  if (job.state === "needs_review") return { tone: "wait", title: "Waiting for you", detail: `${review} file${review === 1 ? "" : "s"} to review below. Nothing is deleted until you confirm.` };
  if (job.state === "completed") return { tone: "ok", title: "Finished", detail: `${done} files copied.` };
  if (job.state === "completed_with_errors") return { tone: "bad", title: "Finished with problems", detail: "Some files could not be copied. See below." };
  if (job.icloud && (failed > 0 || job.error)) {
    return { tone: "bad", title: failed > 0 ? `${failed} file${failed === 1 ? " is" : "s are"} NOT backed up` : "Cannot read the download folder", detail: `${job.error || "See the list below."} Don't delete these from iCloud yet. The app retries every 10 minutes.` };
  }
  if (!job.icloud) return { tone: "busy", title: "Copying files", detail: `${done} of ${total} files` };
  if (!icloud || icloud.phase === "starting") return { tone: "busy", title: "Connecting to iCloud…", detail: "This can take a minute." };
  if (icloud.needs_mfa) return { tone: "wait", title: "Apple needs a sign-in code", detail: "Open the sign-in page and enter the code Apple sent you." };
  if (icloud.failed) return { tone: "bad", title: "Something went wrong", detail: icloud.message || "icloudpd stopped. Press Pause, then Resume to try again." };
  if (icloud.phase === "downloading") {
    const count = icloud.library_total === null ? `${icloud.checked} checked` : `${icloud.checked} of ${icloud.library_total} checked`;
    const eta = icloud.eta_seconds === null ? "" : ` · ${formatEta(icloud.eta_seconds)} left`;
    return { tone: "busy", title: "Checking iCloud for new photos…", detail: count + eta };
  }
  if (done < total) return { tone: "busy", title: "Filing new photos…", detail: `${done} of ${total} filed` };
  const next = icloud.waiting_since_ms ? `Last checked ${clock(icloud.waiting_since_ms)} · next check about ${clock(icloud.waiting_since_ms + job.watch_secs * 1000)}.` : `Checks iCloud every ${every(job.watch_secs)}.`;
  return { tone: "ok", title: "Up to date", detail: next };
}

function title(job: Job): string {
  return job.icloud ? `iCloud Photos → ${baseName(job.dest)}` : `${baseName(job.source)} → ${baseName(job.dest)}`;
}

function homeView(jobs: ListedJob[]): string {
  const rows = jobs.length
    ? jobs.map((job) => {
        const head = headline(job, job.done, job.total, 0, job.icloud_status, job.failed);
        return `
        <button class="job" type="button" data-open="${job.id}">
          <span class="dot ${head.tone}"></span>
          <b>${escapeHtml(title(job))}</b>
          <span class="status ${head.tone}">${escapeHtml(head.title)}</span>
          <small>${escapeHtml(head.detail)}</small>
        </button>`;
      }).join("")
    : `<div class="empty"><b>Nothing is syncing yet.</b><p>Connect your iCloud Photos once. The app keeps checking for new photos and files them into month folders.</p></div>`;
  return `
    <div class="row">
      <div><h1>Syncs</h1><p class="sub">Each sync keeps a folder up to date. Closing the window does not stop it.</p></div>
      <button class="primary" type="button" data-go="new">New sync</button>
    </div>
    ${rows}`;
}

function newView(): string {
  return `
    <button class="back" type="button" data-go="home">‹ Syncs</button>
    <h1>New sync</h1>
    <p class="sub">Photos land in the download folder, then get filed into month folders in your archive.</p>
    <div id="form-error">${notice ? `<div class="error">${escapeHtml(notice)}</div>` : ""}</div>
    <div class="checks">
      <label><input type="checkbox" id="icloud" checked> Keep syncing from iCloud Photos<small>Uses icloudpd. It checks iCloud again and again, and files anything new. Untick to copy an existing folder once instead.</small></label>
    </div>
    <div id="icloud-fields">
      <label class="field"><span>Apple ID</span><input id="apple" type="text" placeholder="name@icloud.com" autocomplete="off"></label>
      <label class="field"><span>Check iCloud every (seconds)</span><input id="watch" type="number" min="60" value="3600"><small class="hint-inline">3600 = hourly. Your password stays in the macOS keychain. A sign-in code opens in icloudpd’s own page.</small></label>
    </div>
    <label class="field"><span>Download folder</span><div class="picker"><input id="source" readonly placeholder="Choose a folder"><button class="ghost" type="button" id="pick-source">Choose</button></div></label>
    <label class="field"><span>Archive folder (photos end up here)</span><div class="picker"><input id="dest" readonly placeholder="Choose a folder"><button class="ghost" type="button" id="pick-dest">Choose</button></div></label>
    <div class="checks">
      <label><input type="checkbox" id="sort" checked> File photos into YYYY-MM folders<small>Uses the date the photo was taken.</small></label>
      <label><input type="checkbox" id="duplicates"> Ask me about duplicates<small>Shows a list first. Nothing is deleted until you confirm.</small></label>
    </div>
    <details class="more"><summary>More options</summary>
      <div class="checks">
        <label><input type="checkbox" id="recursive" checked disabled> Include subfolders<small>Always on for iCloud. For a plain folder copy, off means only files directly inside it.</small></label>
        <label><input type="checkbox" id="since"> Only files modified on or after <input type="date" id="since-date" disabled></label>
        <label><input type="checkbox" id="junk" checked> Skip .DS_Store and ._* files</label>
        <label><input type="checkbox" id="removal" disabled> Remove archive files that left the source<small>Shows a list first. Only available when not filing into month folders.</small></label>
      </div>
      <div class="note hidden" id="sort-note">Filing into months builds an archive, not a mirror, so removal stays off.</div>
    </details>
    <button class="primary" type="button" id="start">Start</button>`;
}

function checkBlock(id: string): string {
  const mine = check?.id === id ? check : null;
  const button = `<button class="ghost" type="button" id="check"${mine?.busy ? " disabled" : ""}>${mine?.busy ? "Checking…" : "Check now"}</button>`;
  let out = "";
  if (mine?.error) out = `<div class="error">${escapeHtml(mine.error)}</div>`;
  const r = mine?.result;
  if (r) {
    const bad = r.missing.length + r.wrong_folder.length;
    out = `<div class="hero ${bad ? "bad" : "ok"}"><div><b>${bad ? "Do not delete from iCloud yet" : "Every file is in the archive"}</b><p>${r.source_files.toLocaleString()} in the download folder · ${r.dest_files.toLocaleString()} in the archive. Compares names and folders only, not file contents.</p></div></div>`;
    if (r.missing.length) out += `<p class="sub">Missing from the archive (${r.missing.length})</p><ul>${r.missing.slice(0, 200).map((rel) => `<li>${escapeHtml(rel)}</li>`).join("")}</ul>`;
    if (r.wrong_folder.length) out += `<p class="sub">Filed by the date inside the file, which differs from iCloud's date (${r.wrong_folder.length})</p><ul>${r.wrong_folder.slice(0, 200).map(([rel, dirs]) => `<li>${escapeHtml(rel)} → ${escapeHtml(dirs)}</li>`).join("")}</ul>`;
  }
  return `<p class="sub">Confirms every photo in the download folder has a same-named file in the right archive folder. Fast: names only.</p>${button}${out}`;
}

function stat(value: number, text: string): string {
  return `<div class="stat"><b>${value.toLocaleString()}</b><span>${text}</span></div>`;
}

function jobView(detail: JobView): string {
  const { job, done, total, errors, review, bytes_done, bytes_total, icloud } = detail;
  const duplicate = review.filter((item) => item.kind === "duplicate");
  const removal = review.filter((item) => item.kind === "removal");
  const head = headline(job, done, total, review.length, icloud, errors.length);
  const errorBlock = [notice, job.error || ""].filter(Boolean).map((text) => `<div class="error">${escapeHtml(text)}</div>`).join("");
  const fileErrors = errors.length
    ? `<h2>Could not finish</h2><ul>${errors.map((item) => `<li>${escapeHtml(item.rel_path)} — ${escapeHtml(item.error || "failed")} (attempt ${item.attempts})</li>`).join("")}</ul>`
    : "";
  let reviewBlock = "";
  if (job.state === "needs_review" && duplicate.length) {
    const groups = new Map<string, string[]>();
    for (const item of duplicate) {
      const keep = item.keep_path || "";
      groups.set(keep, [...(groups.get(keep) || []), item.remove_path]);
    }
    const rows = [...groups.entries()].map(([keep, remove]) => `<tr><td class="keep">${escapeHtml(keep)}</td><td class="drop">${remove.map(escapeHtml).join("<br>")}</td></tr>`).join("");
    reviewBlock = `<h2>Duplicates</h2><p class="sub">Same name and identical content. Nothing is deleted until you confirm.</p><table><thead><tr><th>Keep</th><th>Remove</th></tr></thead><tbody>${rows}</tbody></table><div class="actions"><button class="primary" type="button" id="confirm">Delete these copies</button><button class="ghost" type="button" id="skip">Keep everything</button></div>`;
  } else if (job.state === "needs_review" && removal.length) {
    const rows = removal.map((item) => `<tr><td>${escapeHtml(item.remove_path)}</td></tr>`).join("");
    reviewBlock = `<h2>Files that left the source</h2><p class="sub">These were copied by an earlier run. Files you added yourself are not listed.</p><table><thead><tr><th>Archive file</th></tr></thead><tbody>${rows}</tbody></table><div class="actions"><button class="primary" type="button" id="confirm">Delete these files</button><button class="ghost" type="button" id="skip">Keep everything</button></div>`;
  }
  const running = job.state === "running";
  const halted = job.state === "paused" || job.state === "stopped";
  const control = running
    ? `<button class="ghost" type="button" id="pause">Pause</button>`
    : halted
      ? `<button class="primary" type="button" id="resume">Resume</button>`
      : job.state === "draft"
        ? `<button class="primary" type="button" id="start-existing">Start</button>`
        : "";
  const remove = running || job.state === "needs_review"
    ? ""
    : `<button class="ghost" type="button" id="delete">${confirmDelete === job.id ? "Click again to delete" : "Delete sync"}</button>`;
  const signin = icloud?.needs_mfa && running ? `<button class="primary" type="button" id="signin">Open sign-in page</button>` : "";
  const now = job.current_file && running
    ? `<p class="now">Filing ${escapeHtml(baseName(job.current_file))}${bytes_total > 0 ? ` · ${formatBytes(bytes_done)} / ${formatBytes(bytes_total)}` : ""}</p>`
    : "";
  const bar = running && (icloud?.phase === "downloading" && icloud.library_total)
    ? progressBar(percent(icloud.checked, icloud.library_total), false)
    : running && !job.icloud && total > 0 ? progressBar(percent(done, total), false) : "";
  const stats = icloud
    ? `<div class="stats">${stat(icloud.saved, "new this check")}${stat(icloud.already, "already had")}${stat(done, "in your archive")}</div>`
    : `<div class="stats">${stat(done, "copied")}${stat(total, "total files")}</div>`;
  const options = [
    job.icloud ? `Checks iCloud every ${every(job.watch_secs)}` : "",
    job.sort_into_months ? "Files into month folders" : "Keeps the source layout",
    job.review_duplicates ? "Asks about duplicates" : "",
    job.remove_missing ? "Asks before removing files that left the source" : "",
  ].filter(Boolean).map((text) => `<li>${escapeHtml(text)}</li>`).join("");
  return `
    <button class="back" type="button" data-go="home">‹ Syncs</button>
    ${errorBlock}
    <h1>${escapeHtml(title(job))}</h1>
    <div class="hero ${head.tone}">
      <div><span class="dot ${head.tone}"></span><b>${escapeHtml(head.title)}</b><p>${escapeHtml(head.detail)}</p></div>
      <div class="actions">${signin}${control}${remove}</div>
    </div>
    ${bar}${now}
    ${reviewBlock}
    ${job.state === "needs_review" ? "" : stats}
    ${fileErrors}
    ${icloud?.log.length ? `<details class="log"${logOpen ?? icloud.failed ? " open" : ""}><summary>icloudpd log</summary><pre>${escapeHtml(icloud.log.join("\n"))}</pre></details>` : ""}
    <h2>Check backup</h2>
    ${checkBlock(job.id)}
    <h2>Folders</h2>
    <div class="folders">
      <div><span>${job.icloud ? "Download folder (landing spot)" : "Source"}</span><code>${escapeHtml(job.source)}</code></div>
      <div><span>Archive</span><code>${escapeHtml(job.dest)}</code></div>
    </div>
    <h2>How it works</h2>
    <ul class="how">${options}</ul>`;
}

function banner(recovery: Recovery | null): string {
  if (!recovery) return "";
  const dismiss = `<button class="ghost" type="button" id="dismiss">Dismiss</button>`;
  if (recovery.phase === "catching_up") return `<div class="hero busy"><div><b>The app closed unexpectedly</b><p>It restarted and is copying everything that arrived in the meantime.</p></div></div>`;
  if (recovery.phase === "ok") return `<div class="hero ok"><div><b>Recovered after an unexpected close</b><p>Checked every download against the archive. Nothing is missing.</p></div>${dismiss}</div>`;
  return `<div class="hero bad"><div><b>Recovered, but ${recovery.missing + recovery.wrong_folder} file${recovery.missing + recovery.wrong_folder === 1 ? "" : "s"} need attention</b><p>${recovery.missing} missing from the archive, ${recovery.wrong_folder} in an unexpected folder. Open the sync and press Check now. Do not delete from iCloud yet.</p></div>${dismiss}</div>`;
}

function settingsView(settings: Settings, stray: Stray[]): string {
  return `
    <h1>Settings</h1>
    ${notice ? `<div class="error">${escapeHtml(notice)}</div>` : ""}
    <p class="sub">This Mac. A later Windows build would use a startup app instead of a login item.</p>
    <div class="setting"><div><b>Start at login</b><p>After a restart, a job that was running continues. A paused or stopped job stays that way.</p></div><input type="checkbox" id="login" ${settings.login ? "checked" : ""}></div>
    <div class="setting"><div><b>Restart after a crash</b><p>If the app crashes, it relaunches within seconds, stops any leftover icloudpd and catches up. Quit from the menu bar never restarts it. Takes effect at the next login.</p></div><input type="checkbox" id="restart" ${settings.restart_on_crash ? "checked" : ""}></div>
    <div class="setting"><div><b>Parallel copies</b><p>The Java sync uses 20. One slow iCloud download occupies a single slot.</p></div><input id="workers" type="number" min="1" max="20" value="${settings.workers}"></div>
    <div class="setting"><div><b>icloudpd command</b><p>The program on PATH, or a full path to the binary. Auto-delete is never turned on.</p></div><input id="icloudpd" type="text" value="${escapeHtml(settings.icloudpd)}"></div>
    <button class="primary" type="button" id="save-settings">Save</button>
    <h2>Stray icloudpd</h2>
    ${stray.length
      ? `<p class="sub">These icloudpd downloads are running but this app is not watching them, so nothing copies their files.</p><ul>${stray.map((item) => `<li><code>${escapeHtml(item.command)}</code></li>`).join("")}</ul><button class="ghost" type="button" id="stop-stray">Stop them</button>`
      : `<p class="sub">None. Every running icloudpd belongs to this app.</p>`}`;
}

app.addEventListener("click", (event) => {
  const node = (event.target as HTMLElement | null)?.closest<HTMLElement>("button");
  if (!node) return;
  if (node.dataset.go) {
    screen = node.dataset.go as typeof screen;
    notice = "";
    drawn = "";
    void render();
    return;
  }
  if (node.dataset.open) {
    selected = node.dataset.open || "";
    screen = "job";
    notice = "";
    drawn = "";
    void render();
    return;
  }
  if (node.id === "pick-source") void choose("#source");
  if (node.id === "pick-dest") void choose("#dest");
  if (node.id === "start") void startNew();
  if (node.id === "pause") void act("pause_job");
  if (node.id === "stop") void act("stop_job");
  if (node.id === "resume") void act("resume_job");
  if (node.id === "delete") {
    if (confirmDelete !== selected) {
      confirmDelete = selected;
      drawn = "";
      void render();
    } else {
      confirmDelete = "";
      void invoke("delete_job", { id: selected }).then(() => { screen = "home"; drawn = ""; return render(); }, (err) => { notice = String(err); drawn = ""; return render(); });
    }
  }
  if (node.id === "start-existing") void act("start_job");
  if (node.id === "confirm") void act("confirm_review", true);
  if (node.id === "skip") void act("confirm_review", false);
  if (node.id === "save-settings") void saveSettings();
  if (node.id === "check") void runCheck();
  if (node.id === "dismiss") void invoke("dismiss_recovery").then(() => { drawn = ""; return render(); });
  if (node.id === "stop-stray") void invoke("stop_stray").then(() => { drawn = ""; return render(); });
  if (node.id === "signin") void invoke("open_signin");
});

app.addEventListener("change", (event) => {
  const target = event.target as HTMLInputElement | null;
  if (!target) return;
  if (target.id === "since") {
    const date = document.querySelector<HTMLInputElement>("#since-date");
    if (date) date.disabled = !target.checked;
  }
  if (target.id === "sort") syncRemoval();
  if (target.id === "icloud") {
    document.querySelector("#icloud-fields")?.classList.toggle("hidden", !target.checked);
    const recursive = document.querySelector<HTMLInputElement>("#recursive");
    const sort = document.querySelector<HTMLInputElement>("#sort");
    if (target.checked) {
      if (recursive) {
        recursive.checked = true;
        recursive.disabled = true;
      }
      if (sort && !sort.checked) sort.checked = true;
    } else if (recursive) {
      recursive.disabled = false;
    }
    syncRemoval();
  }
});

function syncRemoval(): void {
  const sort = document.querySelector<HTMLInputElement>("#sort")?.checked || false;
  const removal = document.querySelector<HTMLInputElement>("#removal");
  document.querySelector("#sort-note")?.classList.toggle("hidden", !sort);
  if (removal) {
    removal.disabled = sort;
    if (sort) removal.checked = false;
  }
}

async function act(command: string, confirm?: boolean): Promise<void> {
  notice = "";
  try {
    await invoke(command, confirm === undefined ? { id: selected } : { id: selected, confirm });
  } catch (err) {
    notice = String(err);
  }
  drawn = "";
  await render();
}

async function runCheck(): Promise<void> {
  check = { id: selected, busy: true };
  drawn = "";
  await render();
  try {
    check = { id: selected, busy: false, result: await invoke<BackupCheck>("check_backup_cmd", { id: selected }) };
  } catch (err) {
    check = { id: selected, busy: false, error: String(err) };
  }
  drawn = "";
  await render();
}

async function choose(selector: string): Promise<void> {
  try {
    const path = await invoke<string | null>("pick_folder");
    const input = document.querySelector<HTMLInputElement>(selector);
    if (path && input) input.value = path;
  } catch {
    // Cancel leaves the field as it was.
  }
}

async function startNew(): Promise<void> {
  const source = document.querySelector<HTMLInputElement>("#source")?.value || "";
  const dest = document.querySelector<HTMLInputElement>("#dest")?.value || "";
  const since = document.querySelector<HTMLInputElement>("#since")?.checked || false;
  const day = document.querySelector<HTMLInputElement>("#since-date")?.value || "";
  const icloud = document.querySelector<HTMLInputElement>("#icloud")?.checked || false;
  const watch = Number(document.querySelector<HTMLInputElement>("#watch")?.value || "3600");
  notice = "";
  let id = "";
  try {
    id = await invoke<string>("create_job", {
      source,
      dest,
      recursive: document.querySelector<HTMLInputElement>("#recursive")?.checked || false,
      modifiedSince: since ? day : null,
      skipJunk: document.querySelector<HTMLInputElement>("#junk")?.checked || false,
      sortIntoMonths: document.querySelector<HTMLInputElement>("#sort")?.checked || false,
      reviewDuplicates: document.querySelector<HTMLInputElement>("#duplicates")?.checked || false,
      removeMissing: document.querySelector<HTMLInputElement>("#removal")?.checked || false,
      icloud,
      appleId: document.querySelector<HTMLInputElement>("#apple")?.value || "",
      watchSecs: Number.isFinite(watch) ? watch : 3600,
    });
  } catch (err) {
    notice = String(err);
    const box = document.querySelector("#form-error");
    if (box) box.innerHTML = `<div class="error">${escapeHtml(notice)}</div>`;
    return;
  }
  selected = id;
  screen = "job";
  try {
    await invoke("start_job", { id });
  } catch (err) {
    notice = String(err);
  }
  drawn = "";
  await render();
}

async function saveSettings(): Promise<void> {
  const login = document.querySelector<HTMLInputElement>("#login")?.checked || false;
  const workers = Number(document.querySelector<HTMLInputElement>("#workers")?.value || "20");
  const icloudpd = document.querySelector<HTMLInputElement>("#icloudpd")?.value || "icloudpd";
  try {
    const restartOnCrash = document.querySelector<HTMLInputElement>("#restart")?.checked ?? true;
    await invoke("set_settings", { login, restartOnCrash, workers, icloudpd });
  } catch (err) {
    notice = String(err);
    drawn = "";
    await render();
    return;
  }
  notice = "";
  screen = "home";
  drawn = "";
  await render();
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char] || char);
}

void render();
window.setInterval(() => {
  if (screen === "home" || screen === "job") void render();
}, 1000);
