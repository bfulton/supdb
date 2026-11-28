//! The device's own counter, which the ordered load's device bytes are read
//! from (`env::device_stat_of`, `env::device_written_bytes`), held to two
//! writes whose answer is known: one the device takes, where the counter
//! and the process's own count agree, and one it never sees, where the
//! process counts what it dirtied and the device writes nothing of it.
//!
//! A binary of its own: the counter is the device's, every writer's at
//! once, and any test beside this one that writes a store would be counted
//! in it.

/// The process's `write_bytes` and `cancelled_write_bytes` from
/// `/proc/self/io`: what it dirtied, and what of that was dropped dirty.
#[cfg(target_os = "linux")]
fn process_io() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/io").unwrap();
    let field = |k: &str| -> u64 {
        s.lines()
            .find_map(|l| l.strip_prefix(k))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(|| panic!("/proc/self/io has no {k}"))
    };
    (field("write_bytes:"), field("cancelled_write_bytes:"))
}

/// `bytes` written to a new file at `path` a mebibyte at a time.
#[cfg(target_os = "linux")]
fn write_file(path: &std::path::Path, bytes: usize) -> std::fs::File {
    use std::io::Write as _;
    let mut f = std::fs::File::create(path).unwrap();
    let piece = vec![0x5au8; 1 << 20];
    for _ in 0..bytes / piece.len() {
        f.write_all(&piece).unwrap();
    }
    f
}

#[cfg(target_os = "linux")]
#[test]
fn the_devices_counter_takes_what_reached_the_device_and_nothing_dropped_before_it() {
    use supdb_bench::env::{device_stat_of, device_written_bytes, sync_filesystem};
    const N: usize = 32 << 20;
    let dir = std::env::temp_dir().join(format!("supdb-devbytes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The premise: the store's directory is on a device the kernel counts.
    // A run whose temp directory is not would record no device bytes at
    // all, and this test would prove nothing about the one that does.
    let stat = device_stat_of(&dir).unwrap_or_else(|| {
        panic!(
            "{} is on no block device the kernel counts (an overlay or a tmpfs): \
             the load's device bytes cannot be read here",
            dir.display()
        )
    });
    let device = || device_written_bytes(&stat).unwrap();
    let n = N as u64;

    // Written and synced: the device takes every byte, and the process
    // counted every byte as it dirtied it. The device writes the file's
    // metadata and the journal's blocks beside it, which the process does
    // not count, and anybody else's writes in the window; a quarter of
    // the file is far more than either.
    sync_filesystem(&dir).unwrap();
    let (d0, (w0, c0)) = (device(), process_io());
    write_file(&dir.join("kept"), N).sync_all().unwrap();
    sync_filesystem(&dir).unwrap();
    let (d1, (w1, c1)) = (device(), process_io());
    let (dev, proc_) = (d1 - d0, w1 - w0);
    eprintln!("kept: device {dev} process {proc_} cancelled {}", c1 - c0);
    // The file's pages, and the filesystem's metadata blocks the process
    // dirtied allocating them (bitmaps, the inode's table), which it is
    // charged for as it is for its own pages: six pages of ext4's here.
    assert!(
        (n..n + n / 64).contains(&proc_),
        "the process dirtied {proc_} for {n} written"
    );
    assert!(dev >= n, "the device wrote {dev} of the {n} synced");
    assert!(dev < n + n / 4, "the device wrote {dev} for {n} synced");

    // Written and dropped before any writeback: the process counted the
    // bytes as it dirtied them and the truncation as cancelled, and the
    // device wrote none of them -- the case the process's count gets
    // wrong, and the one a sync before the reading must not turn into a
    // write.
    let (d2, (w2, c2)) = (device(), process_io());
    drop(write_file(&dir.join("dropped"), N));
    std::fs::remove_file(dir.join("dropped")).unwrap();
    sync_filesystem(&dir).unwrap();
    let (d3, (w3, c3)) = (device(), process_io());
    let (dev, proc_, cancelled) = (d3 - d2, w3 - w2, c3 - c2);
    eprintln!("dropped: device {dev} process {proc_} cancelled {cancelled}");
    assert!(
        (n..n + n / 64).contains(&proc_),
        "the process dirtied {proc_} for {n} written"
    );
    assert_eq!(
        cancelled, n,
        "the premise: nothing of the file was written back before its removal"
    );
    assert!(
        dev < n / 4,
        "the device wrote {dev} of a file it was never given"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Off Linux the counter is not read, and the load records no device
/// bytes rather than another quantity under their name.
#[cfg(not(target_os = "linux"))]
#[test]
fn off_linux_no_device_is_read() {
    let dir = std::env::temp_dir();
    assert!(supdb_bench::env::device_stat_of(&dir).is_none());
}
