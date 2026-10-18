//! Hunt stress0, the minimal form: the windows scenario and the snapshot
//! phase over public API only, so the same file builds at older commits. A
//! value the snapshot never held, in a scan through the snapshot handle, is a
//! failure.
//!
//! `HUNT_UPKEEP` picks the writer's upkeep: `bg1` (the default of this file),
//! `bg2`, `bg3`, `inline`, or `default` for the library's own. `HUNT_ITERS`
//! repeats the scenario in one process (the first failure ends it).
//!
//! To build it at a commit that lacks something it names: a tree from before
//! `Options::upkeep` has no `HUNT_UPKEEP` match to keep (delete the `match`
//! below), and a tree from before the seal hold has no `hold_seal_landing`
//! (delete that one line). Built so at 78bc23d, 39401bb, 049f4cf, 1087ec8,
//! 7b0ceb8, e607e70, 579f3d4, f32222a, 39b8f17, eca185b, 6d4b450, 9f5f1ab and
//! d9712ee, each in a target directory of its own (trees that share one
//! overwrite each other's binaries). Below e607e70 this scenario fails 42 runs
//! in 44, at the first key, for an older reason that is not this defect.

use std::collections::BTreeMap;
use std::path::PathBuf;
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

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-huntstress0min-{name}-{}", std::process::id()));
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
        ..Options::default()
    };
    match std::env::var("HUNT_UPKEEP").as_deref() {
        Ok("inline") => opts.upkeep = supdb::Upkeep::Inline,
        Ok("bg2") => opts.upkeep = supdb::Upkeep::Background(2),
        Ok("bg3") => opts.upkeep = supdb::Upkeep::Background(3),
        Ok("default") => {}
        _ => opts.upkeep = supdb::Upkeep::Background(1),
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
        if round == 1 {
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
    let snap = db.reader().unwrap();
    snap.snapshot();
    let at_snapshot = model.clone();
    for k in keys.iter().step_by(5).take(200) {
        db.put(k, b"after the snapshot");
    }
    db.commit().unwrap();
    let mut bad: Option<String> = None;
    'scans: for start in starts.iter().step_by(3) {
        for &limit in &[3usize, 57, 114] {
            let got = scanned(&snap, start, limit);
            for (k, v) in &got {
                let known = at_snapshot.get(k).is_some_and(|vs| vs.contains(v));
                if !known {
                    bad = Some(format!(
                        "from {:?} for {limit}: {:?} = a value the snapshot never held",
                        String::from_utf8_lossy(start),
                        String::from_utf8_lossy(k)
                    ));
                    break 'scans;
                }
            }
        }
    }
    drop(latest);
    drop(snap);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
    match bad {
        Some(b) => Err(b),
        None => Ok(()),
    }
}

#[test]
fn hunt_scenario() {
    let iters: usize = std::env::var("HUNT_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    for i in 0..iters {
        if let Err(e) = one_run(i) {
            eprintln!("HUNT-FAIL snapshot-later-write {e}");
            panic!("hunt failure");
        }
    }
    eprintln!("HUNT-OK minimal iters {iters}");
}
