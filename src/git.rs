//! Git change markers via the `git` binary (no libgit2). Every call has a timeout
//! and every failure degrades to "no git info" instead of an error.

use regex::Regex;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    /// Lines were deleted right below this line.
    DeletedBelow,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ItemGit {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
}

impl ItemGit {
    pub fn is_empty(&self) -> bool {
        self.added + self.modified + self.deleted == 0
    }
}

#[derive(Debug, Default)]
pub struct GitInfo {
    pub changes: BTreeMap<usize, Change>,
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
}

impl GitInfo {
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    pub fn range_stats(&self, start: usize, end: usize) -> ItemGit {
        let mut s = ItemGit::default();
        for (_, c) in self.changes.range(start..=end) {
            match c {
                Change::Added => s.added += 1,
                Change::Modified => s.modified += 1,
                Change::DeletedBelow => s.deleted += 1,
            }
        }
        s
    }
}

fn run_git(dir: &Path, args: &[&str], timeout: Duration) -> Option<(bool, Vec<u8>)> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some((status.success(), reader.join().ok()?)),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    }
}

fn hunk_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap())
}

/// Parse `git diff -U0` output into per-line markers (new-file line numbers).
pub fn parse_diff(diff: &str) -> GitInfo {
    let mut info = GitInfo::default();
    for line in diff.lines() {
        let Some(c) = hunk_re().captures(line) else {
            continue;
        };
        let num = |i: usize, default: usize| {
            c.get(i)
                .and_then(|m| m.as_str().parse().ok())
                .unwrap_or(default)
        };
        let (old_n, new_start, new_n) = (num(2, 1), num(3, 1), num(4, 1));
        if old_n == 0 {
            for l in new_start..new_start + new_n {
                info.changes.insert(l, Change::Added);
            }
            info.added += new_n;
        } else if new_n == 0 {
            info.changes
                .entry(new_start.max(1))
                .or_insert(Change::DeletedBelow);
            info.deleted += old_n;
        } else {
            let modified = old_n.min(new_n);
            for l in new_start..new_start + modified {
                info.changes.insert(l, Change::Modified);
            }
            info.modified += modified;
            if new_n > old_n {
                for l in new_start + modified..new_start + new_n {
                    info.changes.insert(l, Change::Added);
                }
                info.added += new_n - old_n;
            } else if old_n > new_n {
                info.changes
                    .entry(new_start + new_n - 1)
                    .or_insert(Change::DeletedBelow);
                info.deleted += old_n - new_n;
            }
        }
    }
    info
}

/// Changes of `path` against `rev` (working tree incl. staged). `None` when git is
/// missing, the file is not in a repository, or the command timed out.
pub fn collect(path: &Path, rev: &str) -> Option<GitInfo> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let name = path.file_name()?.to_str()?;
    let t = Duration::from_secs(5);

    let (ok, out) = run_git(&dir, &["rev-parse", "--is-inside-work-tree"], t)?;
    if !ok || String::from_utf8_lossy(&out).trim() != "true" {
        return None;
    }
    let (tracked, _) = run_git(&dir, &["ls-files", "--error-unmatch", "--", name], t)?;
    if !tracked {
        return Some(GitInfo::default());
    }
    let (ok, out) = run_git(
        &dir,
        &[
            "diff",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            rev,
            "--",
            name,
        ],
        t,
    )?;
    if !ok {
        return None;
    }
    Some(parse_diff(&String::from_utf8_lossy(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks() {
        let d = "@@ -3,0 +4,2 @@\n+a\n+b\n@@ -10,2 +12,0 @@\n-x\n-y\n@@ -20,2 +22,3 @@\n";
        let g = parse_diff(d);
        assert_eq!(g.changes.get(&4), Some(&Change::Added));
        assert_eq!(g.changes.get(&5), Some(&Change::Added));
        assert_eq!(g.changes.get(&12), Some(&Change::DeletedBelow));
        assert_eq!(g.changes.get(&22), Some(&Change::Modified));
        assert_eq!(g.changes.get(&23), Some(&Change::Modified));
        assert_eq!(g.changes.get(&24), Some(&Change::Added));
        assert_eq!((g.added, g.modified, g.deleted), (3, 2, 2));
    }
}
