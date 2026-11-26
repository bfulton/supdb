//! What the runner's release of freed memory returns, in a binary with no
//! other test in it (`run::release_freed_memory`).
//!
//! The release is `malloc_trim` of every arena, the process's and not the
//! caller's, so any test beside this one that runs a pass gives the freed
//! blocks back before this one has counted them. In the library's binary a
//! lock held this test apart from the tests that run a pass, and it held
//! only the tests that took it: one that ran a pass without it
//! (`a_bounded_passes_start_enters_the_row`) trimmed between this test's
//! free and its first read, and the blocks read as never resident -- 349 MB
//! resident at the free and 27 at the read, once the pass was started
//! inside that window on purpose. A binary of its own excludes every test
//! whether or not it remembers to.

/// This process's resident bytes, as `/proc/self/statm` has them.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn resident_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
    let pages: u64 = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
    // SAFETY: sysconf reads a constant of the system.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    pages * u64::try_from(page).unwrap()
}

/// The shape that filled the full run's memory: a thread allocates small
/// blocks in its own arena, frees them, and exits, with one block allocated
/// last still live so the arena cannot give its heap back from the top. The
/// freed memory stays resident until the runner releases it -- the first
/// assertion is that the test reached that case at all, the second that the
/// release returns most of it.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn a_dropped_stores_freed_memory_is_returned_before_the_next_pass() {
    const BLOCKS: usize = 3_000_000;
    const SIZE: usize = 100;
    let held = BLOCKS * SIZE;
    let pin = std::thread::spawn(|| {
        let mut v: Vec<Box<[u8; SIZE]>> = Vec::with_capacity(BLOCKS);
        for i in 0..BLOCKS {
            v.push(Box::new([i as u8; SIZE]));
        }
        let pin = Box::new([1u8; SIZE]);
        std::hint::black_box(&v);
        drop(v);
        pin
    })
    .join()
    .unwrap();
    let before = resident_bytes();
    supdb_bench::run::release_freed_memory();
    let after = resident_bytes();
    assert!(
        before >= held as u64,
        "the freed blocks were not resident to begin with: {before} bytes, {held} allocated"
    );
    assert!(
        before - after >= held as u64 / 2,
        "released {} of the {held} bytes freed ({before} -> {after})",
        before.saturating_sub(after)
    );
    drop(pin);
}
