//! One open of a store a killed writer left, timed: the recovery
//! workloads' open (`recover`, `recover-shuffled`) on its own, so that it
//! can be run under `strace -f -T -y` or `perf record` with nothing else
//! in the process.
//!
//!   bench crash-load SPEC          # until it prints its line; kill -9 it
//!   cp -r STORE COPY               # an open recovers, so each open takes a copy
//!   reopen ARM COPY [MAP_GB]
//!
//! The spec is the one a recovery pass writes (`run::crash_load`): the arm,
//! the directory, the size, the map's gigabytes, the batch, the value's
//! size and compressibility, and whether the load is shuffled. The line
//! printed is the open's wall and the process's CPU over it, user and
//! system, with its faults: an open slower than its CPU waited, on the
//! device or on a thread it joined.
use std::path::Path;
use std::time::Instant;
use supdb_bench::engines;

/// The process's user and system CPU in milliseconds, and its minor and
/// major faults, threads that have exited included.
fn usage() -> (f64, f64, i64, i64) {
    // SAFETY: `getrusage` fills the struct it is handed and reads nothing
    // else; a zeroed `rusage` is a valid one to hand it.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
    (
        ms(u.ru_utime),
        ms(u.ru_stime),
        u.ru_minflt as i64,
        u.ru_majflt as i64,
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(arm), Some(dir)) = (args.first(), args.get(1)) else {
        eprintln!("reopen ARM DIR [MAP_GB]");
        std::process::exit(2);
    };
    let map_gb: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4);
    let before = usage();
    let t = Instant::now();
    let e = match engines::reopen(arm, Path::new(dir), map_gb) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("reopen: {e}");
            std::process::exit(1);
        }
    };
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let after = usage();
    println!(
        "reopen {arm} open_ms {open_ms:.2} user_ms {:.2} sys_ms {:.2} minflt {} majflt {}",
        after.0 - before.0,
        after.1 - before.1,
        after.2 - before.2,
        after.3 - before.3,
    );
    drop(e);
}
