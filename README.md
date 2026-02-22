1. Add symlink folder for iCloud
- mklink /D "C:\Users\diemdiudang\Pictures\iCloud Photos\" "D:\xena\iCloud Photos"

2. Sync history
- Sync on 2025/11/16
- Sync on 2022/15/02

---

## Run commands

Build first:
```bash
mvn compile
```

### Sync
Syncs photos from iCloud Photos source to a target folder (paths are set in `Sync.java`).
```bash
mvn exec:java -Dexec.mainClass="org.example.Sync"
```

### MoveFolder
Moves photos from source into date-based subfolders under target (paths are set in `MoveFolder.java`).
```bash
mvn exec:java -Dexec.mainClass="org.example.MoveFolder"
```

### DuplicateCleaner
Finds duplicate files by name (e.g. `IMG_0381` and `IMG_0381(1)`) and removes copies that have the same checksum. Scans `G:\xena\arranged-photos` by default.

```bash
# Default root (G:\xena\arranged-photos), no deletion
mvn exec:java -Dexec.mainClass="org.example.DuplicateCleaner" -Dexec.args="--dry-run"

# Dry run and write JSON report
mvn exec:java -Dexec.mainClass="org.example.DuplicateCleaner" -Dexec.args="--dry-run --output report.json"

# Custom root folder
mvn exec:java -Dexec.mainClass="org.example.DuplicateCleaner" -Dexec.args="D:\my\photos --dry-run"

# Run for real (deletes duplicate files)
mvn exec:java -Dexec.mainClass="org.example.DuplicateCleaner"
```

Options:
- `--dry-run` — do not delete; only report what would be removed.
- `--output <file>` — write duplicate report to a JSON file.
- First non-option argument — root folder to scan (default: `G:\xena\arranged-photos`).

**Note:** Add `exec-maven-plugin` to `pom.xml` if `mvn exec:java` is not available.