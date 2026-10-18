//! A forcing schedule for the upkeep thread's cut assertion ("an overlay
//! key's cut is behind the walk"): a partition rewritten by a merge that
//! drops one key and adds another, so that its count and its key at every
//! block's first rank and at its last are what they were, is taken for the
//! same blocks, and the writer's table is carried across it with the cuts
//! of the keys it filed against the partition before.
//!
//! One partition of a hundred keys; a delete of k0040 and an insert of
//! k0061 sealed into a piece the landing of which is held; a write of k0050
//! filed against the partition as it stands; the landing, and the piece's
//! merge into the partition; a write of k0052, filed against the partition
//! as it now stands; and a scan, which builds the block over both.

use std::collections::BTreeMap;
use std::path::PathBuf;
use supdb::{Db, Options, Reader};

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
                .map(|v| String::from_utf8_lossy(&v[..v.iter().position(|&b| b == b'.').unwrap_or(v.len())]).to_string())
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
    panic!("{what}: {} keys differ ({} scanned, {} in the model); {}", parts.len(), got.len(), want.len(), lines.join("; "));
}

#[test]
fn a_rewrite_that_changes_interior_keys_leaves_cuts_behind() {
    let dir = scratch("interior");
    let mut db = Db::create(
        &dir,
        Options {
            partition_bytes: Some(1 << 20),
            scan_block_cache: true,
            l0_trigger: 1,
            ..Options::default()
        },
    )
    .unwrap();
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
