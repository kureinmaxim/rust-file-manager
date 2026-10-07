//! Embeds build metadata (git commit/branch/dirty, build date) into the
//! binary as env vars; shown in the web UI footer and the startup log.
//! Also embeds the Telegram Mini App (`miniapp/`) so the server stays a
//! single binary: see `src/miniapp.rs`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// FNV-1a: a stable content hash for ETags (no build dependencies needed).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.ends_with(".md") {
            continue;
        }
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn embed_miniapp() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dir = root.join("miniapp");
    println!("cargo:rerun-if-changed=miniapp");
    let mut files = Vec::new();
    collect(&dir, &mut files);
    files.sort();
    let mut code = String::from("pub static ASSETS: &[Asset] = &[\n");
    for path in files {
        let rel = path
            .strip_prefix(&dir)
            .expect("inside miniapp/")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).expect("read miniapp asset");
        writeln!(
            code,
            "    Asset {{ path: {rel:?}, body: include_bytes!({:?}), content_type: {:?}, etag: \"\\\"{:016x}\\\"\" }},",
            path.display().to_string(),
            content_type(&path),
            fnv1a(&bytes)
        )
        .expect("write to String");
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("miniapp_assets.rs");
    std::fs::write(out, code).expect("write miniapp_assets.rs");
}

fn main() {
    embed_miniapp();
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let build_date = Command::new("date")
        .args(["-u", "+%Y-%m-%d %H:%M UTC"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());

    println!(
        "cargo:rustc-env=BUILD_GIT_COMMIT={commit}{}",
        if dirty { "-dirty" } else { "" }
    );
    println!("cargo:rustc-env=BUILD_GIT_BRANCH={branch}");
    println!("cargo:rustc-env=BUILD_DATE={build_date}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
