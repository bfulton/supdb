//! Stamp every row with the revision that produced it.
//!
//! A field built to say which engine produced a measurement once read
//! "unknown" for the whole life of a suite, because nothing set the
//! compile-time variable it read, and a set of committed results drifted 65%
//! away from the engine without anything noticing. The engine and this suite
//! are one repository, so one SHA names both.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // HEAD alone is not enough: a commit on the same branch rewrites the
    // branch's ref file (or packed-refs), not HEAD, and a stamp that only
    // watched HEAD would carry the previous commit into every row after it.
    for p in git_paths() {
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rustc-env=SUPDB_SHA={}", sha());
    println!("cargo:rustc-env=SUPDB_RUSTC={}", rustc());
}

/// The files whose change means HEAD may name a different commit: HEAD, the
/// ref it points at if it is symbolic, and packed-refs -- each only if it
/// exists, because cargo treats a watched path that does not exist as
/// changed on every build, which would re-run this script every time.
/// Empty when git cannot say, in which case the stamp is computed once.
fn git_paths() -> Vec<String> {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) else {
        return Vec::new();
    };
    let mut paths = vec![format!("{dir}/HEAD"), format!("{dir}/packed-refs")];
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        paths.push(format!("{dir}/{r}"));
    }
    paths.retain(|p| std::path::Path::new(p).exists());
    paths
}

/// HEAD of the repository, with `-dirty` when it has uncommitted changes.
///
/// "unknown" only when git cannot answer at all -- a source tarball, say. It
/// is deliberately not a silent empty string: a row whose provenance is
/// missing should say so in the row.
fn sha() -> String {
    let out = Command::new("git").args(["rev-parse", "HEAD"]).output();
    let Ok(out) = out else {
        return "unknown".into();
    };
    if !out.status.success() {
        return "unknown".into();
    }
    let mut s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        return "unknown".into();
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty {
        s.push_str("-dirty");
    }
    s
}

fn rustc() -> String {
    Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}
