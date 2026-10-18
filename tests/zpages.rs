//! The z form's layout against the partitions the engine itself writes.
//!
//! The layout reads the records of a segment through its directory. Two
//! writers produce segments, and one of them, the close of a direct run,
//! keeps its commit markers inside the record region, between the batches'
//! records: a reader that took a record's length as the difference of two
//! directory words would read a marker as the end of a record. The unit
//! tests of the module build their segments with the sorted writer, which
//! leaves nothing between records, so this file loads a store through `Db`
//! the way the suite loads and holds the layout to what the engine wrote.
//!
//! It is a test of the crate's own and not one of the module's unit tests
//! because a `Db` is threads, a log and fsyncs, and the unit tests are what
//! the emulated ARM job runs (`cargo test --lib` under qemu-user), which has
//! not been asked to open a store; the native legs run this file.

use std::path::{Path, PathBuf};
use supdb::db::zpages::{
    bind32, decode_group, fill_group, parse_page, verify_page, FenceEntry, Fill, FillScratch,
    GroupBuf, Images, SegImages, ZPAGE,
};
use supdb::flatindex::{record_key, stream_record_as, FlatIndex};
use supdb::{Blob, Bytes, Db, MmapBytes, Options};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-zpages-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fixed xorshift stream: the values written and the shuffle.
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

/// What the layout makes of one segment of the engine's.
struct Segment {
    /// Records, and how many of them are compact.
    records: usize,
    compact: usize,
    /// Bytes of the record region that lie between records.
    gap: usize,
    /// Pages built over the records, and records no page holds.
    pages: usize,
    declined: usize,
}

/// The records of the segment at `path` as images, each held to the bytes
/// the writer's own encoder makes of the record the reader reads back, and
/// the pages `fill_group` builds over them, each decoded and held to its
/// images and to the check a reader's first decode makes.
fn check_segment(path: &Path) -> Segment {
    let src = MmapBytes::open(path).unwrap();
    let blob = Blob::open(MmapBytes::open(path).unwrap()).unwrap();
    let off = blob.index_offset();
    let sec = src.slice_at(off, (src.len() - off) as usize).unwrap();
    let idx = FlatIndex::parse(sec).expect("a flat index");
    let (recs, dir) = idx.regions(sec).unwrap();
    let n = blob.keys();
    let images = SegImages::new(recs, dir, n);
    assert_eq!(images.records(), n);
    let (mut compact, mut gap, mut end) = (0usize, 0usize, None::<usize>);
    for r in 0..n {
        let stored = images.image(r);
        let (key, exts, tail) = blob.exts_at_full(r).expect("a record reads back");
        let mut made = Vec::new();
        stream_record_as(&mut made, key, &exts, tail, true)
            .expect("a record the engine read is one it can write");
        assert_eq!(
            stored,
            &made[..],
            "rank {r}: the stored record is the streamed one"
        );
        compact += usize::from(stored[3] & 0x80 != 0);
        let at = idx.record_off(dir, r).expect("a directory word");
        if let Some(e) = end {
            assert!(at >= e, "records ascend with rank in a segment");
            gap += at - e;
        }
        end = Some(at + stored.len());
    }
    let bind = bind32(77, 1, n as u64, 0);
    let (mut s, mut buf) = (FillScratch::new(), GroupBuf::new());
    let (mut rank, mut hint, mut pages, mut declined) = (0usize, 0usize, 0usize, 0usize);
    while rank < n {
        let mut page = [0u8; ZPAGE];
        match fill_group(&images, rank, hint, bind, &mut s, &mut page) {
            Fill::Page(g) => {
                let group: Vec<u8> = (rank..rank + g.count)
                    .flat_map(|r| images.image(r).iter().copied())
                    .collect();
                let (h, payload) = parse_page(&page, bind).expect("a page fill_group built");
                assert_eq!(
                    decode_group(&h, payload, &mut buf).unwrap(),
                    &group[..],
                    "rank {rank}: the page holds the images"
                );
                let entry = FenceEntry {
                    index: pages,
                    first_rank: rank as u32,
                    count: g.count as u32,
                    page: pages as u32,
                    key: record_key(images.image(rank), 0).unwrap(),
                };
                assert_eq!(
                    verify_page(&page, bind, &entry, &mut buf).map(|h| usize::from(h.count)),
                    Ok(g.count),
                    "rank {rank}"
                );
                pages += 1;
                rank += g.count;
                hint = g.count;
            }
            Fill::NoPage(_) => {
                declined += 1;
                rank += 1;
            }
            other => panic!("rank {rank} of {n}: {other:?}"),
        }
    }
    Segment {
        records: n,
        compact,
        gap,
        pages,
        declined,
    }
}

#[test]
fn the_images_of_partitions_the_engine_wrote_are_the_streamed_records() {
    for (name, shuffled) in [("ordered", false), ("shuffled", true)] {
        let dir = scratch(name);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let n = 6000usize;
        let mut order: Vec<usize> = (0..n).collect();
        if shuffled {
            for i in (1..n).rev() {
                order.swap(i, rng.below(i + 1));
            }
        }
        let mut db = Db::create(&dir, Options::default()).unwrap();
        for chunk in order.chunks(500) {
            for &i in chunk {
                let v: Vec<u8> = (0..100).map(|_| (rng.next() >> 24) as u8).collect();
                db.append(format!("{i:016}").as_bytes(), &v);
            }
            db.commit().unwrap();
        }
        db.flush().unwrap();
        db.close().unwrap();
        let mut segs: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "sup"))
            .collect();
        segs.sort();
        assert!(!segs.is_empty(), "{name}: the flush left a segment");
        let (mut total, mut between) = (0, 0);
        for p in &segs {
            let seg = check_segment(p);
            eprintln!(
                "{name}: {} holds {} records, {} compact, {} bytes between records, {} pages, {} declined",
                p.file_name().unwrap().to_string_lossy(),
                seg.records,
                seg.compact,
                seg.gap,
                seg.pages,
                seg.declined
            );
            assert_eq!(seg.declined, 0);
            assert_eq!(seg.compact, seg.records, "the suite's records are compact");
            assert!(seg.pages > 0);
            total += seg.records;
            between += seg.gap;
        }
        assert_eq!(total, n);
        // The case this test exists for is a record region with something
        // other than records in it, and whether the engine leaves it there
        // is the engine's: say so when the fixture stops covering it. A
        // direct run's partitions keep a commit marker between batches; the
        // memtable's seal writes the records and nothing else.
        if shuffled {
            assert_eq!(between, 0, "{name}: a sealed memtable has no markers");
        } else {
            assert!(
                between > 0,
                "{name}: the direct run's markers are no longer in the record region, so \
                 this test no longer covers a directory that skips bytes between records"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ---- the forced arm ---------------------------------------------------------
//
// A store under `ZPages::Always` and one under `Off`, fed the same operations,
// answer every read and count as a model of the operations does; and the one
// under `Always` answers the partitions' from its sidecars, which each test
// says by counting: a path only the arm exercises is a path nothing tests.

use std::collections::BTreeMap;
use supdb::db::zpages::{Trailer, ZFence, ZTRAILER};
use supdb::{Reader, ZPages};

/// A key's values, in the order the store hands them.
type Model = BTreeMap<Vec<u8>, Vec<Vec<u8>>>;

/// A store forced onto the second form, with the floor lowered so that the
/// partitions a test can afford to write are given sidecars.
fn forced(extra: impl FnOnce(&mut Options)) -> Options {
    let mut o = Options {
        zpages: ZPages::Always,
        zpages_min_bytes: 0,
        ..Options::default()
    };
    extra(&mut o);
    o
}

fn read_vec(r: &Reader, key: &[u8]) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    let n = r.read_all(key, |x| v.push(x.to_vec())).unwrap();
    assert_eq!(n as usize, v.len(), "the count read_all returns");
    v
}

/// Every key in `keys` read and counted through `r`, as `model` has it.
fn check(r: &Reader, model: &Model, keys: &[Vec<u8>], what: &str) {
    for k in keys {
        let want = model.get(k).cloned().unwrap_or_default();
        assert_eq!(
            read_vec(r, k),
            want,
            "{what}: read of {:?}",
            String::from_utf8_lossy(k)
        );
        assert_eq!(
            r.count(k).unwrap() as usize,
            want.len(),
            "{what}: count of {:?}",
            String::from_utf8_lossy(k)
        );
    }
}

/// Every key the model holds, scanned, is what the model says.
fn check_scan(r: &Reader, model: &Model, what: &str) {
    let mut scanned: Model = BTreeMap::new();
    r.scan(b"", usize::MAX, |k, v| {
        scanned.entry(k.to_vec()).or_default().push(v.to_vec())
    })
    .unwrap();
    let live: Model = model
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(scanned, live, "{what}: the scan");
}

/// The sidecars in a store's directory, by name.
fn sidecar_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("zpg-"))
        })
        .collect();
    v.sort();
    v
}

/// The keys a sidecar's fence names, read out of its file by the layout's
/// own decoder.
fn fence_keys(path: &Path) -> Vec<Vec<u8>> {
    let bytes = std::fs::read(path).unwrap();
    let t = Trailer::decode(&bytes[bytes.len() - ZTRAILER..]).unwrap();
    let (fence, _) =
        ZFence::decode_resident(&bytes[t.res_off as usize..bytes.len() - ZTRAILER], &t).unwrap();
    (0..fence.len())
        .map(|i| fence.key(i).unwrap().to_vec())
        .collect()
}

fn apply(model: &mut Model, batch: &mut Vec<(Vec<u8>, Option<Vec<u8>>)>) {
    for (k, v) in batch.drain(..) {
        let e = model.entry(k).or_default();
        match v {
            Some(v) => e.push(v),
            None => e.clear(),
        }
    }
}

const ORACLE_KEYS: usize = 240;

fn oracle_opts(z: bool, inline: bool) -> Options {
    Options {
        zpages: if z { ZPages::Always } else { ZPages::Off },
        zpages_min_bytes: 0,
        seal_bytes: 8 << 10,
        partition_bytes: Some(4 << 10),
        inline_bytes: if inline {
            1 << 16
        } else {
            Options::default().inline_bytes
        },
        l0_trigger: 2,
        ..Options::default()
    }
}

fn oracle_keys() -> Vec<Vec<u8>> {
    // Past the keys written too: a key nobody wrote is absent.
    (0..ORACLE_KEYS + 20)
        .map(|i| format!("k{i:04}").into_bytes())
        .collect()
}

#[test]
fn the_oracle_holds_with_zpages_forced_on() {
    oracle(true);
}

#[test]
fn the_oracle_holds_when_runs_in_blocks_decline_the_partitions() {
    // The same operations at the engine's own inline limit: a key that has
    // collected a few hundred bytes of values has its run in a block, and a
    // partition with one is declined and read raw.
    oracle(false);
}

fn oracle(inline: bool) {
    // Puts, multi-value keys, deletes, seals, merges, promotions and
    // reopens, on a store under `Always` and a store under `Off`, each
    // checked against the model after every settle: through the writer, and
    // through a handle made for the purpose.
    let tag = if inline { "inline" } else { "blocks" };
    let (dz, dr) = (
        scratch(&format!("oracle-z-{tag}")),
        scratch(&format!("oracle-raw-{tag}")),
    );
    let mut z = Db::create(&dz, oracle_opts(true, inline)).unwrap();
    let mut raw = Db::create(&dr, oracle_opts(false, inline)).unwrap();
    let mut model: Model = BTreeMap::new();
    let mut pending: Vec<(Vec<u8>, Option<Vec<u8>>)> = Vec::new();
    let mut rng = Rng(0x0AC1E);
    let keys = oracle_keys();
    let (mut z_reads, mut crashes, mut settles) = (0u64, 0, 0);
    let both = |z: &mut Db, raw: &mut Db, f: &dyn Fn(&mut Db)| {
        f(z);
        f(raw);
    };
    for step in 0..3000u64 {
        let key = format!("k{:04}", rng.below(ORACLE_KEYS)).into_bytes();
        match rng.below(20) {
            0 => {
                both(&mut z, &mut raw, &|d| d.delete(&key));
                pending.push((key, None));
            }
            1..=3 => {
                // A key with several values at once, some of them empty.
                for j in 0..2 + rng.below(4) {
                    let v = if j == 1 {
                        Vec::new()
                    } else {
                        format!("m{step}.{j}").into_bytes()
                    };
                    both(&mut z, &mut raw, &|d| d.append(&key, &v));
                    pending.push((key.clone(), Some(v)));
                }
            }
            _ => {
                let pad = rng.below(80);
                let v = format!("v{step}-{}", "p".repeat(pad)).into_bytes();
                both(&mut z, &mut raw, &|d| d.append(&key, &v));
                pending.push((key, Some(v)));
            }
        }
        match rng.below(100) {
            0..=9 => {
                z.commit().unwrap();
                raw.commit().unwrap();
                apply(&mut model, &mut pending);
            }
            10..=12 => {
                z.commit().unwrap();
                raw.commit().unwrap();
                apply(&mut model, &mut pending);
                z.seal().unwrap();
                raw.seal().unwrap();
            }
            13 if step % 5 == 0 => {
                // A crash: what was not committed vanishes, on both.
                z_reads += z.zpage_counts().reads;
                drop(z);
                drop(raw);
                pending.clear();
                z = Db::open(&dz, oracle_opts(true, inline)).unwrap();
                raw = Db::open(&dr, oracle_opts(false, inline)).unwrap();
                crashes += 1;
                check(&z, &model, &keys, "after a crash");
                check(&raw, &model, &keys, "after a crash, raw");
            }
            _ => {}
        }
        if step % 250 == 249 {
            z.commit().unwrap();
            raw.commit().unwrap();
            apply(&mut model, &mut pending);
            if step % 500 == 499 {
                // A drain: the memtable into partitions, by promotion where
                // the engine can.
                z.flush().unwrap();
                raw.flush().unwrap();
            }
            z.zpages_settle().unwrap();
            raw.settle().unwrap();
            settles += 1;
            check(&z, &model, &keys, "z, writer");
            check(&raw, &model, &keys, "raw, writer");
            let (hz, hr) = (z.reader().unwrap(), raw.reader().unwrap());
            check(&hz, &model, &keys, "z, handle");
            check(&hr, &model, &keys, "raw, handle");
            check_scan(&z, &model, "z");
        }
    }
    z.commit().unwrap();
    raw.commit().unwrap();
    apply(&mut model, &mut pending);
    z.zpages_settle().unwrap();
    raw.settle().unwrap();
    let before = z.zpage_counts();
    check(&z, &model, &keys, "z, at the end");
    check(&raw, &model, &keys, "raw, at the end");
    let snap = z.reader().unwrap();
    snap.snapshot();
    check(&snap, &model, &keys, "z, a snapshot");
    // The path was taken: the partitions' reads and counts were answered
    // from sidecars, every partition has one, and the store under `Off`
    // never touched the form.
    let c = z.zpage_counts();
    z_reads += c.reads;
    assert!(c.partitions > 2, "the oracle made partitions: {c:?}");
    assert!(crashes > 0 && settles > 8);
    assert_eq!(
        c.rejected_damage + c.rejected_identity + c.fallbacks,
        0,
        "{c:?}"
    );
    assert_eq!(c.damaged + c.bad_pages, 0, "{c:?}");
    if inline {
        assert_eq!(
            c.sidecars, c.partitions,
            "every partition has a sidecar: {c:?}"
        );
        assert_eq!(c.declined_block_backed, 0, "{c:?}");
        assert!(c.reads > before.reads && c.counts > before.counts, "{c:?}");
        assert!(z_reads > 0);
    } else {
        // Runs in blocks decline their partitions, and those are read raw
        // and counted; what a sidecar was built for answers from it.
        assert!(c.declined_block_backed > 0, "{c:?}");
        assert!(c.rawserves > 0 && c.sidecars <= c.partitions, "{c:?}");
    }
    // The partitions are counted whichever form the store is in; nothing
    // else is, under `Off`.
    let off = raw.zpage_counts();
    assert!(off.partitions > 0);
    assert_eq!(
        supdb::db::ZCounts {
            partitions: 0,
            ..off
        },
        Default::default()
    );
    assert!(
        sidecar_files(&dr).is_empty(),
        "the store under Off wrote none"
    );
    let _ = std::fs::remove_dir_all(&dz);
    let _ = std::fs::remove_dir_all(&dr);
}
