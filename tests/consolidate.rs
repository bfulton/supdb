//! Deferred consolidation against the inline policy, on every path that
//! publishes, replays or reads an extent list.
//!
//! `Options::defer_merge` changes *when* a fragmented key is rewritten and
//! how much of it, and nothing else. Everything observable through the public
//! API -- values, their order, scans, counts, what survives a crash -- must
//! be identical between the two policies. The tests here are differential
//! wherever possible, because the failure mode of a merge policy is not a
//! crash: it is a key quietly serving its values out of order, or a tombstone
//! quietly dropped, and this repository has had both.
//!
//! The shapes lean on the lessons already paid for in `tests/known_bugs.rs`:
//! a path is only covered if the test *proves* it was taken (merges counted,
//! fragments observed), a reopen is what engages the in-place checkpoint, and
//! a crash is `mem::forget`, never `close`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use supdb::{Options, Reader, Reclaim, Store};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-consolidate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

/// A buffer small enough that interleaved appends seal constantly, which is
/// what fragments every key and drives the merge path under test.
fn opts(defer: bool) -> Options {
    Options {
        defer_merge: defer,
        buffer_bytes: 1 << 16,
        reclaim: Reclaim::AfterReads,
        ..Default::default()
    }
}

/// The two policies must be indistinguishable through the public API.
///
/// Interleaved appends fragment every key; replacements and deletes cross the
/// merge path with the two operations that have historically lost data around
/// extent-list rewrites (delete resurrection, dropped tombstones); and
/// checkpoints land mid-stream so sealed, staged and pending state all exist
/// at once. Both arms answer through the writer's own paths and through a
/// fresh `Reader`, and the arms are compared against each other as well as
/// against the model -- the stronger assertion, since it does not bake in a
/// belief about either one.
#[test]
fn deferred_consolidation_changes_no_answer() {
    type Seen = (Vec<(Vec<u8>, Vec<u8>)>, Vec<(Vec<u8>, Vec<u8>)>);
    fn run(defer: bool, dir: &Path) -> (Seen, u64) {
        let path = dir.join("s.dat");
        let s = Store::create(&path, opts(defer)).unwrap();
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        // Line order: every key's run is broken by every other key's, the
        // shape W1.3 measures at 18x on the space axis.
        for round in 0..60u32 {
            for k in 0..25u32 {
                let key = format!("key-{k:04}").into_bytes();
                let val = format!("v{k}-{round}-padding-to-make-seals-happen");
                s.append(&key, val.as_bytes()).unwrap();
                model
                    .entry(key)
                    .or_default()
                    .extend_from_slice(val.as_bytes());
            }
            // Checkpoints mid-stream, so merges land before, between and
            // after publications rather than only at the end.
            if round % 17 == 16 {
                s.checkpoint().unwrap();
            }
            // A replacement collapses one key mid-fragmentation...
            if round % 20 == 19 {
                let key = format!("key-{:04}", round % 25).into_bytes();
                s.put(&key, b"replaced").unwrap();
                model.insert(key, b"replaced".to_vec());
            }
            // ...and a delete must stay deleted through every later merge.
            if round == 30 {
                let key = b"key-0007".to_vec();
                s.delete(&key).unwrap();
                model.remove(&key);
            }
        }
        // What the writer sees, one entry per value and one per key.
        let mut scanned = Vec::new();
        s.scan(None, usize::MAX, |k, v| {
            scanned.push((k.to_vec(), v.to_vec()));
        })
        .unwrap();
        let mut whole = Vec::new();
        for k in model.keys() {
            let mut got = Vec::new();
            s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            whole.push((k.clone(), got));
        }
        for (k, v) in &whole {
            assert_eq!(
                model.get(k),
                Some(v),
                "defer={defer}: read_all disagrees with the model for {:?}",
                String::from_utf8_lossy(k)
            );
        }
        s.checkpoint().unwrap();
        let stats = s.close().unwrap();
        // And the same questions through a fresh reader after sealing.
        let r = Reader::open(&path).unwrap();
        for (k, v) in &whole {
            let mut got = Vec::new();
            r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
            assert_eq!(
                &got,
                v,
                "defer={defer}: the writer and a fresh reader disagree on {:?}",
                String::from_utf8_lossy(k)
            );
        }
        ((scanned, whole), stats.merges)
    }

    let base = scratch("differential");
    let (a, b) = (base.join("deferred"), base.join("inline"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let (inline_seen, inline_merges) = run(false, &b);
    let (deferred_seen, deferred_merges) = run(true, &a);
    // A clean result proves nothing about a path the test never took: both
    // arms must actually have merged, or the comparison compared nothing.
    assert!(
        inline_merges > 0 && deferred_merges > 0,
        "the shape stopped exercising the merge path: inline {inline_merges}, \
         deferred {deferred_merges} merges"
    );
    assert_eq!(
        inline_seen.0, deferred_seen.0,
        "the merge policy changed what a scan yields"
    );
    assert_eq!(
        inline_seen.1, deferred_seen.1,
        "the merge policy changed what read_all returns"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// Reopen, then fragment, then checkpoint in place -- with merges deferred.
///
/// The in-place checkpoint and the redo log publish only what `dirty` names,
/// and a suffix merge rewrites a key's extent list outside any append, so a
/// merge that forgot to mark its key dirty would leave the published index
/// pointing at extents the merge just released. It needs a reopen to show,
/// because a fresh store takes the full-rewrite path -- the same masking that
/// hid the dropped-tombstone bug. `scan` and `read_all` take different routes
/// through the flat index (directory against hash slot), so both are asked.
#[test]
fn a_deferred_merge_is_published_by_an_in_place_checkpoint() {
    let base = scratch("inplace");
    let path = base.join("s.dat");
    let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    {
        let s = Store::create(&path, opts(true)).unwrap();
        for k in 0..20u32 {
            let key = format!("key-{k:04}").into_bytes();
            s.append(&key, b"seed").unwrap();
            model.insert(key, b"seed".to_vec());
        }
        s.checkpoint().unwrap();
        s.close().unwrap();
    }
    // Reopened, so an index section exists and the in-place path can engage.
    let s = Store::open(&path, opts(true)).unwrap();
    for round in 0..40u32 {
        for k in 0..20u32 {
            let key = format!("key-{k:04}").into_bytes();
            let val = format!("-r{round}-value-long-enough-to-seal-often");
            s.append(&key, val.as_bytes()).unwrap();
            model
                .get_mut(&key)
                .unwrap()
                .extend_from_slice(val.as_bytes());
        }
        if round % 8 == 7 {
            s.checkpoint().unwrap();
        }
    }
    s.checkpoint().unwrap();
    let stats = s.close().unwrap();
    assert!(
        stats.merges > 0,
        "no merge fired, so this exercised nothing"
    );

    let r = Reader::open(&path).unwrap();
    let mut walked = 0usize;
    let mut per_key: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    r.scan(None, usize::MAX, |k, v| {
        walked += 1;
        per_key.entry(k.to_vec()).or_default().extend_from_slice(v);
    })
    .unwrap();
    assert_eq!(
        per_key, model,
        "scan through the published index disagrees with the model"
    );
    assert!(
        walked > model.len(),
        "scan yielded one entry per key, not per value"
    );
    for (k, v) in &model {
        let mut got = Vec::new();
        r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
        assert_eq!(
            &got,
            v,
            "read_all is stale for {:?}",
            String::from_utf8_lossy(k)
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// Crash-reopen replays fragmented keys correctly under deferred merging.
///
/// Every log record carries its checkpoint's generation inside the CRC'd
/// frame and replay applies it only over older index state; a suffix merge
/// changes the extent list a record carries but must not disturb any of
/// that. The crash is `mem::forget` -- a close would drop the arena and hide
/// the replay path entirely -- and a deleted key is in the mix because
/// resurrection is how replay bugs here have always presented.
#[test]
fn crash_reopen_replays_deferred_fragments() {
    // The identical, deterministic workload, once per destination. `merges`
    // is only reported by `close()`, and closing is exactly what a crash test
    // must not do -- so a probe run in a sibling directory closes cleanly and
    // proves the shape merges, and the crash run is trusted to have done the
    // same because nothing between the two differs but the ending.
    fn write_workload(path: &Path) -> (Store, BTreeMap<Vec<u8>, Vec<u8>>) {
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        let s = Store::create(path, opts(true)).unwrap();
        for k in 0..30u32 {
            let key = format!("key-{k:04}").into_bytes();
            s.append(&key, b"seed").unwrap();
            model.insert(key, b"seed".to_vec());
        }
        s.checkpoint().unwrap();
        // Fragment across several logged checkpoints, so replayed records
        // carry multi-extent lists shaped by the deferred policy.
        for round in 0..30u32 {
            for k in 0..30u32 {
                let key = format!("key-{k:04}").into_bytes();
                let val = format!("-r{round}-value-long-enough-to-seal-often");
                s.append(&key, val.as_bytes()).unwrap();
                model
                    .get_mut(&key)
                    .unwrap()
                    .extend_from_slice(val.as_bytes());
            }
            if round % 6 == 5 {
                s.checkpoint().unwrap();
            }
        }
        s.delete(b"key-0003").unwrap();
        model.remove(b"key-0003".as_slice());
        s.checkpoint().unwrap();
        (s, model)
    }

    let base = scratch("crash");
    {
        let (probe, _) = write_workload(&base.join("probe.dat"));
        let stats = probe.close().unwrap();
        assert!(
            stats.merges > 0,
            "the workload never merged, so the crash run would prove nothing"
        );
    }
    let path = base.join("s.dat");
    let model = {
        let (s, model) = write_workload(&path);
        std::mem::forget(s); // crash, not close
        model
    };
    // Both replay paths must reconstruct the acknowledged state.
    let s = Store::open(&path, opts(true)).unwrap();
    for (k, v) in &model {
        let mut got = Vec::new();
        s.read_all(k, |x| got.extend_from_slice(x)).unwrap();
        assert_eq!(
            &got,
            v,
            "Store::open replay lost or reordered {:?}",
            String::from_utf8_lossy(k)
        );
    }
    let mut n = 0usize;
    s.read_all(b"key-0003", |v| n += v.len()).unwrap();
    assert_eq!(n, 0, "the deleted key came back through Store::open replay");
    std::mem::forget(s);

    let r = Reader::open(&path).unwrap();
    for (k, v) in &model {
        let mut got = Vec::new();
        r.read_all(k, |x| got.extend_from_slice(x)).unwrap();
        assert_eq!(
            &got,
            v,
            "Reader replay disagrees for {:?}",
            String::from_utf8_lossy(k)
        );
    }
    let mut n = 0usize;
    r.read_all(b"key-0003", |v| n += v.len()).unwrap();
    assert_eq!(n, 0, "the deleted key came back through Reader replay");
    let _ = std::fs::remove_dir_all(&base);
}
