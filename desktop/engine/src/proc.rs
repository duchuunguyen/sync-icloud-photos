use std::collections::HashMap;
use std::path::PathBuf;

/// Turn a Settings command into a program `Command::new` can open.
///
/// A Mac app launched from Finder does not inherit the shell PATH, so a bare
/// `icloudpd` is also looked up in the pipx and Homebrew directories.
pub fn resolve_command(configured: &str) -> String {
    resolve_command_in(configured, &command_search_dirs())
}

fn resolve_command_in(configured: &str, dirs: &[PathBuf]) -> String {
    let configured = configured.trim();
    let name = if configured.is_empty() { "icloudpd" } else { configured };
    if name.contains('/') || name.contains('\\') {
        return name.to_string();
    }
    for dir in dirs {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

fn command_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join("Library/Application Support/pipx/venvs/icloudpd/bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs
}

/// Stop a spawned program and every process it started.
///
/// The icloudpd on PATH is a launcher. It starts another process, and that
/// process starts the downloader. Signaling only the launcher leaves the
/// downloader running. The launcher is started as its own process group, and
/// this also signals each descendant in case one of them left the group.
pub fn signal_interrupt(root: u32) {
    if root == 0 {
        return;
    }
    let targets = descendants(root);
    let group = format!("-{root}");
    let _ = std::process::Command::new("kill")
        .args(["-INT", "--", &group])
        .stderr(std::process::Stdio::null())
        .status();
    for pid in targets {
        let id = pid.to_string();
        let _ = std::process::Command::new("kill")
            .args(["-INT", "--", &id])
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn descendants(root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, ppid) in process_parents() {
        children.entry(ppid).or_default().push(pid);
    }
    let mut out = vec![root];
    let mut index = 0;
    while index < out.len() {
        if let Some(kids) = children.get(&out[index]) {
            for kid in kids {
                if !out.contains(kid) {
                    out.push(*kid);
                }
            }
        }
        index += 1;
    }
    out
}

fn process_parents() -> Vec<(u32, u32)> {
    let Ok(output) = std::process::Command::new("ps").args(["-axo", "pid=,ppid="]).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse::<u32>().ok()?;
            let ppid = parts.next()?.parse::<u32>().ok()?;
            Some((pid, ppid))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{resolve_command_in, signal_interrupt};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn bare_command_is_found_outside_path_and_a_path_is_kept() {
        let root = std::env::temp_dir().join(format!("icloudpd-lookup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("icloudpd"), "#!/bin/sh\n").unwrap();
        let found = resolve_command_in("icloudpd", &[bin]);
        assert_eq!(found, root.join(".local/bin/icloudpd").to_string_lossy());
        assert_eq!(resolve_command_in("/usr/local/bin/icloudpd", &[PathBuf::from("/missing")]), "/usr/local/bin/icloudpd");
        assert_eq!(resolve_command_in("  ", &[PathBuf::from("/missing")]), "icloudpd");
        let _ = fs::remove_dir_all(&root);
    }
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn interrupt_stops_a_grandchild_the_launcher_would_leave_behind() {
        let mut child = Command::new("python3")
            .args([
                "-u",
                "-c",
                "import subprocess\np=subprocess.Popen(['sleep','60'])\nprint(p.pid, flush=True)\np.wait()\n",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let parent = child.id();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let grand: u32 = line.trim().parse().unwrap();

        signal_interrupt(parent);

        let parent_dead = wait_until(Duration::from_secs(5), || child.try_wait().unwrap().is_some());
        let grand_dead = wait_until(Duration::from_secs(5), || !pid_alive(grand));
        if !parent_dead || !grand_dead {
            let _ = Command::new("kill").args(["-KILL", &parent.to_string(), &grand.to_string()]).status();
        }
        assert!(parent_dead, "launcher still running");
        assert!(grand_dead, "grandchild still running");
    }

    fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(30));
        }
        done()
    }

    fn pid_alive(pid: u32) -> bool {
        Command::new("ps").args(["-p", &pid.to_string()]).output().map(|output| output.status.success()).unwrap_or(false)
    }
}
