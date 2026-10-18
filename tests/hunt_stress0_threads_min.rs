//! Hunt stress0, the minimal form with the snapshot handles on threads of
//! their own: the store and the windows scans of `hunt_stress0_min.rs`, then
//! `HUNT_SNAP_THREADS` handles (default 2) are claimed and pinned with
//! `snapshot()`, each is moved to a thread that scans from many starts in a
//! loop and checks every value against the model as it was when the handles
//! were pinned, while the writer puts and commits (`HUNT_COMMITS`, default 6,
//! of `HUNT_AFTER_N`, default 200, puts each, a different value each commit).
//! A value the snapshot never held, read by a thread, is the defect. Nothing
//! here depends on the writer's upkeep running on a thread: with
//! `HUNT_UPKEEP=inline` the publisher is the commit itself.
//!
//! The same file builds at older commits: delete the `match` on `HUNT_UPKEEP`
//! in a tree from before `Options::upkeep`, and the `hold_seal_landing` line in
//! a tree from before the hold. `HUNT_SEAL=0` leaves out the seal of the
//! second round, `HUNT_ITERS` repeats the scenario in one process.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use supdb::{Db, Options, Reader};

type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "supdb-next-huntstress0threads-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn suite_value(rng: &mut Rng) -> Vec<u8> {
    let motif = b"the quick brown fox jumps over the lazy dog 0123456789 ";
    let phase = rng.below(motif.len());
    let mut v: Vec<u8> = (0..50).map(|j| motif[(phase + j) % motif.len()]).collect();
    v.extend((0..50).map(|_| (rng.next() >> 24) as u8));
    v
}

fn spaced_key(i: usize) -> Vec<u8> {
    format!("key-{:08}", i * 3).into_bytes()
}

fn around(k: &[u8]) -> Vec<Vec<u8>> {
    let mut v = vec![
        k.to_vec(),
        [k, &[0u8][..]].concat(),
        [k, &[0xffu8][..]].concat(),
    ];
    if let Some((&last, head)) = k.split_last() {
        v.push(head.to_vec());
        v.push([head, &[last.wrapping_sub(1)][..]].concat());
        v.push([head, &[last.wrapping_add(1)][..]].concat());
    }
    v
}

fn scanned(r: &Reader, start: &[u8], limit: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut got = Vec::new();
    r.scan(start, limit, |k, v| got.push((k.to_vec(), v.to_vec())))
        .unwrap();
    got
}

fn one_run(iter: usize) -> Result<(), String> {
    let dir = scratch(&format!("i{iter}"));
    let mut opts = Options {
        partition_bytes: Some(160 << 10),
        scan_block_cache: true,
        commit_forms: true,
        ..Options::default()
    };
    match std::env::var("HUNT_UPKEEP").as_deref() {
        Ok("inline") => opts.upkeep = supdb::Upkeep::Inline,
        Ok("bg1") => opts.upkeep = supdb::Upkeep::Background(1),
        Ok("bg2") => opts.upkeep = supdb::Upkeep::Background(2),
        Ok("bg3") => opts.upkeep = supdb::Upkeep::Background(3),
        _ => {}
    }
    let mut db = Db::create(&dir, opts).unwrap();
    let mut rng = Rng(0x71D);
    let mut model = Model::new();
    let n = 5000usize;
    for chunk in (0..n).collect::<Vec<_>>().chunks(500) {
        for &i in chunk {
            let (k, v) = (spaced_key(i), suite_value(&mut rng));
            db.append(&k, &v);
            model.insert(k, vec![v]);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    db.settle().unwrap();
    db.hold_seal_landing(true);
    let mut rng = Rng(0x71D1);
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    let seal = env_usize("HUNT_SEAL", 1) != 0;
    for round in 0..3 {
        for _ in 0..120 {
            let k = keys[rng.below(keys.len())].clone();
            match rng.below(6) {
                0 => {
                    db.delete(&k);
                    model.insert(k, Vec::new());
                }
                1 => {
                    let new = [k.as_slice(), b"+"].concat();
                    let v = suite_value(&mut rng);
                    db.append(&new, &v);
                    model.entry(new).or_default().push(v);
                }
                _ => {
                    let v = suite_value(&mut rng);
                    db.put(&k, &v);
                    model.insert(k, vec![v]);
                }
            }
        }
        db.commit().unwrap();
        if round == 1 && seal {
            db.seal().unwrap();
        }
    }
    let mut starts: Vec<Vec<u8>> =
        vec![b"".to_vec(), b"a".to_vec(), b"zzz".to_vec(), vec![0xff; 8]];
    for i in (0..n).step_by(238) {
        starts.extend(around(&spaced_key(i)));
    }
    let latest = db.reader().unwrap();
    let limits = [1usize, 2, 3, 5, 17, 56, 57, 58, 114, usize::MAX];
    for start in &starts {
        for &limit in &limits {
            let cap = limit.min(1500);
            let _ = scanned(&db, start, cap);
            let _ = scanned(&latest, start, cap);
        }
    }
    let threads = env_usize("HUNT_SNAP_THREADS", 2);
    let snaps: Vec<Reader> = (0..threads)
        .map(|_| {
            let r = db.reader().unwrap();
            r.snapshot();
            r
        })
        .collect();
    let at_snapshot = model.clone();
    let stop = AtomicBool::new(false);
    let bad: Mutex<Option<String>> = Mutex::new(None);
    let commits = env_usize("HUNT_COMMITS", 6);
    let after_n = env_usize("HUNT_AFTER_N", 200);
    std::thread::scope(|s| {
        for snap in snaps {
            let (stop, bad, at_snapshot, starts) = (&stop, &bad, &at_snapshot, &starts);
            s.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    for start in starts.iter().step_by(3) {
                        for &limit in &[3usize, 57, 114] {
                            let got = scanned(&snap, start, limit);
                            for (k, v) in &got {
                                let known = at_snapshot.get(k).is_some_and(|vs| vs.contains(v));
                                if !known {
                                    let mut b = bad.lock().unwrap();
                                    if b.is_none() {
                                        *b = Some(format!(
                                            "from {:?} for {limit}: {:?} = a value the snapshot never held",
                                            String::from_utf8_lossy(start),
                                            String::from_utf8_lossy(k)
                                        ));
                                    }
                                    stop.store(true, Ordering::Relaxed);
                                    return;
                                }
                            }
                        }
                    }
                }
            });
        }
        for c in 0..commits {
            let value = format!("after the snapshot {c}");
            for k in keys.iter().step_by(5).take(after_n) {
                db.put(k, value.as_bytes());
            }
            db.commit().unwrap();
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        stop.store(true, Ordering::Relaxed);
    });
    if std::env::var_os("HUNT_COUNTS").is_some() {
        eprintln!("HUNT-COUNTS canonical_forms {:?}", db.canonical_forms());
    }
    drop(latest);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
    match bad.into_inner().unwrap() {
        Some(b) => Err(b),
        None => Ok(()),
    }
}

#[test]
fn hunt_scenario() {
    let iters = env_usize("HUNT_ITERS", 1);
    for i in 0..iters {
        if let Err(e) = one_run(i) {
            eprintln!("HUNT-FAIL thread-later-write {e}");
            panic!("hunt failure");
        }
    }
    eprintln!("HUNT-OK threads iters {iters}");
}
