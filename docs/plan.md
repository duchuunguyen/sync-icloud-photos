# Sync iCloud Photos — desktop app

Mac menu-bar app. The Java tools stay in `src/` as the behavior reference. This app does not talk to iCloud. It starts [icloudpd](https://github.com/icloud-photos-downloader/icloud_photos_downloader), watches that program's output, and copies finished files into month folders while the download is still running.

The phrase "Tauri 2 + TypeScript shell" only describes how the window is built. Tauri is a tool that produces a normal Mac app you double-click. It is not a website and it is not hosted anywhere. The part you see — buttons, lists, forms — is drawn with HTML and TypeScript. TypeScript is JavaScript with types, which is why an HTML mockup is a faithful preview of that window. The copying, hashing, and job database are Rust, a separate native language in the same app. The window asks that code to start, pause, stop, or resume. A long sync does not run inside the web view, so closing the window does not stop the job.

The clickable mockup of the window is `docs/mockup.html`. Open it in a browser. The screen switcher jumps to Home, New sync, a job while it is copying, duplicate review, removal review, and Settings. The menu-bar item at the top right opens the tray menu.

This file, `docs/plan.md`, is the plan we implement from. Each task has a status: `pending`, `in progress`, or `done`. Update the status here when the work starts and when it finishes.

## Decisions

- One job is one source folder and one destination folder. Several jobs can exist. Only one runs at a time.
- The copy rule matches `Sync.java`: snapshot the file list, skip a destination file whose MD5 already matches, replace it when the MD5 differs, copy it when it is missing. MD5 is the streaming content hash, same algorithm as `DigestUtils.md5Hex`.
- A crash must not leave a truncated file under the real name. Bytes go to `.<name>.sync-partial` beside the destination and are renamed into place only after the temp file matches the source MD5. Pause, stop, and crash delete the temp file and copy that name again later. A previous good destination file stays in place until that rename.
- Verified files are not hashed again on resume. That is what makes a multi-day job resumable.
- Closing the window hides it. Quit, crash, or reboot while a job is `running` continues that job the next time the worker starts. Paused and stopped jobs stay halted until Resume.
- Start at login is on by default (a per-user LaunchAgent). The app is unsandboxed. No Apple Developer account.
- Worker count stays 20.
- The app does not delete the source. icloudpd's download folder stays intact, so the next icloudpd run still sees the files and does not download them again.
- The app starts stock icloudpd. It does not fork icloudpd and it does not pass `--auto-delete`. Photos removed on the phone stay in the download folder.

These Java `main` details are run settings, not the sync rule:

- Hardcoded `D:\` and `G:\` paths. The window asks for folders instead.
- The 15 Oct 2025 modified-since filter. It becomes an optional date on the job, empty by default (all files).
- `error_files.txt` in the working directory. Failed names are stored on the job and shown in the window.

## Application design

### What you can do

Pick a download folder and a destination, start a sync, and leave it running with the window closed. Pause, stop, and resume survive quitting the app, a crash, and a Mac restart. An iCloud job starts icloudpd and arranges each finished file while later files are still downloading. A plain folder job, with no Apple ID, still copies one snapshot and does not start icloudpd.

After the copy, the same job can sort files into month folders, ask before deleting duplicates, and ask before removing destination files that are no longer in the source. Those three are checkboxes, off by default. They are part of this plan, not a follow-up project.

### Screens

**Home.** Lists jobs: source folder, destination folder, state, and files finished / total. New sync starts a job. An empty home explains the app in one line and offers New sync.

**New sync.**

- Source folder and destination folder, chosen with the system folder dialogs.
- Include subfolders. Default off, which is what `Sync.java` does (top-level files only).
- Only files modified on or after a date. Empty means all files. The comparison is the file's last-modified time at the start of that local day, same as `listFilesByDate`.
- Skip `.DS_Store` and `._*` files. Default on.
- Sort into `YYYY-MM` folders. Default off.
- Review duplicates when the copy finishes. Default off.
- Remove destination files that are no longer in the source. Default off. The form turns this off and explains why if Sort into months is on. The arranged archive is not a mirror of iCloud. `sync2ways` was already commented out of `Sync.main` for that reason.
- Download with icloudpd. Optional. Asks for the Apple ID and how many seconds to wait between iCloud checks (default 3600, minimum 60). The password stays in the macOS keychain. A sign-in code is entered in icloudpd's own page at `http://127.0.0.1:8080`. When this is on, Include subfolders and Sort into YYYY-MM start checked, and removal stays off. Auto-delete is off and is not a checkbox.

**Job.** A folder job shows one bar for the snapshot. An iCloud job shows two bars: syncing (items checked in the library) and arranging (files placed in month folders). The file being copied is listed under arranging, with that file’s byte count. Pause and Stop show while it runs. Resume shows when it is paused or stopped. Errors list the file name, the attempt count, and the message. Options are read-only while the job is running.

**Duplicate review.** Shown only when that option is on and the copy has finished. One row per group: the file to keep, the files to remove. Confirm deletes those files. Skip leaves them. Nothing is deleted before Confirm.

**Removal review.** Shown only when removal is on, sorting is off, and the copy has finished. Lists destination files this app copied on an earlier run that are not in the new snapshot. Confirm deletes that list. Skip leaves them. The first run has no earlier record, so it deletes nothing. Files the app did not write are never in the list.

**Tray.** Status of the running job, Pause or Resume, Open, Quit. Quit exits the process. A `running` job continues on the next launch.

**Settings.** Start at login. Parallel copies, default 20. Path of the `icloudpd` command, default `icloudpd`.

If listing or reading the source fails with a permission error, the job screen says to enable Full Disk Access for this app. The app cannot grant that itself. An iCloud placeholder is read like any other file: the read waits while macOS downloads it, and one stuck file uses only one of the 20 workers.

### Stages

A job remembers the stage it is in. Resume continues that stage and does not scan again, except a `running` job that died during the scan starts the scan over.

1. **Scan.** Build the snapshot. Honor subfolders, the optional date, and the junk-file skip. Files that show up after the snapshot are not part of this job.
2. **Copy.** Apply the MD5 rule with the temp file and rename. One file's error does not stop the others.
3. **Verify.** Hash the snapshot again except files already recorded as verified. A mismatch or a hash error is retried, not deleted.
4. **Sort.** Only if the checkbox is on. Each verified file is moved into `destination/YYYY-MM/`.
5. **Duplicate review.** Only if that checkbox is on. Wait for Confirm or Skip.
6. **Removal review.** Only if that checkbox is on and sort is off. Wait for Confirm or Skip.
7. **Done.** `completed`, or `completed_with_errors` when any file used up its retries.

### Job and file states

```text
draft → running → completed
                → completed_with_errors
                → needs_review
      → paused
      → stopped
```

`needs_review` is the duplicate list or the removal list waiting for a button.

Per file:

```text
pending → copying → copied → verified → arranged
                         ↘ copy_failed → failed
              verified ↘ verify_failed → failed
```

- `copying` after a crash is `pending` again. The temp file is removed. The destination bytes from before that attempt are still there.
- `copy_failed` and `verify_failed` retry on resume, up to 3 attempts.
- `failed` means the attempts are used up. The destination file is left as it is and the name stays on the error list. The source is untouched.
- A verify mismatch does not delete the destination. The next attempt copies again. This replaces the Java behavior that wrote `error_files.txt` and deleted the bad copy.

### Sort rules

`MoveFolder` intends to use `Date/Time Original` for images and `Creation Date` for videos. The tag lookup compares a string with a collection of tag objects, so it never matches, and every image and video is filed under `File Modified Date`. A file that is not jpg, jpeg, heic, png, mov, or mp4 is put in a folder named `webp`. The app does not repeat that.

- jpg, jpeg, heic, png: `Date/Time Original`, then last-modified time.
- mov, mp4: creation date, then last-modified time.
- Anything else: last-modified time.
- The folder name is `yyyy-MM` under the destination. There is no `webp` bucket.
- With subfolders and sort both on, the month folder is still the destination layout. The source's relative folders are not kept. Two files with the same name from different folders are handled like a collision.
- Collision in the month folder, same MD5: the extra copy is not written.
- Collision, different MD5: keep the file already there, write the new one as `name (N).ext` with the smallest free N. If duplicate review is on, that pair is in the review list. The review still only removes a copy when the normalized name and the MD5 both match, which is the `DuplicateCleaner` rule, so a different-MD5 collision is shown and not removed.

### Duplicate rule

Inside one folder (one month folder, or the destination root when sort is off): strip a trailing ` (N)` or `(N)` before the extension, and require the same MD5. Keep the name without `(N)`, otherwise the smallest N, otherwise the alphabetical name. Same checksum under unrelated names is kept. The same bytes in two month folders are kept.

### Removal rule

Only when sort is off. A destination file is proposed for deletion only when a previous job by this app recorded it and its relative path is absent from the new snapshot. Confirm is required. This is narrower than `sync2ways`, which deletes any destination name missing from the source, including files you put there yourself.

### icloudpd

icloudpd is a separate command. It logs into iCloud on the web and downloads into `--directory`. Checked out at `~/it/code/icloud_photos_downloader`. The app runs the `icloudpd` binary. A bare name is looked up on `PATH`, then in `~/.local/bin` (pipx), the pipx venv, Homebrew, and `/usr/local/bin`, because a Mac app opened from Finder does not inherit the shell PATH. The Settings field can point at a full path. It does not import the Python package and it does not fork the project.

There is no status socket and no status file. `StatusExchange` stays inside the icloudpd process. The web page on port 8080 is HTML and starts only for the password or the sign-in code. `--notification-script` runs only when two-factor authentication expires. The channel the app uses is stdout. icloudpd's logger prints `YYYY-MM-DD HH:MM:SS LEVEL    message` and flushes every line. The default log level is debug, so the app does not pass `--log-level`.

The app starts this command. It never adds `--auto-delete`.

```text
icloudpd
  --directory "<download folder>"
  --username "<apple id>"
  --password-provider keyring
  --password-provider webui
  --mfa-provider webui
  --no-progress-bar
  --watch-with-interval <seconds>
```

The Apple ID password stays in the macOS keychain. If the keychain does not have it yet, icloudpd asks for it on the same page it uses for the sign-in code, then saves it to the keychain. When stdout says two-factor or two-step authentication is required, the job shows that and opens `http://127.0.0.1:8080` on request. Cookies stay in icloudpd's own cookie directory.

Before the first download, icloudpd asks iCloud for `itemCount` (library items, not counting hidden or Recently Deleted) and logs `Downloading N original photos and videos to <directory> ...`. That N is items, not files. A Live Photo is one item and two files. `--recent N` replaces the total. `--until-found` logs `???`, so the total is unknown. The app shows N as "in iCloud".

It does not know up front how many of those items are missing on disk. At debug level each item then logs one of:

- `… already exists` — already in the download folder
- `Downloaded …` — saved on this pass
- `Skipping …` — left out by a filter

The logged path can be cut to 96 characters, so the app does not copy from that path. It counts the lines. "Checked" is those three counts added. "Saved this pass" is the `Downloaded` lines. "Already on this Mac" is the `already exists` lines.

The time left is an estimate the app computes from how fast recent items were checked: remaining items times elapsed time divided by items checked. It appears after one second, and only when the total is a number. It is not a byte-size estimate. icloudpd learns a file's size when it reaches that file and does not add those sizes up. The label says "about".

A file is safe to copy when its final name is in the download folder. icloudpd writes `….part` and renames only after the bytes are complete. The app scans the download folder about once a second and queues any new file that is not still a partial. Copying overlaps the next download. Files that were already in the folder are queued as soon as the job starts. The copy uses the same MD5 temp-file rule, into `destination/YYYY-MM/`. The source file stays where it is.

The scan skips `.part` files, `.icloudpd-pruned/` and everything inside it, `.icloudpd-prune.json`, and `.icloudpd-prune.lock`. Include subfolders is on, because icloudpd's default layout is `YYYY/MM/DD`. The month folder is still one level, `2024-03`, not `2024/03`.

Duplicate review and removal review wait until the pass is quiet. A pass is quiet when a line contains `All photos` or `All videos` and `have been downloaded`, no `ERROR` line follows in that pass, and then a line contains `Waiting` and `sec`. Exit code 0 after the downloaded line counts the same way. `ERROR`, a non-zero exit, or `Iteration was cancelled` does not open a review. The job shows the line and keeps arranging files that are already complete. The next `Downloading N` line starts a new pass and clears that error.

An empty review does not finish the job. The job stays running and keeps watching. A review that has rows stays on the confirm screen. icloudpd keeps running in its wait. Confirm or Skip returns to arranging. A second review is not opened until the next quiet pass, and only after the files from that pass are arranged.

Pause, stop, and Quit send SIGINT to the whole icloudpd process tree. The program on PATH is a launcher: it starts another process, and that process is the one that downloads. Signaling only the launcher leaves the download running. icloudpd does not freeze in place. Pause and stop both end that run. Resume starts icloudpd again, and it skips files already on disk. A `running` iCloud job starts icloudpd again on the next launch. A `needs_review` job does not, until the user confirms or resumes.

### Job screen while iCloud is downloading

Syncing and arranging are separate bars. The arrange queue is not the overall progress.

- Syncing: checked items out of the N from `Downloading N …`, or "total unknown" with no filled bar. Already on this Mac, saved this pass, and about … left sit under that bar.
- Arranging: files placed in `YYYY-MM` out of the files already in the download folder. That total grows as downloads finish. It is not the library total, and it is not the progress of one file. While the library pass is still running, the label says more files will appear.
- The file being copied is a line under Arranging, with that file’s byte count.
- A button to open the sign-in page when Apple asks for a code.

### Where state lives

SQLite at `~/Library/Application Support/Sync iCloud Photos/jobs.db`. The copier writes a file's state before it starts the next one.

### Code layout

```text
desktop/
  engine/             scan, MD5, copy, verify, sort, review, SQLite, worker
  ui/                 Vite + TypeScript window
  src-tauri/          Tauri commands, tray, login item
```

The engine is its own crate so `cargo test --manifest-path desktop/engine/Cargo.toml` does not compile the window. The window calls create, start, pause, stop, resume, confirm review, and status. Tests call the library directly.

## Tasks

### Design

- [x] **done** — Review `docs/mockup.html` (Home, New sync, Copying, Duplicate review, Removal review, Settings, and the menu-bar menu).
- [x] **done** — Save this document as `docs/plan.md`.

### Engine

- [x] **done** — Scan a snapshot: top-level only, optional subfolders, optional modified-since date, skip `.DS_Store` and `._*` when that option is on.
- [x] **done** — MD5 copy rule: skip when equal, replace when different, create when missing. Temp file, then rename only after the hash matches.
- [x] **done** — Verify pass. Mismatch or hash error retries up to 3 times and does not delete the destination. A used-up file stays on the error list.
- [x] **done** — One file's error does not cancel the other files.
- [x] **done** — Sort verified files into `YYYY-MM` using the date rules above, including same-MD5 and different-MD5 collisions.
- [x] **done** — Build a duplicate-review list with the `DuplicateCleaner` keep rule. Delete only the confirmed names.
- [x] **done** — Build a removal list from files this app recorded that are missing from the new snapshot. Delete only the confirmed names. Do nothing on the first run.

### Job control

- [x] **done** — SQLite job and file states, including stage and attempt count.
- [x] **done** — Pause and stop abandon temp files and stay halted across restart. Resume continues the same snapshot and stage.
- [x] **done** — A `running` job continues on worker start. `copying` becomes `pending`. A `needs_review` job stays on its confirm screen.

### Desktop shell

- [x] **done** — Home, New sync, and Job screens, including both review screens.
- [x] **done** — Tray, hide on close, Quit, and the permission-error hint.
- [x] **done** — Start at login via a per-user LaunchAgent. Default on. Parallel-copy setting, default 20.

### Windows

- [ ] **pending** — Not needed for now. Run the same engine and window on Windows, with the login hook as a startup app instead of a LaunchAgent.

### icloudpd

- [x] **done** — Start stock icloudpd with keyring, webui, no progress bar, and a watch interval. Do not pass `--auto-delete`. Do not store the Apple ID password.
- [x] **done** — Count library size, checked items, files saved, and an item-rate time estimate from stdout.
- [x] **done** — Arrange finished files into `YYYY-MM` while icloudpd is still downloading. Skip `.part`, `.icloudpd-pruned/`, `.icloudpd-prune.json`, and `.icloudpd-prune.lock`.
- [x] **done** — Open duplicate or removal review only after a quiet pass, and only when that checkbox is on. An error line does not open a review.
- [x] **done** — Pause, stop, and Quit send SIGINT. Resume and the next launch start icloudpd again when the job is `running`.
- [x] **done** — Pause, stop, and Quit stop the icloudpd process tree. The launcher pid alone is not enough.
- [x] **done** — Find a bare `icloudpd` command in `~/.local/bin`, the pipx venv, Homebrew, and `/usr/local/bin` when Finder's PATH does not include it.
- [x] **done** — Show iCloud sync progress and local arrange progress as two bars. The file queue is not the only progress bar.

## Tests

`cargo test` uses real temp directories and real MD5. No UI tests in this pass. Each engine and job-control task is done only when the tests for it pass.

Copy and scan:

- Identical destination is not rewritten (modified time unchanged).
- Different destination is replaced and then matches the source MD5.
- Missing destination is created.
- A crash token (`copying` plus a partial temp) leaves the old destination bytes unchanged, and resume copies that file again.
- With subfolders off, a nested file is not in the snapshot. With subfolders on, it is, under its relative path.
- A file created after the snapshot is not copied.
- Modified-since keeps a file changed on that local day and drops an older one.
- `.DS_Store` and `._*` are skipped only when the option is on.
- A copy error on one file still copies the others.
- Empty source completes with zero files.
- A known MD5 vector matches commons-codec.

Verify and retries:

- Verify match does not change the file.
- Verify mismatch does not delete the file. After 3 failed attempts the state is `failed` and the bytes are still there.
- A hash read that throws is retried the same way, not dropped the way `verify` drops an exception from `getChecksum`.

Sort:

- Image fixture with `Date/Time Original` goes to that `yyyy-MM`, not the file's modified month, when the two differ.
- Video fixture with a creation date does the same.
- A file with no capture date uses last-modified time.
- A non-media file uses last-modified time and is not placed in `webp`.
- Same name and same MD5 in the month folder does not write a second file.
- Same name and different MD5 writes `name (N).ext` and keeps the original.

Reviews:

- `IMG_0381(1).jpg` and `IMG_0381.jpg` with the same MD5 propose keeping `IMG_0381.jpg`.
- The same normalized name with different MD5s proposes nothing.
- Confirm deletes the proposed files. Skip deletes nothing.
- Removal on a first run deletes nothing.
- Removal proposes a previously recorded file that is absent from the new snapshot, and does not propose a file the app never recorded.

Job control:

- Pause leaves the remaining names pending. Resume finishes them and does not scan again.
- Worker start continues a `running` job and does not continue a `paused`, `stopped`, or `needs_review` job.
- Stop then Resume continues the same snapshot.

icloudpd:

- `Downloading 10 original photos and videos to /library ...` sets the library total to 10. A later `Downloaded a.jpg` and `b.jpg already exists` set saved to 1 and already to 1.
- After one second of checks, the estimate is remaining items times elapsed time divided by items checked.
- `All photos and videos have been downloaded` followed by `Waiting for 3600 sec...` marks the pass quiet. An `ERROR` line before `Waiting` does not.
- The spawned arguments include `--password-provider keyring`, `--mfa-provider webui`, and `--watch-with-interval`, and do not include `--auto-delete`.
- A file written into the download folder while the job is running is copied. `skip.part`, `.icloudpd-pruned/old.txt`, and `.icloudpd-prune.json` are not.

## Build order

1. Review `docs/mockup.html`. `docs/plan.md` is already in the repo.
2. Engine tasks, in the order listed, each with its tests.
3. Job-control tasks and their tests.
4. Desktop shell.
5. Windows task is not needed for now.
6. icloudpd: start the command, arrange finished files in parallel, show the counts.
