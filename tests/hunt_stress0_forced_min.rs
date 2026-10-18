//! Hunt stress0, the minimal form forced: the store and the snapshot of
//! `hunt_stress0_min.rs`, but the forms publisher is held after its slot
//! swaps and before its store of the position (`dbgtrace::hold_forms_position`),
//! and the snapshot handle scans while it is parked there. A value the
//! snapshot never held is the defect; a publisher that never parks is
//! reported as such (a clean result then proves nothing).
//!
//! The hook is the engine's own in the hunt branch. At an older commit it is
//! a patch of two pieces: a `dbgtrace` module with the three functions, and
//! a call to `dbgtrace::forms_hold_wait()` as the first statement of the
//! `if self.publish_dirty(false) {` block of the upkeep pass. As in the
//! minimal form, a tree without `hold_seal_landing` loses that line, and a
//! tree without `Options::upkeep` has no `HUNT_UPKEEP` match.

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
    let d = std::env::temp_dir().join(format!("supdb-next-huntstress0forcedmin-{name}-{}", std::process::id()));
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

fn one_run(iter: usize) -> Result<(bool, Option<String>), String> {
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
    supdb::db::dbgtrace::hold_forms_position(true);
    for k in keys.iter().step_by(5).take(300) {
        db.put(k, b"after the snapshot");
    }
    db.commit().unwrap();
    let began = std::time::Instant::now();
    while !supdb::db::dbgtrace::forms_hold_parked()
        && began.elapsed() < std::time::Duration::from_secs(3)
    {
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    let parked = supdb::db::dbgtrace::forms_hold_parked();
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
    supdb::db::dbgtrace::hold_forms_position(false);
    drop(latest);
    drop(snap);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
    Ok((parked, bad))
}

#[test]
fn hunt_forced() {
    let (parked, bad) = one_run(0).unwrap();
    eprintln!("HUNT-FORCED publisher parked {parked} wrong-while-parked {bad:?}");
    if parked {
        assert!(bad.is_none(), "a value the snapshot never held");
    }
}
