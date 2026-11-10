//! What a commit's fdatasync costs appended against pre-written, by the
//! commit's size, on the device the temp directory is on.
//!
//! A log grown by its appends pays a journal commit at every fdatasync, for
//! the file's growth; a log pre-written in zeros ahead of its tail pays one
//! at each pre-write and writes every byte twice, the zeros and then the
//! frames over them. Which is cheaper turns on how many commits a megabyte
//! of the log holds, and `Options::wal_ahead_steps` gates the pre-write on
//! the commit's size by this probe's crossing: the write mixes' commits of
//! a hundred writes, seventeen kilobytes, are on one side of it and the
//! loads' and bursts' of a thousand, a hundred and thirty, on the other.
//!
//! For each commit size, six mebibytes of commits of that size, each
//! written at the tail and fdatasynced, into a fresh file: appended; and
//! pre-written a step past the end, in page-sized writes of zeros as the
//! engine writes them, whenever a commit does not fit what is pre-written,
//! with steps of 256 KiB and of a mebibyte. The shapes alternate their
//! order round by round and the median round is printed as milliseconds a
//! mebibyte of log, the pre-written shapes beside it as a multiple of the
//! appended log's rate. A sitting reads the device at one of its speeds,
//! so two sittings an hour apart are the measurement, not one.
//!
//!     cargo run --release --example walsync -- [--dir DIR] [--rounds N] [--sizes BYTES,BYTES,...]
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

const PAGE: u64 = 4096;
const LOG: u64 = 6 << 20;

/// One log file under a pre-write step; zero appends.
struct Log {
    file: File,
    path: PathBuf,
    written: u64,
    extent: u64,
    step: u64,
}

impl Log {
    fn create(dir: &Path, step: u64) -> Log {
        let path = dir.join(format!("walsync-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .expect("the probe's file");
        file.write_all_at(b"SUPDBWL\x04", 0).expect("the header");
        Log {
            file,
            path,
            written: 8,
            extent: 8,
            step,
        }
    }

    /// One commit: the pre-write where the step says, the frames at the
    /// tail, the fdatasync.
    fn commit(&mut self, frames: &[u8]) {
        let end = self.written + frames.len() as u64;
        if self.step > 0 && end > self.extent {
            let to = (end + self.step + PAGE - 1) & !(PAGE - 1);
            let mut at = (end + PAGE - 1) & !(PAGE - 1);
            let zeros = [0u8; PAGE as usize];
            while at < to {
                self.file.write_all_at(&zeros, at).expect("the zeros");
                at += PAGE;
            }
            self.extent = to;
        }
        self.file
            .write_all_at(frames, self.written)
            .expect("the frames");
        self.written = end;
        self.file.sync_data().expect("the fdatasync");
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Milliseconds a mebibyte of log over `LOG` bytes of commits of `size`
/// under `step`, after eight commits untimed.
fn run(dir: &Path, step: u64, frames: &[u8]) -> f64 {
    let mut log = Log::create(dir, step);
    for _ in 0..8 {
        log.commit(frames);
    }
    let n = (LOG / frames.len() as u64).max(1);
    let t = Instant::now();
    for _ in 0..n {
        log.commit(frames);
    }
    let ms = t.elapsed().as_secs_f64() * 1e3;
    ms / ((n * frames.len() as u64) as f64 / (1u64 << 20) as f64)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let mut dir = std::env::temp_dir();
    let mut rounds = 9usize;
    let mut sizes: Vec<usize> = vec![
        4_000, 8_000, 16_800, 24_000, 32_000, 40_000, 48_000, 56_000, 64_000, 80_000, 96_000,
        133_000,
    ];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| panic!("{a} takes a value"));
        match a.as_str() {
            "--dir" => dir = PathBuf::from(value()),
            "--rounds" => rounds = value().parse().expect("--rounds N"),
            "--sizes" => {
                sizes = value()
                    .split(',')
                    .map(|s| s.parse().expect("--sizes BYTES,BYTES,..."))
                    .collect()
            }
            other => panic!("unknown argument {other}"),
        }
    }
    let steps: [(&str, u64); 3] = [
        ("appended", 0),
        ("step 256 KiB", 256 << 10),
        ("step 1 MiB", 1 << 20),
    ];
    println!(
        "{:>9}  {:>16}  {:>24}  {:>24}",
        "commit", "appended ms/MiB", "step 256 KiB ms/MiB", "step 1 MiB ms/MiB"
    );
    for size in sizes {
        // Bytes no page of which is zeros, as frames are.
        let frames: Vec<u8> = (0..size).map(|i| (i as u8) | 1).collect();
        let mut ms: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for round in 0..rounds {
            for k in 0..steps.len() {
                let i = if round % 2 == 0 {
                    k
                } else {
                    steps.len() - 1 - k
                };
                ms[i].push(run(&dir, steps[i].1, &frames));
            }
        }
        let m: Vec<f64> = ms.iter_mut().map(|v| median(v)).collect();
        println!(
            "{:>7.1}KB  {:>16.2}  {:>15.2} ({:>4.2}x)  {:>15.2} ({:>4.2}x)",
            size as f64 / 1000.0,
            m[0],
            m[1],
            m[0] / m[1],
            m[2],
            m[0] / m[2],
        );
    }
}
