//! The process's allocations counted by the thread that made them, for the
//! question the allocator's own figures cannot answer: whose memory an
//! arena that stands free and resident was. Behind the `alloc-census`
//! feature and in no binary the suite measures with: the count is a
//! thread-local read and two atomics on every allocation of every arm.
//!
//! glibc keeps a thread's arena after the thread has ended, with whatever
//! of its top it does not trim, and hands the arena to the next thread
//! that starts. `malloc_info` says how large each arena is and how much of
//! it is free; it does not say which threads filled it. This does: each
//! thread's peak of bytes it allocated and nobody had yet freed, by the
//! thread's name, with whether the thread is still there. A thread that
//! frees what another allocated takes the bytes off its own count, so a
//! thread's figure is exact where it frees its own, which a job's thread
//! and a reader's do, and an upper bound on its arena's high water
//! otherwise.
//!
//! A thread with no name of its own carries the name of the thread that
//! started it, as the kernel has it: a seal's thread reads as the
//! process's main thread and a merge's as `supdb-maint`. The suite's
//! reader threads are named `bench-reader` under the feature
//! (`run::threaded`), so that what is left under the process's name beside
//! its first thread is the engine's.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicU64, AtomicUsize, Ordering};

/// The counting allocator: every call passed to the system's unchanged.
pub struct Counting;

/// The threads counted apart; the ones past it share the last place.
const T: usize = 8192;

static LIVE: [AtomicIsize; T] = [const { AtomicIsize::new(0) }; T];
static PEAK: [AtomicIsize; T] = [const { AtomicIsize::new(0) }; T];
static TID: [AtomicU64; T] = [const { AtomicU64::new(0) }; T];
static NAME_LO: [AtomicU64; T] = [const { AtomicU64::new(0) }; T];
static NAME_HI: [AtomicU64; T] = [const { AtomicU64::new(0) }; T];
static NEXT: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    /// This thread's place, zero until its first allocation.
    static IDX: Cell<usize> = const { Cell::new(0) };
}

/// This thread's place in the tables, taken at its first allocation with
/// its kernel id and its name as they stand then.
fn idx() -> usize {
    IDX.with(|c| {
        let mut i = c.get();
        if i == 0 {
            i = NEXT.fetch_add(1, Ordering::Relaxed).min(T - 1);
            c.set(i);
            let mut name = [0u8; 16];
            // SAFETY: the thread's name into a buffer of the sixteen bytes
            // the call writes.
            unsafe { libc::prctl(libc::PR_GET_NAME, name.as_mut_ptr()) };
            NAME_LO[i].store(
                u64::from_le_bytes(name[..8].try_into().unwrap()),
                Ordering::Relaxed,
            );
            NAME_HI[i].store(
                u64::from_le_bytes(name[8..].try_into().unwrap()),
                Ordering::Relaxed,
            );
            // SAFETY: a system call with no argument.
            let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u64;
            TID[i].store(tid, Ordering::Relaxed);
        }
        i
    })
}

fn add(n: isize) {
    let i = idx();
    let live = LIVE[i].fetch_add(n, Ordering::Relaxed) + n;
    if n > 0 {
        PEAK[i].fetch_max(live, Ordering::Relaxed);
    }
}

// SAFETY: every call is passed to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            add(l.size() as isize);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            add(l.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(p, l) };
        add(-(l.size() as isize));
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            add(new as isize - l.size() as isize);
        }
        q
    }
}

/// One line on stderr under `tag`: for each thread name, the threads still
/// there and the threads gone, with the largest peak among each, the sum
/// of their peaks and what they hold now; then the gone threads' peaks of
/// a megabyte and more one by one, largest first, which is what an arena
/// left whole and free is to be matched against.
pub fn report(tag: &str, arm: &str, point: &str) {
    let here: std::collections::HashSet<u64> = std::fs::read_dir("/proc/self/task")
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_string_lossy().parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let mb = |b: isize| b as f64 / (1u64 << 20) as f64;
    // (threads, largest peak, peaks summed, live summed) by name and by
    // whether the thread has gone.
    let mut by: std::collections::BTreeMap<(String, bool), (usize, isize, isize, isize)> =
        Default::default();
    let mut gone_peaks: Vec<(isize, String)> = Vec::new();
    for i in 1..NEXT.load(Ordering::Relaxed).min(T) {
        let mut name = [0u8; 16];
        name[..8].copy_from_slice(&NAME_LO[i].load(Ordering::Relaxed).to_le_bytes());
        name[8..].copy_from_slice(&NAME_HI[i].load(Ordering::Relaxed).to_le_bytes());
        let name = String::from_utf8_lossy(&name)
            .trim_end_matches('\0')
            .to_string();
        let gone = !here.contains(&TID[i].load(Ordering::Relaxed));
        let (peak, live) = (
            PEAK[i].load(Ordering::Relaxed),
            LIVE[i].load(Ordering::Relaxed),
        );
        let e = by.entry((name.clone(), gone)).or_insert((0, 0, 0, 0));
        e.0 += 1;
        e.1 = e.1.max(peak);
        e.2 += peak;
        e.3 += live;
        if gone && peak >= 1 << 20 {
            gone_peaks.push((peak, name));
        }
    }
    let groups: Vec<String> = by
        .iter()
        .filter(|(_, v)| v.1 >= 1 << 20)
        .map(|((name, gone), v)| {
            format!(
                "{name}{} n {} most {:.1} sum {:.1} live {:.1}",
                if *gone { " gone" } else { "" },
                v.0,
                mb(v.1),
                mb(v.2),
                mb(v.3)
            )
        })
        .collect();
    gone_peaks.sort_by(|a, b| b.0.cmp(&a.0));
    let peaks: Vec<String> = gone_peaks
        .iter()
        .take(24)
        .map(|(p, n)| format!("{n} {:.1}", mb(*p)))
        .collect();
    eprintln!(
        "{tag} {arm} {point} | {} | gone peaks: {}",
        groups.join(" | "),
        peaks.join(", ")
    );
}
