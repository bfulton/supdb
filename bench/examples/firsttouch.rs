//! What the first touch of a mapped page-cache page costs its reader, by
//! how the page came into the cache, on the machine the temp directory is
//! on.
//!
//! A page this process wrote is in the cache, and a mapping of it reads it
//! at a cache miss. A page the cache dropped and the kernel read back
//! (`POSIX_FADV_WILLNEED`, `readahead`: nothing in the guest copies it)
//! is resident again, and mapped again once `MADV_POPULATE_READ` has been
//! over it -- no fault of this process's is left to take -- and its first
//! touch still costs: in a guest the frame the device's read filled has no
//! second-stage mapping until the guest itself touches it, and that fault
//! is taken below the guest, on the toucher's CPU clock, with nothing in
//! `/proc` counting it. The price is the host's and moves with the
//! machine's state: 1.2 to 1.5 µs a page here before a restart of the
//! guest and 0.5 after it, against 0.03 for a page never dropped.
//!
//! The probe writes a file, maps it, populates the mapping and times one
//! byte of every eighth page in a scattered order, twice (`written`); then
//! drops the file's cache, reads it back by advice alone, waits for it by
//! `mincore`, populates and times the same touches twice (`read back`),
//! and last a run of pages no pass has touched, in order. The second pass
//! of each is the control: the same touches with nothing left to pay.
//!
//!     cargo run --release --example firsttouch -- [--dir DIR] [--mb N] [--wait SECONDS]
//!
//! `--wait` sleeps between the drop and the read back, for a host that
//! takes freed pages away after a delay.
use std::io::Write as _;
use std::os::unix::io::AsRawFd as _;
use std::path::PathBuf;
use std::time::Instant;

const PAGE: usize = 4096;
const MADV_POPULATE_READ: libc::c_int = 22;

/// This thread's CPU time in milliseconds and its minor and major faults.
fn thread_counts() -> (f64, i64, i64) {
    // SAFETY: plain reads into values on the stack.
    unsafe {
        let mut ts: libc::timespec = std::mem::zeroed();
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts);
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_THREAD, &mut ru);
        (
            ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 / 1e6,
            ru.ru_minflt,
            ru.ru_majflt,
        )
    }
}

/// The pages of `len` bytes at `p` that the page cache holds.
fn resident(p: *mut libc::c_void, len: usize) -> usize {
    let mut vec = vec![0u8; len.div_ceil(PAGE)];
    // SAFETY: `p..p+len` is one live mapping and `vec` has a byte a page.
    if unsafe { libc::mincore(p, len, vec.as_mut_ptr()) } != 0 {
        return 0;
    }
    vec.iter().filter(|b| **b & 1 != 0).count()
}

/// One byte of every `stride`th page read once, the pages taken in a
/// scattered order as scans at random starts would take them.
fn pass(tag: &str, p: *const u8, len: usize, stride: usize) {
    let n = len / PAGE;
    let (cpu0, min0, maj0) = thread_counts();
    let t = Instant::now();
    let (mut i, mut k, mut sum) = (0usize, 0usize, 0u64);
    let mut j = 0;
    while j < n {
        i = (i + 2_654_435_761) % n;
        // SAFETY: `i < n`, so the byte is inside the mapping.
        sum += u64::from(unsafe { std::ptr::read_volatile(p.add(i * PAGE + 17)) });
        k += 1;
        j += stride;
    }
    let us = t.elapsed().as_secs_f64() * 1e6;
    let (cpu1, min1, maj1) = thread_counts();
    std::hint::black_box(sum);
    println!(
        "  {tag:<26} {k:>7} pages {:>8.3} ms wall {:>8.3} ms cpu {:>6.2} us/page  minflt {} majflt {}",
        us / 1e3,
        cpu1 - cpu0,
        us / k as f64,
        min1 - min0,
        maj1 - maj0
    );
}

fn main() {
    let mut dir = std::env::temp_dir();
    let (mut mb, mut wait) = (256usize, 0u64);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| panic!("{a} wants a value"));
        match a.as_str() {
            "--dir" => dir = PathBuf::from(val()),
            "--mb" => mb = val().parse().expect("--mb N"),
            "--wait" => wait = val().parse().expect("--wait SECONDS"),
            _ => panic!("usage: firsttouch [--dir DIR] [--mb N] [--wait SECONDS]"),
        }
    }
    let path = dir.join(format!("supdb-firsttouch-{}", std::process::id()));
    let len = mb << 20;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("the probe's file");
    let mut buf = vec![0u8; 1 << 20];
    let mut x = 12_345u32;
    for _ in 0..mb {
        for w in buf.as_chunks_mut::<4>().0 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *w = x.to_le_bytes();
        }
        file.write_all(&buf).expect("write");
    }
    file.sync_all().expect("fsync");
    let fd = file.as_raw_fd();
    let map = |fd: libc::c_int| -> *mut libc::c_void {
        // SAFETY: a read-only shared mapping of a file held open, checked.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert!(p != libc::MAP_FAILED, "mmap");
        // SAFETY: advice over the mapping just made.
        unsafe { libc::madvise(p, len, libc::MADV_RANDOM) };
        p
    };
    let populate = |p: *mut libc::c_void| {
        let t = Instant::now();
        // SAFETY: advice over a live mapping; it changes no byte.
        let rc = unsafe { libc::madvise(p, len, MADV_POPULATE_READ) };
        println!(
            "  populate_read {} in {:.1} ms",
            if rc == 0 { "ok" } else { "refused" },
            t.elapsed().as_secs_f64() * 1e3
        );
    };

    let p = map(fd);
    println!(
        "written here, never dropped: resident {} of {}",
        resident(p, len),
        len / PAGE
    );
    populate(p);
    pass("first pass", p as *const u8, len, 8);
    pass("second pass, same pages", p as *const u8, len, 8);
    // SAFETY: the mapping made above, unmapped once.
    unsafe { libc::munmap(p, len) };

    // SAFETY: advice on a descriptor this process holds.
    unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) };
    let p = map(fd);
    println!("dropped: resident {}; waiting {wait} s", resident(p, len));
    std::thread::sleep(std::time::Duration::from_secs(wait));
    let t = Instant::now();
    for _ in 0..400 {
        // SAFETY: advice and readahead on a descriptor this process holds.
        unsafe {
            libc::posix_fadvise(fd, 0, len as libc::off_t, libc::POSIX_FADV_WILLNEED);
            let mut off = 0usize;
            while off < len {
                libc::readahead(fd, off as libc::off64_t, 1 << 20);
                off += 1 << 20;
            }
        }
        if resident(p, len) * 100 >= len / PAGE * 99 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    println!(
        "read back by advice: resident {} of {} in {:.1} ms",
        resident(p, len),
        len / PAGE,
        t.elapsed().as_secs_f64() * 1e3
    );
    populate(p);
    pass("first pass", p as *const u8, len, 8);
    pass("second pass, same pages", p as *const u8, len, 8);
    let t = Instant::now();
    let (mut k, mut sum, mut i) = (0usize, 0u64, 3usize);
    while i < len / PAGE {
        // SAFETY: `i` is a page inside the mapping.
        sum += u64::from(unsafe { std::ptr::read_volatile((p as *const u8).add(i * PAGE + 33)) });
        k += 1;
        i += 16;
    }
    std::hint::black_box(sum);
    let us = t.elapsed().as_secs_f64() * 1e6;
    println!(
        "  {:<26} {k:>7} pages {:>8.3} ms wall {:>29.2} us/page",
        "untouched pages, in order",
        us / 1e3,
        us / k as f64
    );
    // SAFETY: the mapping made above, unmapped once.
    unsafe { libc::munmap(p, len) };
    drop(file);
    let _ = std::fs::remove_file(&path);
}
