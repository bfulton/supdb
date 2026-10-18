//! Hunt stress0: the second defect's workload, without the second form.
//!
//! The compact form's branch ran this oracle against a store with the form
//! forced on and one with it off, each checked against a model after every
//! settle. The form is gone here and both stores are kept (plain), so the
//! engine's threads and the schedule are the original's. A debug assertion
//! in the forms upkeep (`BuildCtx::cut_known`, "an overlay key's cut is
//! behind the walk") fired in about 3% of its runs with the block cache on,
//! and then `Lend::take_back`'s assertion fired again inside `Drop for Db`
//! while unwinding, which aborts the process.
//!
//! Knobs (environment): `HUNT_ITERS` runs per process, `HUNT_SEED` xored
//! into the operation stream, `HUNT_UPKEEP` (inline|bg1|bg2|bg3),
//! `HUNT_ORACLE_INLINE` (1: values inline, the default), `HUNT_ORACLE_CACHE`
//! (1: the block cache on, the default), `HUNT_ORACLE_STORES` (2),
//! `HUNT_ORACLE_STEPS` (3000), `HUNT_ORACLE_SEAL_KIB` (8),
//! `HUNT_ORACLE_PART_KIB` (4).

use std::collections::BTreeMap;
use std::path::PathBuf;
use supdb::{Db, Options, Reader, Upkeep};

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

fn env_str(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn env_usize(name: &str, default: usize) -> usize {
    env_str(name).map_or(default, |s| s.parse().expect(name))
}

fn env_flag(name: &str, default: bool) -> bool {
    env_str(name).map_or(default, |s| s != "0")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-hunt0o-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn show(k: &[u8]) -> String {
    String::from_utf8_lossy(k).into_owned()
}

const ORACLE_KEYS: usize = 240;

fn upkeep_of(name: &str) -> Upkeep {
    match name {
        "inline" => Upkeep::Inline,
        "bg1" => Upkeep::Background(1),
        "bg2" => Upkeep::Background(2),
        "bg3" => Upkeep::Background(3),
        other => panic!("HUNT_UPKEEP {other}"),
    }
}

fn read_vec(r: &Reader, key: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut v = Vec::new();
    let n = r
        .read_all(key, |x| v.push(x.to_vec()))
        .map_err(|e| format!("read_all: {e}"))?;
    if n as usize != v.len() {
        return Err(format!("the count read_all returns: {n} for {}", v.len()));
    }
    Ok(v)
}

fn check_keys(r: &Reader, model: &Model, keys: &[Vec<u8>], what: &str) -> Result<(), String> {
    for k in keys {
        let want = model.get(k).cloned().unwrap_or_default();
        let got = read_vec(r, k)?;
        if got != want {
            return Err(format!(
                "HUNT-FAIL oracle-mismatch {what}: read of {}: got {} values, wanted {}\n{}",
                show(k),
                got.len(),
                want.len(),
                r.debug_key(k)
            ));
        }
        let n = r.count(k).map_err(|e| format!("count: {e}"))? as usize;
        if n != want.len() {
            return Err(format!(
                "HUNT-FAIL oracle-mismatch {what}: count of {}: {n}, wanted {}\n{}",
                show(k),
                want.len(),
                r.debug_key(k)
            ));
        }
    }
    Ok(())
}

fn check_scan_all(r: &Reader, model: &Model, what: &str) -> Result<(), String> {
    let mut seen: Model = BTreeMap::new();
    r.scan(b"", usize::MAX, |k, v| {
        seen.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .map_err(|e| format!("scan: {e}"))?;
    let live: Model = model
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if seen != live {
        let k = seen
            .iter()
            .find(|(k, v)| live.get(*k) != Some(*v))
            .map(|(k, _)| k.clone())
            .or_else(|| {
                live.iter()
                    .find(|(k, v)| seen.get(*k) != Some(*v))
                    .map(|(k, _)| k.clone())
            })
            .unwrap_or_default();
        return Err(format!(
            "HUNT-FAIL oracle-mismatch {what}: the scan differs at {}: saw {} keys, wanted {}\n{}",
            show(&k),
            seen.len(),
            live.len(),
            r.debug_key(&k)
        ));
    }
    Ok(())
}

fn apply_batch(model: &mut Model, batch: &mut Vec<(Vec<u8>, Option<Vec<u8>>)>) {
    for (k, v) in batch.drain(..) {
        let e = model.entry(k).or_default();
        match v {
            Some(v) => e.push(v),
            None => e.clear(),
        }
    }
}

fn oracle_run(iter: usize) -> Result<(), String> {
    let inline = env_flag("HUNT_ORACLE_INLINE", true);
    let cache = env_flag("HUNT_ORACLE_CACHE", true);
    let stores = env_usize("HUNT_ORACLE_STORES", 2).max(1);
    let steps = env_usize("HUNT_ORACLE_STEPS", 3000) as u64;
    let seed = env_usize("HUNT_SEED", 0) as u64;
    let upkeep = upkeep_of(env_str("HUNT_UPKEEP").as_deref().unwrap_or("bg1"));
    let dirs: Vec<PathBuf> = (0..stores)
        .map(|i| scratch(&format!("{iter}-{i}")))
        .collect();
    let opts = Options {
        scan_block_cache: cache,
        seal_bytes: env_usize("HUNT_ORACLE_SEAL_KIB", 8) << 10,
        partition_bytes: Some(env_usize("HUNT_ORACLE_PART_KIB", 4) << 10),
        inline_bytes: if inline {
            1 << 16
        } else {
            Options::default().inline_bytes
        },
        l0_trigger: 2,
        upkeep,
        ..Options::default()
    };
    let mut dbs: Vec<Db> = dirs
        .iter()
        .map(|d| Db::create(d, opts.clone()).unwrap())
        .collect();
    let mut model: Model = BTreeMap::new();
    let mut pending: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
    let mut rng = Rng(0x0AC1E ^ seed);
    let keys: Vec<Vec<u8>> = (0..ORACLE_KEYS + 20)
        .map(|i| format!("k{i:04}").into_bytes())
        .collect();
    let (mut crashes, mut settles) = (0u64, 0u64);
    let result = (|| -> Result<(), String> {
        for step in 0..steps {
            let key = format!("k{:04}", rng.below(ORACLE_KEYS)).into_bytes();
            match rng.below(20) {
                0 => {
                    for d in dbs.iter_mut() {
                        d.delete(&key);
                    }
                    pending.push((key, None));
                }
                1..=3 => {
                    for j in 0..2 + rng.below(4) {
                        let v = if j == 1 {
                            Vec::new()
                        } else {
                            format!("m{step}.{j}").into_bytes()
                        };
                        for d in dbs.iter_mut() {
                            d.append(&key, &v);
                        }
                        pending.push((key.clone(), Some(v)));
                    }
                }
                _ => {
                    let pad = rng.below(80);
                    let v = format!("v{step}-{}", "p".repeat(pad)).into_bytes();
                    for d in dbs.iter_mut() {
                        d.append(&key, &v);
                    }
                    pending.push((key, Some(v)));
                }
            }
            match rng.below(100) {
                0..=9 => {
                    for d in dbs.iter_mut() {
                        d.commit().map_err(|e| format!("commit: {e}"))?;
                    }
                    apply_batch(&mut model, &mut pending);
                }
                10..=12 => {
                    for d in dbs.iter_mut() {
                        d.commit().map_err(|e| format!("commit: {e}"))?;
                    }
                    apply_batch(&mut model, &mut pending);
                    for d in dbs.iter_mut() {
                        d.seal().map_err(|e| format!("seal: {e}"))?;
                    }
                }
                13 if step % 5 == 0 => {
                    // A crash: what was not committed vanishes, on every store.
                    dbs.clear();
                    pending.clear();
                    dbs = dirs
                        .iter()
                        .map(|d| Db::open(d, opts.clone()).unwrap())
                        .collect();
                    crashes += 1;
                    for d in dbs.iter() {
                        check_keys(d, &model, &keys, "after a crash")?;
                    }
                }
                _ => {}
            }
            if step % 250 == 249 {
                for d in dbs.iter_mut() {
                    d.commit().map_err(|e| format!("commit: {e}"))?;
                }
                apply_batch(&mut model, &mut pending);
                if step % 500 == 499 {
                    for d in dbs.iter_mut() {
                        d.flush().map_err(|e| format!("flush: {e}"))?;
                    }
                }
                for d in dbs.iter_mut() {
                    d.settle().map_err(|e| format!("settle: {e}"))?;
                }
                settles += 1;
                for d in dbs.iter() {
                    check_keys(d, &model, &keys, "writer")?;
                    let h = d.reader().map_err(|e| format!("reader: {e}"))?;
                    check_keys(&h, &model, &keys, "handle")?;
                }
                check_scan_all(&dbs[0], &model, "writer scan")?;
            }
        }
        for d in dbs.iter_mut() {
            d.commit().map_err(|e| format!("commit: {e}"))?;
        }
        apply_batch(&mut model, &mut pending);
        for d in dbs.iter_mut() {
            d.settle().map_err(|e| format!("settle: {e}"))?;
        }
        for d in dbs.iter() {
            check_keys(d, &model, &keys, "at the end")?;
            let snap = d.reader().map_err(|e| format!("reader: {e}"))?;
            snap.snapshot();
            check_keys(&snap, &model, &keys, "a snapshot")?;
        }
        if crashes == 0 || settles < 8 {
            return Err(format!(
                "HUNT-FAIL oracle-shape crashes {crashes} settles {settles}"
            ));
        }
        Ok(())
    })();
    let closed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(dbs);
    }));
    for d in &dirs {
        let _ = std::fs::remove_dir_all(d);
    }
    if closed.is_err() {
        return Err("HUNT-FAIL close panicked".to_string());
    }
    result
}

#[test]
fn hunt_oracle() {
    let iters = env_usize("HUNT_ITERS", 1);
    for iter in 0..iters {
        if let Err(e) = oracle_run(iter) {
            eprintln!("{e}");
            panic!("hunt oracle failure at iteration {iter}");
        }
    }
    eprintln!("HUNT-OK oracle iters {iters}");
}

/// The second defect's schedule, forced and single-threaded (`Upkeep::Inline`).
///
/// A partition of thirty-one keys, one block; a table built over it by the
/// writer's scans. A frozen table (the seal held) carries a key inserted
/// before two keys the live table will update and a key deleted after them.
/// A key updated in the live table is filed in its block's list with its cut
/// in the partition as it stands: against the old blob. The hold is lifted,
/// the piece lands and is merged into the partition, whose count and ends are
/// what they were -- one key in, one out -- so `Db::same_blocks`, which
/// compares the count and the keys at the blocks' first ranks and the last,
/// calls the new blob the old one's blocks and the tables are carried with
/// their filed keys' cuts: a key filed before the rewrite is one rank behind
/// where the rewritten partition has it.
///
/// `HUNT_CUT_VARIANT=assert` (the default) updates two neighbouring keys, one
/// filed before the rewrite and one after, so the walk passes the second's
/// stale cut and `BuildCtx::cut_known` asserts. `silent` updates one key
/// whose stale cut is still ahead of the walk: no assertion, and the scan
/// answers a wrong partition record under the key.
#[test]
fn hunt_cut_forced() {
    let variant = env_str("HUNT_CUT_VARIANT").unwrap_or_else(|| "assert".to_string());
    let dir = scratch("cutforced");
    let opts = Options {
        scan_block_cache: true,
        upkeep: Upkeep::Inline,
        partition_bytes: Some(1 << 20),
        seal_bytes: 1 << 30,
        l0_trigger: 1,
        ..Options::default()
    };
    let key = |n: usize| format!("k{n:04}").into_bytes();
    let mut db = Db::create(&dir, opts).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    for n in (100..=160).step_by(2) {
        let v = format!("p-{n}").into_bytes();
        db.put(&key(n), &v);
        model.insert(key(n), v);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let walk = |db: &Db| -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut got = Vec::new();
        db.scan(b"", usize::MAX, |k, v| got.push((k.to_vec(), v.to_vec())))
            .unwrap();
        got
    };
    // Reads around, so the forms are kept and the block has a table.
    for _ in 0..3 {
        assert_eq!(walk(&db).len(), model.len());
    }
    let (inserted, deleted) = (key(105), key(140));
    let (first, second) = match variant.as_str() {
        "assert" => (Some(key(118)), key(120)),
        "silent" => (None, key(126)),
        other => panic!("HUNT_CUT_VARIANT {other}"),
    };
    db.put(&inserted, b"x-105");
    model.insert(inserted.clone(), b"x-105".to_vec());
    db.delete(&deleted);
    model.remove(&deleted);
    db.commit().unwrap();
    db.hold_seal_landing(true);
    db.seal().unwrap();
    // A scan after the freeze: the writer's tables are carried across it and
    // its snapshot built again, over a live table that is empty, so the key
    // written next is created after the snapshot and is filed in its block's
    // list, not held by the snapshot.
    let _ = walk(&db);
    // The key filed before the rewrite, against the old blob.
    db.put(&second, b"v-second");
    model.insert(second.clone(), b"v-second".to_vec());
    db.commit().unwrap();
    let before = walk(&db);
    let want_before: Vec<_> = model.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(before, want_before, "the scan before the landing");
    let gen_before = db.state_gen();
    db.hold_seal_landing(false);
    db.settle().unwrap();
    eprintln!(
        "HUNT-CUT-FORCED levels {:?} state gen {} -> {} (the piece landed and was merged when the pieces are 0)",
        db.levels(),
        gen_before,
        db.state_gen()
    );
    if let Some(first) = &first {
        db.put(first, b"v-first");
        model.insert(first.clone(), b"v-first".to_vec());
        db.commit().unwrap();
    }
    let want: Vec<_> = model.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if env_flag("HUNT_CUT_DEBUG", false) {
        eprintln!("HUNT-CUT-DEBUG before the last scan: {}", db.debug_key(&second));
    }
    let scanned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| walk(&db)));
    if env_flag("HUNT_CUT_DEBUG", false) {
        eprintln!("HUNT-CUT-DEBUG after the last scan: {}", db.debug_key(&second));
    }
    let verdict = match &scanned {
        Err(_) => "the engine panicked".to_string(),
        Ok(got) if *got == want => "right".to_string(),
        Ok(got) => {
            let at = got
                .iter()
                .zip(&want)
                .position(|(a, b)| a != b)
                .unwrap_or(got.len().min(want.len()));
            format!(
                "wrong: {} entries, wanted {}; first difference at entry {at}: got {:?}, wanted {:?}",
                got.len(),
                want.len(),
                got.get(at).map(|(k, v)| (show(k), show(v))),
                want.get(at).map(|(k, v)| (show(k), show(v)))
            )
        }
    };
    eprintln!("HUNT-CUT-FORCED variant {variant}: the scan after the rewrite is {verdict}");
    let panicked = scanned.is_err();
    drop(scanned);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(db)));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        verdict == "right",
        "the scan over a partition rewritten under the same blocks: {verdict} (panicked {panicked})"
    );
}
