//! Version-management tool, invoked as `cargo xtask <command>`.
//!
//! Source of truth: `[package] version` and `[package.metadata.release] date`
//! in the root Cargo.toml. Derived files (README version line) are kept in
//! sync by `sync` / `bump` / `set` / `release`. See VERSION_MANAGEMENT.md.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const README_MARKER: &str = "**Текущая версия:**";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live inside the workspace")
        .to_path_buf()
}

fn fail(msg: &str) -> ! {
    eprintln!("error: {msg}");
    exit(1);
}

/// Extract the value of a `key = "value"` line. The first match wins, which
/// is safe here: `version` appears first in `[package]` and `date` only in
/// `[package.metadata.release]`.
fn toml_value(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .strip_prefix('"')?
            .strip_suffix('"')
            .map(str::to_string)
    })
}

fn replace_toml_value(manifest: &str, key: &str, new_value: &str) -> String {
    let mut replaced = false;
    manifest
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if !replaced
                && trimmed.starts_with(key)
                && trimmed[key.len()..].trim_start().starts_with('=')
            {
                replaced = true;
                format!("{key} = \"{new_value}\"")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn parse_version(version: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = version.split('.').filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [major, minor, patch] => (*major, *minor, *patch),
        _ => fail(&format!("cannot parse version: {version}")),
    }
}

fn today() -> String {
    let out = Command::new("date")
        .arg("+%d.%m.%Y")
        .output()
        .expect("failed to run `date`");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

struct Project {
    root: PathBuf,
    manifest: String,
    version: String,
    date: String,
}

impl Project {
    fn load() -> Self {
        let root = workspace_root();
        let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
        let version =
            toml_value(&manifest, "version").unwrap_or_else(|| fail("no version in Cargo.toml"));
        let date = toml_value(&manifest, "date")
            .unwrap_or_else(|| fail("no [package.metadata.release] date in Cargo.toml"));
        Self { root, manifest, version, date }
    }

    fn readme_line(&self) -> String {
        format!("{README_MARKER} {} ({})", self.version, self.date)
    }

    fn readme_in_sync(&self) -> bool {
        fs::read_to_string(self.root.join("README.md"))
            .map(|readme| readme.lines().any(|l| l.trim() == self.readme_line()))
            .unwrap_or(false)
    }
}

fn status() {
    let project = Project::load();
    println!("version:       {}", project.version);
    println!("release date:  {}", project.date);
    println!(
        "README.md:     {}",
        if project.readme_in_sync() { "OK" } else { "DESYNC (run: cargo xtask sync)" }
    );
    if let Some(desc) = git(&project.root, &["describe", "--tags", "--always", "--dirty"]) {
        println!("git:           {desc}");
    }
}

fn sync(new_version: Option<&str>) -> Project {
    let mut project = Project::load();
    let version = new_version.unwrap_or(&project.version).to_string();
    if new_version.is_some() {
        parse_version(&version); // validate format
    }
    let date = today();

    project.manifest = replace_toml_value(&project.manifest, "version", &version);
    project.manifest = replace_toml_value(&project.manifest, "date", &date);
    fs::write(project.root.join("Cargo.toml"), &project.manifest).expect("write Cargo.toml");
    project.version = version;
    project.date = date;

    // Rewrite (or report a missing) version line in README.md.
    let readme_path = project.root.join("README.md");
    let readme = fs::read_to_string(&readme_path).expect("read README.md");
    if readme.lines().any(|l| l.trim_start().starts_with(README_MARKER)) {
        let updated: String = readme
            .lines()
            .map(|l| {
                if l.trim_start().starts_with(README_MARKER) {
                    project.readme_line()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&readme_path, updated).expect("write README.md");
    } else {
        eprintln!("warning: no \"{README_MARKER}\" line found in README.md — add one");
    }

    // Refresh Cargo.lock so the version change is reflected there too.
    let _ = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(&project.root)
        .output();

    println!("synced: {} ({})", project.version, project.date);
    project
}

fn bump(level: &str) {
    let project = Project::load();
    let (major, minor, patch) = parse_version(&project.version);
    let next = match level {
        "patch" => format!("{major}.{minor}.{}", patch + 1),
        "minor" => format!("{major}.{}.0", minor + 1),
        "major" => format!("{}.0.0", major + 1),
        other => fail(&format!("unknown bump level: {other} (expected patch|minor|major)")),
    };
    sync(Some(&next));
}

fn release(new_version: Option<&str>) {
    let project = sync(new_version);
    let root = &project.root;
    let tag = format!("v{}", project.version);

    for args in [
        vec!["add", "Cargo.toml", "Cargo.lock", "README.md"],
        vec!["commit", "-m", &format!("Release {tag}")],
        vec!["tag", &tag],
        vec!["push"],
        vec!["push", "origin", &tag],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(root)
            .status()
            .expect("failed to run git");
        if !status.success() {
            fail(&format!("git {} failed", args.join(" ")));
        }
    }
    println!("released {tag} and pushed (commit + tag)");
}

fn print_help() {
    println!(
        "cargo xtask <command>\n\n\
         commands:\n  \
         status              show version, release date and sync state\n  \
         sync                refresh release date and derived files (README)\n  \
         set <X.Y.Z>         set an explicit version and sync\n  \
         bump patch|minor|major   increment version and sync\n  \
         release [X.Y.Z]     sync + git commit + tag vX.Y.Z + push"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["status"] => status(),
        ["sync"] => {
            sync(None);
        }
        ["set", version] => {
            sync(Some(version));
        }
        ["bump", level] => bump(level),
        ["release"] => release(None),
        ["release", version] => release(Some(version)),
        _ => print_help(),
    }
}
