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
};

type ReviewItem = { kind: string; keep_path: string | null; remove_path: string };
type FileError = { rel_path: string; attempts: number; error: string | null };
type ListedJob = Job & { done: number; total: number; arranged: number; icloud_status: IcloudStatus | null };
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
type Settings = { login: boolean; workers: number; icloudpd: string };

const app = document.querySelector("#app") as HTMLElement;
let screen: "home" | "new" | "job" | "settings" = "home";
let selected = "";
let notice = "";
let drawn = "";

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

function label(job: Job): string {
  if (job.state === "needs_review") {
    return job.stage === "mirror_review" ? "Review removal" : "Review duplicates";
  }
  return job.state.replaceAll("_", " ");
}

async function render(): Promise<void> {
  const jobs = await invoke<ListedJob[]>("list_jobs").catch(() => [] as ListedJob[]);
  const detail = selected ? await invoke<JobView>("job_detail", { id: selected }).catch(() => null) : null;
  const settings = screen === "settings" ? await invoke<Settings>("get_settings") : null;
  const html = `
    <div class="shell">
      <nav class="side">
        <button type="button" data-go="home" class="${screen === "settings" ? "" : "on"}">Jobs</button>
        <button type="button" data-go="settings" class="${screen === "settings" ? "on" : ""}">Settings</button>
        <div class="hint">Closing this window keeps a running sync in the menu bar.</div>
      </nav>
      <main>${body(jobs, detail, settings)}</main>
    </div>`;
  if (html === drawn) return;
  drawn = html;
  app.innerHTML = html;
}

function body(jobs: ListedJob[], detail: JobView | null, settings: Settings | null): string {
  if (screen === "settings" && settings) return settingsView(settings);
  if (screen === "new") return newView();
  if (screen === "job" && detail) return jobView(detail);
  return homeView(jobs);
}

function homeView(jobs: ListedJob[]): string {
  const rows = jobs.length
    ? jobs.map((job) => `
        <button class="job" type="button" data-open="${job.id}">
          <b>${baseName(job.source)} → ${baseName(job.dest)}</b>
          <span class="pill ${pill(job.state)}">${label(job)}</span>
          <small>${jobLine(job)}</small>
          <small>${job.source}</small>
          <small>${job.dest}</small>
        </button>`).join("")
    : `<p class="sub">No jobs yet. icloudpd can download into a folder while this app sorts finished files into months.</p>`;
  return `
    <div class="row">
      <div><h1>Jobs</h1><p class="sub">An iCloud job downloads and arranges at the same time. A folder job copies one snapshot.</p></div>
      <button class="primary" type="button" data-go="new">New sync</button>
    </div>
    ${rows}`;
}

function newView(): string {
  return `
    <h1>New sync</h1>
    <p class="sub">The copy uses the same MD5 rule as the Java sync. Extra steps stay off unless you turn them on.</p>
    <div id="form-error">${notice ? `<div class="error">${escapeHtml(notice)}</div>` : ""}</div>
    <label class="field"><span>Download folder</span><div class="picker"><input id="source" readonly placeholder="Choose a folder"><button class="ghost" type="button" id="pick-source">Choose</button></div></label>
    <label class="field"><span>Destination</span><div class="picker"><input id="dest" readonly placeholder="Choose a folder"><button class="ghost" type="button" id="pick-dest">Choose</button></div></label>
    <div class="checks">
      <label><input type="checkbox" id="icloud"> Download with icloudpd<small>Starts icloudpd into the download folder and sorts finished files while the download continues. Auto-delete stays off.</small></label>
    </div>
    <div id="icloud-fields" class="hidden">
      <label class="field"><span>Apple ID</span><input id="apple" type="text" placeholder="name@icloud.com" autocomplete="off"></label>
      <label class="field"><span>Check iCloud every (seconds)</span><input id="watch" type="number" min="60" value="3600"><small class="hint-inline">The password stays in the macOS keychain. A sign-in code opens in icloudpd’s page.</small></label>
    </div>
    <div class="checks">
      <label><input type="checkbox" id="recursive"> Include subfolders<small>Off matches the Java sync: only files sitting directly in the source. An iCloud job always includes subfolders.</small></label>
      <label><input type="checkbox" id="since"> Only files modified on or after <input type="date" id="since-date" disabled><small>Leave this off to copy every file in the snapshot.</small></label>
      <label><input type="checkbox" id="junk" checked> Skip .DS_Store and AppleDouble (._*) files</label>
      <label><input type="checkbox" id="sort"> Sort into YYYY-MM folders<small>Uses the capture date, then the file’s modified time. No “webp” folder.</small></label>
      <label><input type="checkbox" id="duplicates"> Review duplicates when the copy finishes</label>
      <label><input type="checkbox" id="removal"> Remove destination files that are no longer in the source<small>Always shows a list first. The first run deletes nothing.</small></label>
    </div>
    <div class="note hidden" id="sort-note">Sorting builds an archive, not a mirror of iCloud, so removal stays off for this job.</div>
    <button class="primary" type="button" id="start">Start</button>`;
}

function jobLine(job: ListedJob): string {
  if (!job.icloud) return `${job.done} of ${job.total} files`;
  const status = job.icloud_status;
  const sync = !status
    ? "Syncing"
    : status.library_total === null
      ? `Syncing ${status.checked} checked`
      : `Syncing ${status.checked} of ${status.library_total}`;
  const local = job.sort_into_months ? `Arranged ${job.arranged} of ${job.total}` : `Copied ${job.done} of ${job.total}`;
  return `${sync} · ${local}`;
}

function syncTrack(status: IcloudStatus | null): string {
  if (!status) {
    return `<section class="track"><div class="track-head"><b>Syncing</b><span>Starting…</span></div></section>`;
  }
  const known = status.library_total !== null && status.library_total > 0;
  const library = status.library_total ?? 0;
  const pct = known ? percent(status.checked, library) : 0;
  const count = status.library_total === null ? `${status.checked} checked · total unknown` : `${status.checked} of ${status.library_total} items`;
  const eta = status.eta_seconds === null ? "" : `${formatEta(status.eta_seconds)} left`;
  const skipped = status.skipped > 0 ? ` Skipped ${status.skipped}.` : "";
  const signin = status.needs_mfa
    ? `<div class="actions"><button class="primary" type="button" id="signin">Open the sign-in page</button></div>`
    : "";
  const note = status.message ? `<p class="detail">${escapeHtml(status.message)}</p>` : "";
  return `<section class="track">
    <div class="track-head"><b>Syncing</b><span>${escapeHtml(count)}</span></div>
    ${known ? progressBar(pct, false) : ""}
    <div class="meta"><span>${known ? `${pct}%` : "total unknown"}</span><span>${escapeHtml(eta)}</span></div>
    <p class="detail">Already on this Mac ${status.already}. Saved this pass ${status.saved}.${skipped}</p>
    ${note}${signin}
  </section>`;
}

function arrangeTrack(job: Job, done: number, total: number, arranged: number, bytesDone: number, bytesTotal: number, status: IcloudStatus | null): string {
  const sorting = job.sort_into_months;
  const finished = sorting ? arranged : done;
  const pct = percent(finished, total);
  const title = sorting ? "Arranging" : "Copying";
  const count = total === 0 ? "No files yet" : `${finished} of ${total} files`;
  const stillDownloading = status !== null && status.library_total !== null && status.checked < status.library_total;
  const hint = stillDownloading ? "More files appear as they finish downloading" : "Files already in the download folder";
  const bytes = bytesTotal > 0 ? ` · ${formatBytes(bytesDone)} / ${formatBytes(bytesTotal)}` : "";
  const file = job.current_file ? `<p class="file-now">${escapeHtml(job.current_file)}${bytes}</p>` : "";
  return `<section class="track">
    <div class="track-head"><b>${title}</b><span>${count}</span></div>
    ${progressBar(pct, true)}
    <div class="meta"><span>${pct}%</span><span>${hint}</span></div>
    ${file}
  </section>`;
}

function jobView(detail: JobView): string {
  const { job, done, total, arranged, errors, review, bytes_done, bytes_total, icloud } = detail;
  const pct = percent(done, total);
  const duplicate = review.filter((item) => item.kind === "duplicate");
  const removal = review.filter((item) => item.kind === "removal");
  const errorBlock = [notice, job.error || ""].filter(Boolean).map((text) => `<div class="error">${escapeHtml(text)}</div>`).join("");
  const byteLine = bytes_total > 0 ? `<span>${formatBytes(bytes_done)} / ${formatBytes(bytes_total)}</span>` : "";
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
    reviewBlock = `<h2>Review duplicates</h2><p class="sub">Nothing is deleted until you confirm.</p><table><thead><tr><th>Keep</th><th>Remove</th></tr></thead><tbody>${rows}</tbody></table><div class="actions"><button class="primary" type="button" id="confirm">Delete these copies</button><button class="ghost" type="button" id="skip">Skip</button>${job.icloud ? `<button class="quiet" type="button" id="stop">Stop</button>` : ""}</div>`;
  } else if (job.state === "needs_review" && removal.length) {
    const rows = removal.map((item) => `<tr><td>${escapeHtml(item.remove_path)}</td></tr>`).join("");
    reviewBlock = `<h2>Remove files no longer in the source</h2><p class="sub">These were copied by an earlier run. Files you added yourself are not listed.</p><table><thead><tr><th>Destination file</th></tr></thead><tbody>${rows}</tbody></table><div class="actions"><button class="primary" type="button" id="confirm">Delete these files</button><button class="ghost" type="button" id="skip">Skip</button>${job.icloud ? `<button class="quiet" type="button" id="stop">Stop</button>` : ""}</div>`;
  }
  const running = job.state === "running";
  const halted = job.state === "paused" || job.state === "stopped";
  const controls = running
    ? `<div class="actions"><button class="ghost" type="button" id="pause">Pause</button><button class="quiet" type="button" id="stop">Stop</button></div>`
    : halted
      ? `<div class="actions"><button class="primary" type="button" id="resume">Resume</button></div>`
      : job.state === "draft"
        ? `<div class="actions"><button class="primary" type="button" id="start-existing">Start</button></div>`
        : "";
  return `
    ${errorBlock}
    <p class="sub">${escapeHtml(job.stage)} · ${escapeHtml(job.state.replaceAll("_", " "))}</p>
    <h1>${escapeHtml(baseName(job.source))} → ${escapeHtml(baseName(job.dest))}</h1>
    ${job.icloud
      ? `${syncTrack(icloud)}${arrangeTrack(job, done, total, arranged, bytes_done, bytes_total, icloud)}`
      : `<p class="sub">${done} of ${total} files</p>${progressBar(pct, false)}<div class="meta"><span>${pct}%</span><span>${escapeHtml(job.source)}</span></div>${job.current_file ? `<div class="current"><b>${escapeHtml(job.current_file)}</b>${byteLine}</div>` : ""}`}
    ${controls}
    ${reviewBlock}
    ${fileErrors}`;
}

function settingsView(settings: Settings): string {
  return `
    <h1>Settings</h1>
    ${notice ? `<div class="error">${escapeHtml(notice)}</div>` : ""}
    <p class="sub">This Mac. A later Windows build would use a startup app instead of a login item.</p>
    <div class="setting"><div><b>Start at login</b><p>After a restart, a job that was running continues. A paused or stopped job stays that way.</p></div><input type="checkbox" id="login" ${settings.login ? "checked" : ""}></div>
    <div class="setting"><div><b>Parallel copies</b><p>The Java sync uses 20. One slow iCloud download occupies a single slot.</p></div><input id="workers" type="number" min="1" max="20" value="${settings.workers}"></div>
    <div class="setting"><div><b>icloudpd command</b><p>The program on PATH, or a full path to the binary. Auto-delete is never turned on.</p></div><input id="icloudpd" type="text" value="${escapeHtml(settings.icloudpd)}"></div>
    <button class="primary" type="button" id="save-settings">Save</button>`;
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
  if (node.id === "start-existing") void act("start_job");
  if (node.id === "confirm") void act("confirm_review", true);
  if (node.id === "skip") void act("confirm_review", false);
  if (node.id === "save-settings") void saveSettings();
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
    await invoke("set_settings", { login, workers, icloudpd });
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
