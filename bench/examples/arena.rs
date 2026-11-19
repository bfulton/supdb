//! What glibc keeps of a thread's arena after the thread has freed
//! everything and ended, and what gives it back: the allocator's side of
//! the question a store under a memory bound asks about every thread it
//! starts and every reader thread its caller does.
//!
//! ```text
//! cargo run --release --example arena
//! ```
//!
//! The mmap threshold starts at 128 KB and rises to the size of the
//! largest chunk the process has mapped and freed, and the trim threshold
//! to twice that; a thread's arena gives its top back only once the top is
//! the trim threshold or more. So the first step raises both as a store's
//! process has them risen -- one chunk of 9.3 MB mapped and freed, a seal's
//! sort array at three hundred and ninety thousand keys -- and each step
//! after it is a thread that holds some megabytes in chunks of one size,
//! frees them all and ends, with the process's anonymous bytes read from
//! the kernel after it. Then `malloc_trim`, and then the trim threshold
//! set low, each followed by the same thread again.
//!
//! glibc only: the calls are its own, and the answer is about it.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn main() {
    fn anon_kb() -> u64 {
        let text = std::fs::read_to_string("/proc/self/status").unwrap();
        text.lines()
            .find_map(|l| l.strip_prefix("RssAnon:"))
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
            .unwrap()
    }
    /// A thread that holds `mb` megabytes in chunks of `chunk` bytes, every
    /// byte written, frees them all and ends.
    fn thread(mb: usize, chunk: usize) {
        std::thread::spawn(move || {
            let n = (mb << 20) / chunk;
            // SAFETY: each chunk is allocated, written through and freed
            // once, by this thread, with the allocator's own calls.
            unsafe {
                let p = libc::malloc(n * std::mem::size_of::<*mut u8>()) as *mut *mut u8;
                for i in 0..n {
                    let c = libc::malloc(chunk) as *mut u8;
                    std::ptr::write_bytes(c, 1, chunk);
                    p.add(i).write(c);
                }
                for i in 0..n {
                    libc::free(p.add(i).read() as *mut libc::c_void);
                }
                libc::free(p as *mut libc::c_void);
            }
        })
        .join()
        .unwrap();
        println!(
            "a thread held {mb:2} MB in chunks of {chunk:7}, freed all and ended: {:6} KB anonymous",
            anon_kb()
        );
    }
    println!("at the start: {} KB anonymous", anon_kb());
    // SAFETY: one chunk allocated, written through and freed.
    unsafe {
        let big = libc::malloc(9_300_000) as *mut u8;
        std::ptr::write_bytes(big, 1, 9_300_000);
        libc::free(std::hint::black_box(big) as *mut libc::c_void);
    }
    println!(
        "a chunk of 9.3 MB mapped and freed, the trim threshold at 18.6 MB: {} KB",
        anon_kb()
    );
    for (mb, chunk) in [(8, 4096), (16, 4096), (18, 4096), (20, 4096), (8, 4096)] {
        thread(mb, chunk);
    }
    for (mb, chunk) in [(16, 2 << 20), (8, 1 << 20)] {
        thread(mb, chunk);
    }
    // SAFETY: the allocator's own call.
    unsafe { libc::malloc_trim(0) };
    println!("after malloc_trim(0): {} KB", anon_kb());
    thread(16, 4096);
    // SAFETY: a tunable of the allocator, set under its own lock.
    unsafe { libc::mallopt(libc::M_TRIM_THRESHOLD, 128 << 10) };
    println!("the trim threshold set to 128 KB");
    thread(8, 4096);
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn main() {
    eprintln!("arena: glibc's arenas are the question, and this is not glibc");
}
