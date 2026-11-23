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
    // Nor are the refs enough. Editing the sources moves neither HEAD nor a
    // ref, so this script did not re-run, cargo replayed the stamp it had,
    // and the row named a commit whose code it was not built from with the
    // `-dirty` below never firing -- which is how an A/B of two engine
    // revisions wrote two rows bearing one SHA. The stamp describes the
    // sources, so the sources are what it watches -- and the lockfiles, since
    // a dependency moved by `cargo update` alone rebuilds what is measured.
    for p in [
        "../src",
        "src",
        "../Cargo.toml",
        "Cargo.toml",
        "../Cargo.lock",
        "Cargo.lock",
    ] {
        if std::path::Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    println!("cargo:rustc-env=SUPDB_SHA={}", sha());
    println!("cargo:rustc-env=SUPDB_RUSTC={}", rustc());
    #[cfg(feature = "codecs")]
    lzav();
    #[cfg(feature = "enc")]
    enc_shims();
    // FASTER is a comparator like LMDB and RocksDB, built wherever its C++
    // store is: Linux on x86-64, the one platform it supports that the
    // suite runs on. The arm is behind `cfg(faster)`, set here, so a build
    // on macOS or ARM has every other arm and no feature to remember.
    println!("cargo:rustc-check-cfg=cfg(faster)");
    let target = |v: &str| std::env::var(v).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "linux" && target("CARGO_CFG_TARGET_ARCH") == "x86_64" {
        faster();
        println!("cargo:rustc-cfg=faster");
    }
}

/// The encrypted comparators' ciphers (`shim/`), each linked against the
/// host's OpenSSL: LMDB's page hook under AES-256-GCM (`lmdb_enc.c`), and
/// RocksDB's encrypted Env under AES-256-CTR (`rocks_enc.cc`), compiled
/// against the headers of the RocksDB the `rocksdb` crate builds --
/// `librocksdb-sys` names its directory to its direct dependents -- at
/// that build's language level and with its `NDEBUG`.
#[cfg(feature = "enc")]
fn enc_shims() {
    println!("cargo:rerun-if-changed=shim/lmdb_enc.c");
    println!("cargo:rerun-if-changed=shim/rocks_enc.cc");
    cc::Build::new()
        .file("shim/lmdb_enc.c")
        .compile("supdb_lmdb_enc");
    let root = std::env::var("DEP_ROCKSDB_CARGO_MANIFEST_DIR")
        .expect("librocksdb-sys says where its sources are");
    cc::Build::new()
        .cpp(true)
        .flag_if_supported("-std=c++17")
        .define("NDEBUG", Some("1"))
        .file("shim/rocks_enc.cc")
        .include(format!("{root}/rocksdb/include"))
        .compile("supdb_rocks_enc");
    println!("cargo:rustc-link-lib=crypto");
}

/// FASTER's C++ store and the shim the `faster` arm calls (`vendor/faster`),
/// built as FASTER's release build builds it: C++17, `-O3`, `NDEBUG`.
///
/// FASTER is Microsoft's and neither a crate nor this repository's:
/// `scripts/faster.sh` fetches its sources at the commit `vendor/faster/PIN`
/// names and records what it fetched in `vendor/faster/COMMIT`, as
/// `scripts/libclang.sh` readies a host for RocksDB's bindgen. A build
/// without them, or with another commit's, is a failed build and says what
/// to run: an arm built from some other FASTER would post rates like any
/// other.
fn faster() {
    println!("cargo:rerun-if-changed=vendor/faster");
    let pin = include_str!("vendor/faster/PIN").trim();
    let have = std::fs::read_to_string("vendor/faster/COMMIT").unwrap_or_default();
    let have = have.trim();
    if have != pin || !std::path::Path::new("vendor/faster/src/core/faster.h").exists() {
        eprintln!(
            "error: the suite's `faster` arm needs FASTER's C++ sources at commit {pin} under \
             bench/vendor/faster/src, and {}. Run `sh bench/scripts/faster.sh` (or \
             `sh scripts/deps.sh`), which fetches them and installs the libraries they link.",
            if have.is_empty() {
                "none has been fetched".to_string()
            } else if have != pin {
                format!("the ones there are commit {have}")
            } else {
                "the ones there are incomplete".to_string()
            }
        );
        std::process::exit(1);
    }
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .opt_level(3)
        .define("NDEBUG", None)
        .warnings(false)
        .include("vendor/faster/src")
        .file("vendor/faster/shim.cc")
        .file("vendor/faster/src/core/address.cc")
        .file("vendor/faster/src/core/lss_allocator.cc")
        .file("vendor/faster/src/core/thread.cc")
        .file("vendor/faster/src/environment/file_linux.cc")
        .compile("supdb_faster");
    for lib in ["aio", "tbb", "uuid", "stdc++fs", "pthread"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}

/// LZAV for the codec probe (`examples/zcodec.rs`), from upstream's header
/// in `vendor/lzav`, at the optimization the release profile asks for.
#[cfg(feature = "codecs")]
fn lzav() {
    println!("cargo:rerun-if-changed=vendor/lzav");
    cc::Build::new()
        .file("vendor/lzav/lzav.c")
        .include("vendor/lzav")
        .compile("supdb_lzav");
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
