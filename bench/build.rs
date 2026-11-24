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
    match build_flags() {
        Ok(stamp) => println!("cargo:rustc-env=SUPDB_BUILD={stamp}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    level_check();
    #[cfg(feature = "codecs")]
    lzav();
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

/// The Rust target features each target the suite runs on is held to by
/// `.cargo/config.toml`, by the names `CARGO_CFG_TARGET_FEATURE` and the
/// standard library's detection macros give them: x86-64-v3 with PCLMUL, and
/// ARMv8 with CRC and the crypto extensions. A target that is not here takes
/// rustc's default and is held to nothing. The one list: the build's check
/// below and the binary's check of its host (`env::level_missing`) are both
/// made from it.
const LEVELS: &[(&str, &[&str])] = &[
    (
        "x86_64-unknown-linux-gnu",
        &[
            "avx",
            "avx2",
            "bmi1",
            "bmi2",
            "f16c",
            "fma",
            "lzcnt",
            "movbe",
            "pclmulqdq",
            "popcnt",
            "sse4.2",
            "xsave",
        ],
    ),
    ("aarch64-unknown-linux-gnu", &["aes", "crc", "sha2"]),
    ("aarch64-apple-darwin", &[]),
];

/// The flags every engine of this build was compiled with, as the row's
/// `build` records them: the Rust flags cargo passed and the C and C++ flags
/// `cc` read for the target. Refused where `.cargo/config.toml` did not
/// reach the build -- its flags absent, or the level missing from the Rust
/// side because a `RUSTFLAGS` replaced them -- for a target it names: such a
/// build compiles the C and C++ comparators with `cc`'s frame pointers and the
/// host compiler's protections, and RocksDB without the CRC instruction the
/// engine uses, and its rows would be read as this suite's.
fn build_flags() -> Result<String, String> {
    let target = std::env::var("TARGET").unwrap_or_default();
    let under = target.replace('-', "_");
    let (cflags, cxxflags) = (format!("CFLAGS_{under}"), format!("CXXFLAGS_{under}"));
    for v in [&cflags, &cxxflags] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    println!("cargo:rerun-if-changed=.cargo/config.toml");
    let get = |v: &str| std::env::var(v).unwrap_or_default();
    let rustflags = get("CARGO_ENCODED_RUSTFLAGS").replace('\x1f', " ");
    let stamp = format!(
        "rustflags: {rustflags}; {cflags}: {}; {cxxflags}: {}",
        get(&cflags),
        get(&cxxflags)
    );
    let Some((_, features)) = LEVELS.iter().find(|(t, _)| *t == target) else {
        return Ok(stamp);
    };
    let have = get("CARGO_CFG_TARGET_FEATURE");
    let have: Vec<&str> = have.split(',').collect();
    let missing: Vec<&str> = features
        .iter()
        .copied()
        .filter(|f| !have.contains(f))
        .collect();
    let why = if get(&cflags).is_empty() || get(&cxxflags).is_empty() {
        format!("{cflags} and {cxxflags} are not set")
    } else if !missing.is_empty() {
        format!(
            "the Rust side lacks {} (a RUSTFLAGS in the environment replaces the file's; \
             name the file's flags in it too)",
            missing.join(", ")
        )
    } else {
        return Ok(stamp);
    };
    Err(format!(
        "the suite is built as bench/.cargo/config.toml defines, and {why}. Cargo reads that \
         file from the directory it runs in: run it from bench/, as every script and workflow \
         does, not with --manifest-path from elsewhere."
    ))
}

/// The host's check against the level, written from `LEVELS` for the target:
/// `env::level_missing`, which `bench` asks before anything, and on x86-64 a
/// C constructor that runs before it. A binary of instructions its host
/// lacks dies of an illegal instruction wherever the first one is, which
/// names nothing, and on x86-64 the first one is in RocksDB's C++ static
/// initialisers, which run before `main`: a Nehalem under qemu died in
/// `cache.cc`'s on a VEX `vmovq`. So the constructor runs at the earliest
/// priority a program may use, is compiled for baseline x86-64 whatever
/// `-march` the environment gives (`target("arch=x86-64")`), and names what
/// is missing and exits; `level_missing` calls into it, which is what keeps
/// its object in the link.
fn level_check() {
    let target = std::env::var("TARGET").unwrap_or_default();
    let features = LEVELS
        .iter()
        .find(|(t, _)| *t == target)
        .map_or(&[][..], |(_, f)| *f);
    let x86 = target.starts_with("x86_64");
    let detect = if x86 {
        "is_x86_feature_detected"
    } else {
        "is_aarch64_feature_detected"
    };
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let constructor = x86 && !features.is_empty();
    let mut body = String::new();
    if constructor {
        body.push_str(
            "    // SAFETY: a C function of no arguments that reads the CPU's\n\
             \x20   // identification and returns a count.\n\
             \x20   let _ = unsafe { supdb_bench_level_missing() };\n",
        );
    }
    if features.is_empty() {
        body.push_str("    Vec::new()\n");
    } else {
        body.push_str("    let mut missing = Vec::new();\n");
        for f in features {
            body.push_str(&format!(
                "    if !std::arch::{detect}!(\"{f}\") {{\n        missing.push(\"{f}\");\n    }}\n"
            ));
        }
        body.push_str("    missing\n");
    }
    let decl = if constructor {
        "extern \"C\" {\n    fn supdb_bench_level_missing() -> i32;\n}\n\n"
    } else {
        ""
    };
    let code = format!(
        "{decl}/// The features of the machine level this binary was compiled to\n\
         /// ({target}, `build.rs`'s `LEVELS`) that the host lacks.\n\
         pub fn level_missing() -> Vec<&'static str> {{\n{body}}}\n"
    );
    std::fs::write(out.join("level.rs"), code).expect("level.rs");
    if !constructor {
        return;
    }
    // The feature names `__builtin_cpu_supports` takes, where they differ.
    fn gcc(f: &str) -> &str {
        match f {
            "bmi1" => "bmi",
            "pclmulqdq" => "pclmul",
            f => f,
        }
    }
    let mut checks = String::new();
    for f in features {
        checks.push_str(&format!(
            "    if (!__builtin_cpu_supports(\"{}\")) {{\n        lacks(n++, \"{f}\");\n    }}\n",
            gcc(f)
        ));
    }
    let c = format!(
        "/* Written by build.rs from LEVELS for {target}. */\n\
         #include <string.h>\n\
         #include <unistd.h>\n\
         \n\
         static int counted = -1;\n\
         \n\
         __attribute__((target(\"arch=x86-64\")))\n\
         static void say(const char *s) {{\n\
         \x20   if (write(2, s, strlen(s)) < 0) {{\n\
         \x20   }}\n\
         }}\n\
         \n\
         __attribute__((target(\"arch=x86-64\")))\n\
         static void lacks(int before, const char *feature) {{\n\
         \x20   say(before == 0 ? \"bench: this binary is compiled to a machine level this host lacks:\" : \",\");\n\
         \x20   say(\" \");\n\
         \x20   say(feature);\n\
         }}\n\
         \n\
         __attribute__((constructor(101), target(\"arch=x86-64\")))\n\
         static void supdb_bench_level(void) {{\n\
         \x20   int n = 0;\n\
         \x20   __builtin_cpu_init();\n\
         {checks}\
         \x20   counted = n;\n\
         \x20   if (n > 0) {{\n\
         \x20       say(\"; the suite's level is bench/.cargo/config.toml's\\n\");\n\
         \x20       _exit(2);\n\
         \x20   }}\n\
         }}\n\
         \n\
         int supdb_bench_level_missing(void) {{\n\
         \x20   return counted;\n\
         }}\n"
    );
    let file = out.join("level.c");
    std::fs::write(&file, c).expect("level.c");
    cc::Build::new().file(&file).compile("supdb_bench_level");
}

/// FASTER's C++ store and the shim the `faster` arm calls (`vendor/faster`),
/// built as FASTER's release build builds it: C++17, `-O3`, `NDEBUG`; and
/// with the C++ flags every engine of the suite takes (`.cargo/config.toml`),
/// which `cc` reads from the environment after these.
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
