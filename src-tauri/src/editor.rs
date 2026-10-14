//! Opening PRs locally: find the user's clones of each GitHub repo, check out a PR's branch
//! (only when the working tree is clean), and open the folder in their editor.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

const MAX_DEPTH: usize = 3;
const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "vendor", "dist", "build", "Library", "Pods", "DerivedData", "Applications",
    "Music", "Movies", "Pictures", "Public", "Downloads",
];

/// Editors we know how to launch on macOS (app bundle names).
const MAC_EDITORS: &[&str] = &[
    "Visual Studio Code", "Cursor", "Zed", "Windsurf", "Sublime Text", "Nova", "IntelliJ IDEA",
    "IntelliJ IDEA CE", "PyCharm", "PyCharm CE", "WebStorm", "GoLand", "RustRover", "PhpStorm", "RubyMine",
    "Android Studio", "Xcode",
];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// By default, scan the home folder (three levels deep, skipping media and system folders).
pub fn default_roots() -> Vec<String> {
    vec![home().to_string_lossy().into_owned()]
}

/// Where new clones go: ~/code.
pub fn default_clone_dir() -> String {
    home().join("code").to_string_lossy().into_owned()
}

/// Clone `owner/repo` into `parent/repo` (via gh, so private repos work). Returns the new path.
pub fn clone_repo(repo: &str, parent: &str) -> Result<String> {
    let name = repo.split('/').nth(1).context("bad repo name")?;
    std::fs::create_dir_all(parent).with_context(|| format!("Couldn't create {parent}"))?;
    let target = Path::new(parent).join(name);
    if target.exists() {
        bail!("{} already exists", target.display());
    }
    let target_s = target.to_string_lossy().into_owned();
    let out = match find_gh() {
        Some(gh) => Command::new(gh).args(["repo", "clone", repo, &target_s]).output()?,
        None => Command::new("git").args(["clone", &format!("https://github.com/{repo}.git"), &target_s]).output()?,
    };
    if !out.status.success() {
        bail!("Clone failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(target_s)
}

static GITHUB_REMOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"github\.com[:/]+([\w.-]+/[\w.-]+?)(?:\.git)?/?$").unwrap());

/// "owner/repo" from the clone's origin remote (falls back to any GitHub remote).
fn github_repo_of(dir: &Path) -> Option<String> {
    let git = dir.join(".git");
    let config = if git.is_dir() {
        std::fs::read_to_string(git.join("config")).ok()?
    } else {
        // Worktrees and submodules: ".git" is a file pointing at the real git dir.
        let pointer = std::fs::read_to_string(&git).ok()?;
        let gitdir = pointer.strip_prefix("gitdir:")?.trim();
        let gitdir = dir.join(gitdir);
        let common = std::fs::read_to_string(gitdir.join("commondir")).ok().map(|c| gitdir.join(c.trim()));
        std::fs::read_to_string(common.unwrap_or(gitdir).join("config")).ok()?
    };
    let mut origin = None;
    let mut any = None;
    let mut section = String::new();
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
        } else if let Some(url) = line.strip_prefix("url").map(|r| r.trim_start_matches([' ', '=']).trim()) {
            if let Some(c) = GITHUB_REMOTE.captures(url) {
                let repo = c[1].to_string();
                if section == "[remote \"origin\"]" {
                    origin = Some(repo);
                } else if any.is_none() {
                    any = Some(repo);
                }
            }
        }
    }
    origin.or(any)
}

/// Walk the roots and map "owner/repo" (lowercase) → clone path. The shallowest clone wins.
pub fn discover(roots: &[String]) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for root in roots {
        walk(Path::new(root), 0, &mut found);
    }
    found
}

fn walk(dir: &Path, depth: usize, found: &mut BTreeMap<String, String>) {
    if dir.join(".git").exists() {
        if let Some(repo) = github_repo_of(dir) {
            found.entry(repo.to_lowercase()).or_insert_with(|| dir.to_string_lossy().into_owned());
        }
        return; // don't descend into a repo
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&e.path(), depth + 1, found);
        }
    }
}

/// Editors installed on this machine.
pub fn installed_editors() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        let dirs = [PathBuf::from("/Applications"), home().join("Applications")];
        MAC_EDITORS
            .iter()
            .filter(|name| dirs.iter().any(|d| d.join(format!("{name}.app")).exists()))
            .map(|s| s.to_string())
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        ["code", "cursor", "zed", "subl"]
            .iter()
            .filter(|cli| Command::new(cli).arg("--version").output().is_ok())
            .map(|s| s.to_string())
            .collect()
    }
}

pub fn open_in(editor: &str, path: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").args(["-a", editor, path]).status();
    #[cfg(not(target_os = "macos"))]
    let status = Command::new(editor).arg(path).status();
    let status = status.with_context(|| format!("Couldn't launch {editor}"))?;
    if !status.success() {
        bail!("{editor} didn't open {path}");
    }
    Ok(())
}

fn git(path: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(path).args(args).output().context("git isn't available")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn find_gh() -> Option<String> {
    let home = home();
    ["gh".to_string(), "/opt/homebrew/bin/gh".into(), "/usr/local/bin/gh".into(), home.join(".local/bin/gh").to_string_lossy().into_owned(), "/usr/bin/gh".into()]
        .into_iter()
        .find(|bin| Command::new(bin).arg("--version").output().is_ok_and(|o| o.status.success()))
}

/// Switch the clone to the PR's branch. Refuses if there are uncommitted changes, so nothing is lost.
pub fn checkout_pr(path: &str, number: i64, branch: Option<&str>, cross_repo: bool) -> Result<String> {
    let dirty = git(path, &["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.is_empty() {
        bail!("{path} has uncommitted changes, so the branch wasn't switched");
    }
    let current = git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    if branch.is_some_and(|b| b == current) && !cross_repo {
        let _ = git(path, &["pull", "--ff-only"]);
        return Ok(current);
    }
    // gh handles forks and existing local branches; fall back to plain git.
    if let Some(gh) = find_gh() {
        let out = Command::new(&gh).current_dir(path).args(["pr", "checkout", &number.to_string()]).output()?;
        if out.status.success() {
            return Ok(git(path, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| format!("PR #{number}")));
        }
    }
    match branch.filter(|_| !cross_repo) {
        Some(b) => {
            git(path, &["fetch", "origin", b])?;
            git(path, &["checkout", b])?;
            let _ = git(path, &["merge", "--ff-only", &format!("origin/{b}")]);
            Ok(b.to_string())
        }
        None => {
            let local = format!("pr-{number}");
            git(path, &["fetch", "origin", &format!("pull/{number}/head:{local}")])?;
            git(path, &["checkout", &local])?;
            Ok(local)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let root = std::env::temp_dir().join(format!("dev-inbox-discover-{}", std::process::id()));
        let dir = root.join("clients/web");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(
            dir.join(".git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://github.com/other/thing.git\n[remote \"origin\"]\n\turl = git@github.com:Acme/Web-App.git\n",
        )
        .unwrap();
        assert_eq!(github_repo_of(&dir).as_deref(), Some("Acme/Web-App"));
        let found = discover(&[root.to_string_lossy().into_owned()]);
        assert_eq!(found.get("acme/web-app").map(String::as_str), Some(dir.to_string_lossy().as_ref()));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod checkout_tests {
    use std::process::Command;

    #[test]
    fn refuses_with_uncommitted_changes() {
        let dir = std::env::temp_dir().join(format!("dev-inbox-dirty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        let run = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&path).args(args).output().unwrap().status.success());
        run(&["init", "-q"]);
        run(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        run(&["add", "a.txt"]);
        run(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "a"]);
        std::fs::write(dir.join("a.txt"), "uncommitted work").unwrap();

        let err = super::checkout_pr(&path, 1, Some("feature"), false).unwrap_err().to_string();
        assert!(err.contains("uncommitted changes"), "{err}");
        // The work is untouched.
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "uncommitted work");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod live_tests {
    /// Lists what discovery finds on this machine: `cargo test discover_here -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn discover_here() {
        let roots = super::default_roots();
        let t = std::time::Instant::now();
        let found = super::discover(&roots);
        println!("roots={roots:?}\nfound {} clones in {:?}", found.len(), t.elapsed());
        for (repo, path) in found.iter().take(8) {
            println!("  {repo} → {path}");
        }
        println!("editors={:?}", super::installed_editors());
    }
}
