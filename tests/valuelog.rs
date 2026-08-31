//! The redo log carries values: durability points with no seal.
//!
//! Every test here emulates the only crash that matters for a log: the arena
//! fsync returned (the ack), and nothing written after it reached the device.
//! The emulation restores both superblock slots to their pre-batch bytes --
//! the sections and superblock of the acked checkpoint are exactly what rides
//! unsynced behind the arena fsync.

use supdb::{Options, ReadOptions, Reader, Store, Sync};

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("vlog-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opts() -> Options {
    Options {
        sync: Sync::Always,
        ..Default::default()
    }
}

/// Superblock region: both slots and nothing else lives below SUPER=4096.
fn snapshot_sb(path: &std::path::Path) -> Vec<u8> {
    std::fs::read(path).unwrap()[0..4096].to_vec()
}

fn crash_to(path: &std::path::Path, sb: &[u8]) {
    let mut now = std::fs::read(path).unwrap();
    now[0..4096].copy_from_slice(sb);
    std::fs::write(path, &now).unwrap();
}

fn read_vec(s: &Store, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    s.read_all(key, |v| out.push(v.to_vec())).unwrap();
    out
}

fn reader_vec(r: &Reader, key: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    r.read_all(key, |v| out.push(v.to_vec())).unwrap();
    out
}

/// The core promise: a durable put whose bytes were never sealed into any
/// block survives losing every write made after the arena fsync.
#[test]
fn unsealed_values_survive_the_ack_point_crash() {
    let path = dir("fresh").join("s.supdb");
    let store = Store::create(&path, opts()).unwrap();
    // The first checkpoint is a full rewrite whose superblock is fsynced
    // before its ack, so THIS is the oldest state a crash can reveal.
    store.checkpoint().unwrap();
    let sb = snapshot_sb(&path);
    for i in 0..50u32 {
        store.put(format!("k{i:03}").as_bytes(), &[7u8; 64]).unwrap();
    }
    store.checkpoint().unwrap(); // the ack
    std::mem::forget(store); // crash, not close
    crash_to(&path, &sb);
    let s = Store::open(&path, opts()).unwrap();
    for i in 0..50u32 {
        let got = read_vec(&s, format!("k{i:03}").as_bytes());
        assert_eq!(got, vec![vec![7u8; 64]], "k{i:03} after crash");
    }
    // The other read path has to agree, including through scan.
    drop(s);
    let r = Reader::open_with(&path, ReadOptions::default()).unwrap();
    assert_eq!(reader_vec(&r, b"k007"), vec![vec![7u8; 64]]);
    let mut seen = 0usize;
    r.scan(None, usize::MAX, |_, v| {
        assert_eq!(v, &[7u8; 64][..]);
        seen += 1;
    })
    .unwrap();
    assert_eq!(seen, 50);
}

/// Appends across several durable points log DELTAS; replay must concatenate
/// them, and a reopen must not re-log what the arena already holds -- the
/// second crash is where a broken watermark shows up as doubled values.
#[test]
fn appended_deltas_concatenate_and_never_double() {
    let path = dir("delta").join("s.supdb");
    let store = Store::create(&path, opts()).unwrap();
    store.checkpoint().unwrap();
    let sb = snapshot_sb(&path);
    for i in 0..3u8 {
        store.append(b"k", &[i; 32]).unwrap();
        store.checkpoint().unwrap();
    }
    std::mem::forget(store);
    crash_to(&path, &sb);
    let s = Store::open(&path, opts()).unwrap();
    assert_eq!(
        read_vec(&s, b"k"),
        vec![vec![0u8; 32], vec![1u8; 32], vec![2u8; 32]]
    );
    // A durable point right after reopen must not re-log the replayed run.
    s.checkpoint().unwrap();
    std::mem::forget(s);
    // Crash again, losing nothing new -- replay walks the same arena.
    let s = Store::open(&path, opts()).unwrap();
    assert_eq!(
        read_vec(&s, b"k"),
        vec![vec![0u8; 32], vec![1u8; 32], vec![2u8; 32]],
        "values doubled: the reopen re-logged bytes the arena already held"
    );
}

/// A put after sealed values logs a replacing run; recovery must serve the
/// replacement alone, not the sealed values it hides.
#[test]
fn a_logged_replacement_hides_what_was_sealed() {
    let path = dir("repl").join("s.supdb");
    let store = Store::create(&path, opts()).unwrap();
    store.put(b"k", &[1u8; 100]).unwrap();
    // A full rewrite seals and publishes the first version.
    store.checkpoint().unwrap();
    store.publish().unwrap();
    let sb = snapshot_sb(&path);
    store.put(b"k", &[2u8; 100]).unwrap();
    store.checkpoint().unwrap(); // ack: the replacement is durable
    std::mem::forget(store);
    crash_to(&path, &sb);
    let s = Store::open(&path, opts()).unwrap();
    assert_eq!(read_vec(&s, b"k"), vec![vec![2u8; 100]]);
    drop(s);
    let r = Reader::open_with(&path, ReadOptions::default()).unwrap();
    assert_eq!(reader_vec(&r, b"k"), vec![vec![2u8; 100]]);
}

/// One durability point can carry all three record kinds at once: a batch
/// big enough to seal inline leaves staged blocks (Blocks + Sealed) and a
/// pending tail (Value). Order within the point is what recovery leans on.
#[test]
fn sealed_and_pending_in_one_point_both_survive() {
    let path = dir("dual").join("s.supdb");
    let o = Options {
        shards: 1,
        ..opts()
    };
    let store = Store::create(&path, o.clone()).unwrap();
    store.checkpoint().unwrap();
    let sb = snapshot_sb(&path);
    // ~200KB through one shard: forces inline seals, then a small tail.
    for i in 0..100u32 {
        store
            .append(b"big", format!("v{i:04}-{}", "x".repeat(2000)).as_bytes())
            .unwrap();
    }
    store.append(b"tail", b"small").unwrap();
    store.checkpoint().unwrap();
    std::mem::forget(store);
    crash_to(&path, &sb);
    let s = Store::open(&path, o).unwrap();
    let big = read_vec(&s, b"big");
    assert_eq!(big.len(), 100);
    for (i, v) in big.iter().enumerate() {
        assert!(v.starts_with(format!("v{i:04}-").as_bytes()), "order lost at {i}");
    }
    assert_eq!(read_vec(&s, b"tail"), vec![b"small".to_vec()]);
}

/// A delete logged after value records must win over them on recovery.
#[test]
fn a_logged_delete_wins_over_logged_values() {
    let path = dir("del").join("s.supdb");
    let store = Store::create(&path, opts()).unwrap();
    store.checkpoint().unwrap();
    let sb = snapshot_sb(&path);
    store.put(b"k", b"alive").unwrap();
    store.checkpoint().unwrap();
    store.delete(b"k").unwrap();
    store.checkpoint().unwrap();
    std::mem::forget(store);
    crash_to(&path, &sb);
    let s = Store::open(&path, opts()).unwrap();
    assert_eq!(read_vec(&s, b"k"), Vec::<Vec<u8>>::new());
    drop(s);
    let r = Reader::open_with(&path, ReadOptions::default()).unwrap();
    assert_eq!(reader_vec(&r, b"k"), Vec::<Vec<u8>>::new());
    let mut seen = 0usize;
    r.scan(None, usize::MAX, |_, _| seen += 1).unwrap();
    assert_eq!(seen, 0, "scan resurrected a deleted key");
}

/// A fresh Reader -- no crash, no reopen -- must see logged values through
/// every entry point: read_all, scan, keys, read_first/read_last.
#[test]
fn a_fresh_reader_serves_logged_values() {
    let path = dir("reader").join("s.supdb");
    let store = Store::create(&path, opts()).unwrap();
    store.append(b"a", b"one").unwrap();
    store.append(b"a", b"twoo").unwrap();
    store.append(b"b", b"three").unwrap();
    store.checkpoint().unwrap();
    let r = Reader::open_with(&path, ReadOptions::default()).unwrap();
    assert_eq!(reader_vec(&r, b"a"), vec![b"one".to_vec(), b"twoo".to_vec()]);
    assert_eq!(r.keys(), 2);
    assert_eq!(r.read_first(b"a").unwrap(), 3);
    assert_eq!(r.read_last(b"a").unwrap(), 4);
    let mut got: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    r.scan(None, usize::MAX, |k, v| got.push((k.to_vec(), v.to_vec()))).unwrap();
    assert_eq!(
        got,
        vec![
            (b"a".to_vec(), b"one".to_vec()),
            (b"a".to_vec(), b"twoo".to_vec()),
            (b"b".to_vec(), b"three".to_vec()),
        ]
    );
    drop(r);
    drop(store);
}

/// A key stays dirty across logged points and its Sealed record is RE-logged
/// while a logged tail is still live. A fold that treated every Sealed as
/// absorbing lost that tail -- one whole round of acknowledged appends in the
/// consolidation crash test that caught it. The re-log must keep the tail.
#[test]
fn a_relogged_sealed_record_keeps_the_live_tail() {
    let path = dir("relog").join("s.supdb");
    let o = Options {
        shards: 1,
        buffer_bytes: 4 << 10, // tiny: appends seal under pressure
        ..opts()
    };
    let store = Store::create(&path, o.clone()).unwrap();
    store.checkpoint().unwrap();
    let sb = snapshot_sb(&path);
    // Enough to seal (extents + dirty), then a tail that stays pending.
    for i in 0..40u32 {
        store.append(b"k", format!("v{i:03}").as_bytes()).unwrap();
    }
    store.checkpoint().unwrap(); // logs Sealed(+tail Value)
    store.append(b"other", b"x").unwrap();
    store.checkpoint().unwrap(); // k still dirty: Sealed RE-logged after k's tail
    std::mem::forget(store);
    crash_to(&path, &sb);
    let s = Store::open(&path, o).unwrap();
    let got = read_vec(&s, b"k");
    assert_eq!(got.len(), 40, "the re-logged Sealed record dropped the live tail");
    for (i, v) in got.iter().enumerate() {
        assert_eq!(v, format!("v{i:03}").as_bytes(), "order lost at {i}");
    }
    drop(s);
    let r = Reader::open_with(&path, ReadOptions::default()).unwrap();
    assert_eq!(reader_vec(&r, b"k").len(), 40, "fresh reader dropped the tail");
}
