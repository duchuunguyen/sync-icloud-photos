# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Two codebases

- **`src/` (Java 17, Maven)** — original command-line tools. Each is a standalone `main` class in `org.example` with source/target paths hardcoded as constants (Windows paths such as `G:\xena\arranged-photos`); edit the constants in the file to change paths.
  - `Sync` — copies iCloud Photos source → target using a 20-thread pool. Skips files whose MD5 already matches, replaces on mismatch, copies if missing. Failures are written to `error_files.txt`.
  - `MoveFolder` — moves photos into date-based subfolders (date read via `metadata-extractor`).
  - `DuplicateCleaner` — finds `IMG_0381` / `IMG_0381(1)`-style name duplicates and deletes copies with identical checksum. Supports `--dry-run`, `--output <json>`, and a root-folder positional arg.
- **`desktop/` (Tauri 2 + TypeScript UI, Rust engine)** — Mac menu-bar app that replaces the Java tools. The Java code is the *behavior reference*; the design lives in `docs/plan.md` (decisions, job/file states, sort/duplicate/removal rules, task statuses — update task status there as work starts/finishes). `docs/mockup.html` is a clickable UI preview.
  - Cargo workspace: `desktop/engine` (crate `sync-icloud-engine`: copy, hash, scan, sort, dates, review, store, worker, icloudpd log parsing) and `desktop/src-tauri` (app shell). UI is in `desktop/ui` (Vite).
  - The app does not talk to iCloud: it launches `icloudpd`, watches its output, and copies finished files into month folders while the download runs. Jobs run in the Rust worker, not the web view, so closing the window does not stop a job.
  - Key invariants (see `docs/plan.md`): copy rule must match `Sync.java`; writes go to `.<name>.sync-partial` and are renamed only after MD5 matches the source; verified files are not re-hashed on resume; the source folder is never deleted from; one job runs at a time.

## Commands

Java (repo root):
```bash
mvn compile
mvn test                                  # all tests (JUnit 3.8.1: AppTest, DuplicateCleanerTest)
mvn test -Dtest=DuplicateCleanerTest      # single test class
mvn exec:java -Dexec.mainClass="org.example.Sync"   # or MoveFolder / DuplicateCleaner
```
`exec-maven-plugin` is not in `pom.xml`; the `mvn exec:java` form resolves it by prefix only if available — add the plugin if it fails.

Desktop (from `desktop/`):
```bash
npm install
npm run dev                # tauri dev
npm run build              # tauri build
cargo test -p sync-icloud-engine            # engine tests
cargo test -p sync-icloud-engine <name>     # single test
```
