//! What a store's background jobs hold in the allocator while they run: the
//! peak of the live bytes by phase of a store's life and by thread, and the
//! large allocations of the flush after scattered updates with where each
//! was made. A measurement and no check, so its one test is ignored:
//!
//! ```text
//! RUST_BACKTRACE=1 cargo test --profile checked --test alloc_peaks -- --ignored --nocapture
//! ```
//!
//! Its own test binary, as `tests/leak.rs` is, because it counts every
//! allocation the process makes. The memtables' blocks are mapped by the
//! engine and are not in the count (`Options::blocks_mapped`); what is in it
//! is every `Vec` a seal, a merge, a run's close or a form file's build
//! holds, which is what the allocator keeps in the arena of a thread that
//! has ended (`docs/engine.md`, which form a workload calls for).
//! `PROBE_N` is the store's key count, six hundred thousand unless set;
//! `PROBE_BIG` the size an allocation is printed from, two megabytes unless
//! set; `PROBE_JOBS_MAPPED=1` runs the store under `Options::jobs_mapped`
//! and `PROBE_MEMO_SLAB=1` under `Options::zpages_memo_slab`, whose
//! mappings the allocator never sees: the bytes mapped at each phase's
//! end are printed beside its line.
#![cfg(target_os = "linux")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};
use supdb::{Db, Options, ZPages, ZStride};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
static BIG: AtomicIsize = AtomicIsize::new(0);
static BIG_FROM: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(2 << 20);

const T: usize = 4096;
static TLIVE: [AtomicIsize; T] = [const { AtomicIsize::new(0) }; T];
static TPEAK: [AtomicIsize; T] = [const { AtomicIsize::new(0) }; T];
static TNAME: [std::sync::atomic::AtomicU64; T] =
    [const { std::sync::atomic::AtomicU64::new(0) }; T];
static TNAME2: [std::sync::atomic::AtomicU64; T] =
    [const { std::sync::atomic::AtomicU64::new(0) }; T];
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
thread_local! { static IDX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn idx() -> usize {
    IDX.with(|c| {
        let mut i = c.get();
        if i == 0 {
            i = NEXT.fetch_add(1, Ordering::Relaxed).min(T - 1);
            c.set(i);
            let mut name = [0u8; 16];
            // SAFETY: the thread's name into a buffer of the sixteen bytes the call writes.
            unsafe { libc::prctl(libc::PR_GET_NAME, name.as_mut_ptr()) };
            TNAME[i].store(
                u64::from_le_bytes(name[..8].try_into().unwrap()),
                Ordering::Relaxed,
            );
            TNAME2[i].store(
                u64::from_le_bytes(name[8..].try_into().unwrap()),
                Ordering::Relaxed,
            );
        }
        i
    })
}

static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRACED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
thread_local! { static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

fn big(n: isize) {
    if n < BIG_FROM.load(Ordering::Relaxed) as isize || !TRACE.load(Ordering::Relaxed) {
        return;
    }
    if INSIDE.with(|c| c.replace(true)) {
        return;
    }
    if TRACED.fetch_add(1, Ordering::Relaxed) < 60 {
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> = bt
            .lines()
            .filter(|l| l.contains("supdb::") && !l.contains("alloc_peaks"))
            .map(|l| l.trim())
            .take(7)
            .collect();
        let i = idx();
        let mut name = [0u8; 8];
        name.copy_from_slice(&TNAME[i].load(Ordering::Relaxed).to_le_bytes());
        eprintln!(
            "probe-big {:6.2} MB on {} (thread live {:6.2} MB): {}",
            n as f64 / 1e6,
            String::from_utf8_lossy(&name),
            TLIVE[i].load(Ordering::Relaxed) as f64 / 1e6,
            frames.join(" < ")
        );
    }
    INSIDE.with(|c| c.set(false));
}

fn add(n: isize) {
    big(n);
    let now = LIVE.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(now, Ordering::Relaxed);
    let i = idx();
    let t = TLIVE[i].fetch_add(n, Ordering::Relaxed) + n;
    TPEAK[i].fetch_max(t, Ordering::Relaxed);
}

fn sub(n: isize) {
    LIVE.fetch_sub(n, Ordering::Relaxed);
    TLIVE[idx()].fetch_sub(n, Ordering::Relaxed);
}

/// The threads' peaks since the last call, by name, summed over the threads of a name and the most of one.
fn threads(what: &str) {
    let mut by: std::collections::BTreeMap<String, (usize, isize, isize)> = Default::default();
    for i in 1..NEXT.load(Ordering::Relaxed).min(T) {
        let p = TPEAK[i].swap(TLIVE[i].load(Ordering::Relaxed), Ordering::Relaxed);
        let mut name = [0u8; 16];
        name[..8].copy_from_slice(&TNAME[i].load(Ordering::Relaxed).to_le_bytes());
        name[8..].copy_from_slice(&TNAME2[i].load(Ordering::Relaxed).to_le_bytes());
        let n = String::from_utf8_lossy(&name)
            .trim_end_matches('\0')
            .to_string();
        let e = by.entry(n).or_insert((0, 0, 0));
        if p > (1 << 20) {
            e.0 += 1;
            e.1 += p;
            e.2 = e.2.max(p);
        }
    }
    for (n, (k, sum, most)) in by {
        if k > 0 {
            eprintln!("probe     {what}: thread {n:16} over 1 MB in {k:3} threads, most {:6.2} MB, mean {:6.2} MB", most as f64 / 1e6, sum as f64 / 1e6 / k as f64);
        }
    }
}

// SAFETY: every call is passed to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            add(l.size() as isize);
            BIG.fetch_max(l.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            add(l.size() as isize);
            BIG.fetch_max(l.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        sub(l.size() as isize);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            add(new as isize - l.size() as isize);
            BIG.fetch_max(new as isize, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn phase<T>(what: &str, n: usize, f: impl FnOnce() -> T) -> T {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    BIG.store(0, Ordering::Relaxed);
    let t = f();
    let (peak, live, big) = (
        PEAK.load(Ordering::Relaxed),
        LIVE.load(Ordering::Relaxed),
        BIG.load(Ordering::Relaxed),
    );
    eprintln!(
        "probe {what:28} peak +{:7.2} MB ({:5.1} B/key)  kept +{:6.2} MB  largest alloc {:6.2} MB  mapped {:6.2} MB",
        (peak - base) as f64 / 1e6,
        (peak - base) as f64 / n as f64,
        (live - base) as f64 / 1e6,
        big as f64 / 1e6,
        Db::mapped_bytes() as f64 / 1e6
    );
    threads(what.trim());
    t
}

/// A value read: nothing kept of it.
fn got_len(v: &[u8]) {
    std::hint::black_box(v.len());
}

fn key(i: usize) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

#[test]
#[ignore = "a measurement: run it by name with --ignored --nocapture"]
fn the_peaks_by_phase_and_thread() {
    let n: usize = std::env::var("PROBE_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(600_000);
    let dir = std::env::temp_dir().join(format!("supdb-next-alloc-peaks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let val = vec![0x5au8; 100];
    // `PROBE_JOBS_MAPPED=1`: the jobs' large arrays as the engine's
    // mappings (`Options::jobs_mapped`), which leave the allocator's count.
    let jobs_mapped = std::env::var("PROBE_JOBS_MAPPED").is_ok_and(|v| v == "1");
    // `PROBE_MEMO_SLAB=1`: the handles' memos in slabs of their own
    // (`Options::zpages_memo_slab`), mappings too.
    let memo_slab = std::env::var("PROBE_MEMO_SLAB").is_ok_and(|v| v == "1");
    // `PROBE_BIG`: the size from which an allocation of the traced flush
    // is printed with where it was made, two megabytes unless set.
    if let Some(b) = std::env::var("PROBE_BIG").ok().and_then(|s| s.parse().ok()) {
        BIG_FROM.store(b, Ordering::Relaxed);
    }
    let off = || Options {
        zpages: ZPages::Off,
        jobs_mapped,
        ..Options::default()
    };
    let mut db = Db::create(&dir, off()).unwrap();
    phase("ordered load, commits", n, || {
        for chunk in (0..n).collect::<Vec<_>>().chunks(1000) {
            for &i in chunk {
                db.append(&key(i), &val);
            }
            db.commit().unwrap();
        }
    });
    phase("  its flush and settle", n, || {
        db.flush().unwrap();
        db.settle().unwrap();
    });
    // A hash table's seal: every key updated once, scattered.
    let m = n / 2;
    phase("updates, commits", m, || {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut i = 0;
        while i < m {
            for _ in 0..1000 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let k = (x % n as u64) as usize;
                db.delete(&key(k));
                db.append(&key(k), &val);
                i += 1;
            }
            db.commit().unwrap();
        }
    });
    TRACE.store(true, Ordering::Relaxed);
    phase("  their flush and settle", m, || {
        db.flush().unwrap();
        db.settle().unwrap();
    });
    TRACE.store(false, Ordering::Relaxed);
    db.close().unwrap();
    for (what, stride) in [
        ("block builds", ZStride::Off),
        ("stride builds", ZStride::Forced),
    ] {
        let o = Options {
            zpages: ZPages::Always,
            zpages_min_bytes: 0,
            zpages_stride: stride,
            zpages_stride_room: false,
            jobs_mapped,
            zpages_memo_slab: memo_slab,
            ..Options::default()
        };
        // The form files of the run before, gone: each run builds its own.
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.file_name().unwrap().to_string_lossy().starts_with("zpg-") {
                std::fs::remove_file(p).unwrap();
            }
        }
        let mut db = phase(&format!("open for {what}"), n, || {
            Db::open(&dir, o).unwrap()
        });
        phase(what, n, || db.zpages_settle().unwrap());
        let c = db.zpage_counts();
        eprintln!(
            "probe   {} form files, {} stride, {} partitions",
            c.form_files, c.stride_files, c.partitions
        );
        // What a reader handle's thread holds: three threads, a handle
        // each, reading uniformly through the form, and then scanning from
        // uniform starts over a store with a few thousand keys unsealed.
        // Each thread drops its handle as it ends, and what it peaked at is
        // what its arena keeps.
        let readers = |db: &Db, what: &str, f: &(dyn Fn(&supdb::db::Reader, u64) + Sync)| {
            phase(what, n, || {
                std::thread::scope(|s| {
                    for t in 0..3u64 {
                        let r = db.reader().unwrap();
                        std::thread::Builder::new()
                            .name("probe-reader".into())
                            .spawn_scoped(s, move || f(&r, t))
                            .unwrap();
                    }
                });
            });
        };
        let next = |x: &mut u64| {
            *x ^= *x << 13;
            *x ^= *x >> 7;
            *x ^= *x << 17;
            *x
        };
        readers(&db, &format!("  point reads, {what}"), &|r, t| {
            let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ (t + 1);
            let mut got = 0u64;
            for _ in 0..200_000 {
                let k = (next(&mut x) % n as u64) as usize;
                got += r.read_all(&key(k), &mut |v: &[u8]| got_len(v)).unwrap();
            }
            assert_eq!(got, 200_000);
        });
        let mut x = 0xD1B5_4A32_D192_ED03u64;
        for _ in 0..5 {
            for _ in 0..1000 {
                let k = (next(&mut x) % n as u64) as usize;
                db.delete(&key(k));
                db.append(&key(k), &val);
            }
            db.commit().unwrap();
        }
        readers(&db, &format!("  scans, {what}"), &|r, t| {
            let mut x = 0x2545_F491_4F6C_DD1Du64 ^ (t + 1);
            let mut got = 0u64;
            for _ in 0..20_000 {
                let k = (next(&mut x) % n as u64) as usize;
                r.scan(&key(k), 100, &mut |_: &[u8], v: &[u8]| {
                    got += v.len() as u64;
                })
                .unwrap();
            }
            assert!(got > 0);
        });
        db.close().unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
