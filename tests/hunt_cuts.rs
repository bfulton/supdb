//! A forcing schedule for the upkeep thread's cut assertion ("an overlay
//! key's cut is behind the walk"): a partition rewritten by a merge that
//! drops one key and adds another, so that its count and its key at every
//! block's first rank and at its last are what they were, is taken for the
//! same blocks, and the writer's table is carried across it with the cuts
//! of the keys it filed against the partition before.
//!
//! `--cfg hunt_old` leaves out the two schedules that hold a seal (`hold_seal_landing`, `hold_seal_durable`), for the
//! commits before the seals in flight were a queue.
//!
//! One partition of a hundred keys; a delete of k0040 and an insert of
//! k0061 sealed into a piece the landing of which is held; a write of k0050
//! filed against the partition as it stands; the landing, and the piece's
//! merge into the partition; a write of k0052, filed against the partition
//! as it now stands; and a scan, which builds the block over both.

use std::collections::BTreeMap;
use std::path::PathBuf;
use supdb::{Db, Options, Reader};

#[allow(dead_code)]
fn key(i: usize) -> Vec<u8> {
    format!("k{i:04}").into_bytes()
}

fn val(tag: &str) -> Vec<u8> {
    let mut v = tag.as_bytes().to_vec();
    v.resize(100, b'.');
    v
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-huntcuts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[allow(dead_code)]
fn scan_all(r: &Reader) -> BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut m: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    r.scan(b"", usize::MAX, |k, v| {
        m.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    m
}

type Scan = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

/// The two scans are the same, or the panic names the first key they part
/// at and what each holds there.
fn same(got: &Scan, want: &Scan, what: &str) {
    if got == want {
        return;
    }
    let show = |m: &Scan, k: &Vec<u8>| match m.get(k) {
        Some(vs) => format!(
            "{:?}",
            vs.iter()
                .map(|v| String::from_utf8_lossy(
                    &v[..v.iter().position(|&b| b == b'.').unwrap_or(v.len())]
                )
                .to_string())
                .collect::<Vec<_>>()
        ),
        None => "absent".to_string(),
    };
    let parts: Vec<&Vec<u8>> = got
        .keys()
        .chain(want.keys())
        .filter(|k| got.get(*k) != want.get(*k))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let lines: Vec<String> = parts
        .iter()
        .take(6)
        .map(|k| {
            format!(
                "{}: scan {}, model {}",
                String::from_utf8_lossy(k),
                show(got, k),
                show(want, k)
            )
        })
        .collect();
    panic!(
        "{what}: {} keys differ ({} scanned, {} in the model); {}",
        parts.len(),
        got.len(),
        want.len(),
        lines.join("; ")
    );
}

#[cfg(not(hunt_old))]
#[test]
fn a_rewrite_that_changes_interior_keys_leaves_cuts_behind() {
    let dir = scratch("interior");
    #[allow(unused_mut)]
    let mut opts = Options {
        partition_bytes: Some(1 << 20),
        scan_block_cache: true,
        l0_trigger: 1,
        ..Options::default()
    };
    // `--cfg hunt_rebase`, on commits that have `forms_rebase`:
    // HUNT_NO_REBASE=1 drops the tables at a merge's rewrite, as before they
    // were carried.
    #[cfg(hunt_rebase)]
    if std::env::var("HUNT_NO_REBASE").is_ok_and(|v| v == "1") {
        opts.forms_rebase = false;
    }
    let mut db = Db::create(&dir, opts).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for i in (0..200).step_by(2) {
        db.append(&key(i), &val("base"));
        model.insert(key(i), vec![val("base")]);
        if i % 50 == 48 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // The regime and the writer's table: a handle's scan, then the writer's.
    let h = db.reader().unwrap();
    same(&scan_all(&h), &model, "the first scan");
    same(&scan_all(&db), &model, "the first scan");

    db.hold_seal_landing(true);
    // X dropped, Y added, in one piece: count and block-first keys stand.
    db.delete(&key(40));
    model.remove(&key(40));
    db.append(&key(61), &val("Y"));
    model.insert(key(61), vec![val("Y")]);
    db.commit().unwrap();
    db.seal().unwrap();
    same(&scan_all(&db), &model, "after the seal");
    // The seal's thread publishes the frozen table's snapshot meanwhile, and
    // the scan that finds it rebuilds its own, emptying the lists of keys
    // filed since: let it be published and taken before Z1 is written.
    std::thread::sleep(std::time::Duration::from_millis(200));
    same(&scan_all(&db), &model, "after the seal's snapshot");
    // Z1 is filed against the partition as it stands.
    db.put(&key(50), &val("Z1"));
    model.insert(key(50), vec![val("Z1")]);
    db.commit().unwrap();
    same(&scan_all(&db), &model, "after Z1");
    same(&scan_all(&h), &model, "the handle after Z1");

    db.hold_seal_landing(false);
    db.settle().unwrap();
    while db.in_flight().1 {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    db.settle().unwrap();
    same(&scan_all(&db), &model, "after the landing and the merge");
    // Z2 is filed against the partition as it now stands.
    db.put(&key(52), &val("Z2"));
    model.insert(key(52), vec![val("Z2")]);
    db.commit().unwrap();
    same(&scan_all(&db), &model, "after Z2");
    same(&scan_all(&h), &model, "the handle after Z2");
    db.settle().unwrap();
    same(&scan_all(&db), &model, "after the last settle");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same hazard by the other route: nothing is frozen when the partition is rewritten, so a landing does not come
/// into it. The seal is held between its two phases (`hold_seal_durable`): its piece is published to readers and the
/// frozen table is retired, and the merge of the piece into the partition finishes but cannot land until the seal is
/// durable. A write is filed against the partition as it stands, the hold is lifted, the merge lands, and a scan reads
/// the block the write is in.
#[cfg(not(hunt_old))]
#[test]
fn a_merge_landing_after_the_look_leaves_cuts_behind() {
    let dir = scratch("durable");
    #[allow(unused_mut)]
    let mut opts = Options {
        partition_bytes: Some(1 << 20),
        scan_block_cache: true,
        l0_trigger: 1,
        ..Options::default()
    };
    #[cfg(hunt_rebase)]
    if std::env::var("HUNT_NO_REBASE").is_ok_and(|v| v == "1") {
        opts.forms_rebase = false;
    }
    let mut db = Db::create(&dir, opts).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for i in (0..200).step_by(2) {
        db.append(&key(i), &val("base"));
        model.insert(key(i), vec![val("base")]);
        if i % 50 == 48 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    let h = db.reader().unwrap();
    same(&scan_all(&h), &model, "the first scan");
    same(&scan_all(&db), &model, "the first scan");

    db.hold_seal_durable(true);
    db.delete(&key(40));
    model.remove(&key(40));
    db.append(&key(61), &val("Y"));
    model.insert(key(61), vec![val("Y")]);
    db.commit().unwrap();
    db.seal().unwrap();
    // The piece is readable and the frozen table retired; the merge of the piece waits to land.
    std::thread::sleep(std::time::Duration::from_millis(300));
    same(&scan_all(&db), &model, "after the seal's readable landing");
    // Z1 is filed against the partition as it stands.
    db.put(&key(50), &val("Z1"));
    model.insert(key(50), vec![val("Z1")]);
    db.commit().unwrap();
    same(&scan_all(&db), &model, "after Z1");
    same(&scan_all(&h), &model, "the handle after Z1");

    db.hold_seal_durable(false);
    db.settle().unwrap();
    while db.in_flight().1 {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    db.settle().unwrap();
    same(
        &scan_all(&db),
        &model,
        "after the durable landing and the merge",
    );
    db.put(&key(52), &val("Z2"));
    model.insert(key(52), vec![val("Z2")]);
    db.commit().unwrap();
    same(&scan_all(&db), &model, "after Z2");
    same(&scan_all(&h), &model, "the handle after Z2");
    db.settle().unwrap();
    same(&scan_all(&db), &model, "after the last settle");
    let _ = std::fs::remove_dir_all(&dir);
}

fn key6(i: usize) -> Vec<u8> {
    format!("k{i:06}").into_bytes()
}

/// The keys of `r` below `k000400`, from the start: what a scan of three hundred keys answers there.
fn head_of(r: &Reader) -> Scan {
    let mut m: Scan = BTreeMap::new();
    let bound = key6(400);
    r.scan(b"", 300, |k, v| {
        if k < bound.as_slice() {
            m.entry(k.to_vec()).or_default().push(v.to_vec())
        }
    })
    .unwrap();
    m
}

fn head_of_model(model: &Scan) -> Scan {
    let bound = key6(400);
    model
        .range(..bound)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The same hazard with no hook of the engine's at all, for the commits that have none: the partition is large, so
/// that the merge of the piece into it outlasts the writes made beside it. The seal's piece lands, the merge starts;
/// a write is filed against the partition as it stands while the merge runs; the merge lands; a scan reads the block.
/// The test says so when the merge was not running at the write (`SCHEDULE NOT REACHED`), which is not a pass.
#[test]
fn a_slow_merge_landing_after_the_look_leaves_cuts_behind() {
    let dir = scratch("slow");
    let n: usize = std::env::var("HUNT_BIG_KEYS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60_000);
    #[allow(unused_mut)]
    let mut opts = Options {
        partition_bytes: Some(1 << 30),
        scan_block_cache: true,
        l0_trigger: 1,
        ..Options::default()
    };
    #[cfg(hunt_rebase)]
    if std::env::var("HUNT_NO_REBASE").is_ok_and(|v| v == "1") {
        opts.forms_rebase = false;
    }
    // `--cfg hunt_lazy`, on the one commit that has both carries: HUNT_EAGER=1 carries the tables inside the publish.
    #[cfg(hunt_lazy)]
    if std::env::var("HUNT_EAGER").is_ok_and(|v| v == "1") {
        opts.forms_carry_lazily = false;
    }
    let mut db = Db::create(&dir, opts).unwrap();
    let mut model: Scan = BTreeMap::new();
    for i in (0..2 * n).step_by(2) {
        db.append(&key6(i), &val("base"));
        model.insert(key6(i), vec![val("base")]);
        if i % 10_000 == 9_998 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    // The regime and the writer's table: a handle's scan, then the writer's.
    let h = db.reader().unwrap();
    same(&head_of(&h), &head_of_model(&model), "the first scan");
    same(&head_of(&db), &head_of_model(&model), "the first scan");

    db.delete(&key6(40));
    model.remove(&key6(40));
    db.append(&key6(61), &val("Y"));
    model.insert(key6(61), vec![val("Y")]);
    db.commit().unwrap();
    db.seal().unwrap();
    // Read through the writer until the merge of the piece is running: on some commits the landing is the writer's to
    // apply at its next operation, on others the segment thread's.
    let t0 = std::time::Instant::now();
    while !db.in_flight().1 {
        same(
            &head_of(&db),
            &head_of_model(&model),
            "waiting for the merge",
        );
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(30),
            "SCHEDULE NOT REACHED: no merge in flight after 30 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    // Z1 is filed against the partition as it stands.
    db.put(&key6(50), &val("Z1"));
    model.insert(key6(50), vec![val("Z1")]);
    db.commit().unwrap();
    same(&head_of(&db), &head_of_model(&model), "after Z1");
    assert!(
        db.in_flight().1,
        "SCHEDULE NOT REACHED: the merge was over before Z1 was filed"
    );
    db.settle().unwrap();
    while db.in_flight().1 {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    db.settle().unwrap();
    same(
        &head_of(&db),
        &head_of_model(&model),
        "after the landing and the merge",
    );
    db.put(&key6(52), &val("Z2"));
    model.insert(key6(52), vec![val("Z2")]);
    db.commit().unwrap();
    same(&head_of(&db), &head_of_model(&model), "after Z2");
    same(&head_of(&h), &head_of_model(&model), "the handle after Z2");
    db.settle().unwrap();
    same(
        &head_of(&db),
        &head_of_model(&model),
        "after the last settle",
    );
    let _ = std::fs::remove_dir_all(&dir);
}
