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

// ---- stores the tests below start from --------------------------------------

/// A value of the suite's shape: a hundred bytes, half a repeated phrase at
/// a phase drawn for each and half entropy.
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

/// `n` keys of the suite's shape in ascending order, three apart so that
/// there is a key between any two, committed in batches and flushed into
/// partitions, and the model of them.
fn ordered_load(db: &mut Db, n: usize, seed: u64) -> Model {
    let mut rng = Rng(seed);
    let mut model = Model::new();
    for chunk in (0..n).collect::<Vec<_>>().chunks(500) {
        for &i in chunk {
            let (k, v) = (spaced_key(i), suite_value(&mut rng));
            db.append(&k, &v);
            model.insert(k, vec![v]);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    model
}

/// A store of one partition, forced, loaded, given its sidecar, and closed:
/// its directory, the model, and the keys of the model.
fn one_partition(name: &str, n: usize, seed: u64) -> (PathBuf, Model, Vec<Vec<u8>>) {
    let dir = scratch(name);
    let mut db = Db::create(&dir, forced(|_| {})).unwrap();
    let model = ordered_load(&mut db, n, seed);
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!((c.partitions, c.sidecars, c.builds), (1, 1, 1), "{c:?}");
    db.close().unwrap();
    let keys = model.keys().cloned().collect();
    (dir, model, keys)
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// Keys around `k`: it, one byte either side of it, a proper prefix of it,
/// and two keys that extend it.
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

#[test]
fn absent_keys_are_absent_through_the_sidecar() {
    // Below the first key, above the last, between two records, one byte
    // either side of every fence key, a proper prefix of one, and a key
    // longer than a fence key that it extends: each answers as the model
    // does, from a partition that holds a sidecar.
    let dir = scratch("absent");
    let mut db = Db::create(&dir, forced(|o| o.partition_bytes = Some(160 << 10))).unwrap();
    let n = 6000;
    let model = ordered_load(&mut db, n, 0xAB5E);
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(c.partitions >= 3 && c.sidecars == c.partitions, "{c:?}");
    let mut probes: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"\x00".to_vec(),
        b"a".to_vec(),
        b"key".to_vec(),
        b"key-".to_vec(),
        b"key-0".to_vec(),
        b"zzz".to_vec(),
        b"\xff\xff".to_vec(),
        format!("key-{:08}", n * 3).into_bytes(),
        format!("key-{:08}", n * 3 + 1).into_bytes(),
    ];
    for f in sidecar_files(&dir) {
        for fk in fence_keys(&f) {
            probes.extend(around(&fk));
        }
    }
    for i in (0..n).step_by(37) {
        let k = spaced_key(i);
        probes.extend([[k.clone(), b"x".to_vec()].concat()]);
        probes.push(format!("key-{:08}", i * 3 + 1).into_bytes());
        probes.push(format!("key-{:08}", i * 3 + 2).into_bytes());
    }
    let absent = probes.iter().filter(|p| !model.contains_key(*p)).count();
    assert!(
        absent > probes.len() / 2 && absent < probes.len(),
        "present and absent both"
    );
    let before = db.zpage_counts();
    check(&db, &model, &probes, "absent keys");
    let after = db.zpage_counts();
    assert_eq!(after.reads - before.reads, probes.len() as u64, "{after:?}");
    assert_eq!(after.counts - before.counts, probes.len() as u64);
    assert_eq!(after.rawserves + after.fallbacks, 0);
    // The store reads the fence keys it was given: one more sidecar than
    // pages of one is not a fence.
    assert!(sidecar_files(&dir).len() as u64 == c.sidecars);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_of_every_shape_reads_the_same() {
    // One value, two, seventeen, two hundred and fifteen hundred; a fixed
    // width and a prefixed run; zero-length values; a key deleted, and a key
    // deleted and written again. All inline, so the partition is given its
    // sidecar; the same operations on a store under `Off` and the model.
    let build = |z: bool, name: &str| -> (Db, PathBuf) {
        let dir = scratch(name);
        let mut o = forced(|o| o.inline_bytes = 1 << 16);
        if !z {
            o.zpages = ZPages::Off;
        }
        (Db::create(&dir, o).unwrap(), dir)
    };
    let (mut z, dz) = build(true, "shapes-z");
    let (mut raw, dr) = build(false, "shapes-raw");
    let mut model = Model::new();
    let mut rng = Rng(0x5A9E5);
    let mut put = |z: &mut Db, raw: &mut Db, k: &[u8], v: &[u8]| {
        z.append(k, v);
        raw.append(k, v);
        model.entry(k.to_vec()).or_default().push(v.to_vec());
    };
    for n in [1usize, 2, 17, 200, 1500] {
        for i in 0..n {
            put(
                &mut z,
                &mut raw,
                format!("fixed-{n:04}").as_bytes(),
                &[(i % 3) as u8, 0, 0, 7],
            );
            put(
                &mut z,
                &mut raw,
                format!("varied-{n:04}").as_bytes(),
                &vec![b'a' + (i % 3) as u8; i % 7],
            );
        }
    }
    for _ in 0..5 {
        put(&mut z, &mut raw, b"zeros", b"");
    }
    put(&mut z, &mut raw, b"zero", b"");
    for i in 0..400 {
        let v = suite_value(&mut rng);
        put(&mut z, &mut raw, format!("suite-{i:05}").as_bytes(), &v);
    }
    put(&mut z, &mut raw, b"gone", b"here once");
    put(&mut z, &mut raw, b"again", b"before the delete");
    z.commit().unwrap();
    raw.commit().unwrap();
    for d in [&mut z, &mut raw] {
        d.delete(b"gone");
        d.delete(b"again");
        d.append(b"again", b"after the delete");
    }
    model.remove(b"gone".as_slice());
    model.insert(b"again".to_vec(), vec![b"after the delete".to_vec()]);
    z.commit().unwrap();
    raw.commit().unwrap();
    z.flush().unwrap();
    raw.flush().unwrap();
    z.zpages_settle().unwrap();
    let c = z.zpage_counts();
    assert!(c.partitions >= 1 && c.sidecars == c.partitions, "{c:?}");
    let mut keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    keys.extend([b"gone".to_vec(), b"nothing".to_vec(), b"zeros2".to_vec()]);
    check(&z, &model, &keys, "z");
    check(&raw, &model, &keys, "raw");
    check(&z.reader().unwrap(), &model, &keys, "z, a handle");
    let after = z.zpage_counts();
    assert!(after.reads >= keys.len() as u64 * 2 && after.counts >= keys.len() as u64 * 2);
    assert_eq!(after.fallbacks + after.rawserves, 0, "{after:?}");
    assert_eq!(raw.zpage_counts().reads + raw.zpage_counts().counts, 0);
    // A run of fifteen hundred values is read whole.
    assert_eq!(read_vec(&z, b"fixed-1500").len(), 1500);
    let _ = std::fs::remove_dir_all(&dz);
    let _ = std::fs::remove_dir_all(&dr);
}

#[test]
fn a_direct_runs_segment_with_markers_inside_its_records_has_a_sidecar() {
    // An ordered load closes into a partition whose record region keeps the
    // commit markers of its batches between records. A build that took a
    // record's length as the difference of two directory words would read a
    // marker as a record, and fail the page it is in.
    let dir = scratch("markers");
    let mut db = Db::create(&dir, forced(|_| {})).unwrap();
    let model = ordered_load(&mut db, 9000, 0x3A7);
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(c.partitions >= 1 && c.sidecars == c.partitions, "{c:?}");
    assert_eq!(
        c.build_failed + c.declined_unplaceable + c.declined_oversize,
        0,
        "{c:?}"
    );
    let mut markers = 0;
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "sup") {
            markers += check_segment(&p).gap;
        }
    }
    assert!(
        markers > 0,
        "the markers are no longer in the record region: this test no longer \
         covers a directory that skips bytes between records"
    );
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    let before = db.zpage_counts();
    check(&db, &model, &keys, "markers");
    let after = db.zpage_counts();
    assert_eq!(after.reads - before.reads, keys.len() as u64);
    assert_eq!(after.fallbacks + after.rawserves, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- a sidecar that is damaged, or is another's ----------------------------------

/// A copy of `dir` opened under `Always`, after `change` has been applied
/// to the copy's sidecar file.
fn open_changed(dir: &Path, name: &str, change: impl FnOnce(&Path)) -> Db {
    let copy = scratch(name);
    copy_dir(dir, &copy);
    let files = sidecar_files(&copy);
    assert_eq!(files.len(), 1);
    change(&files[0]);
    Db::open(&copy, forced(|_| {})).unwrap()
}

/// The keys of a one-partition store that a sidecar's page `p` holds, and a
/// few from elsewhere: the reads that tell a damaged page from a sound one.
fn keys_of_page(sidecar: &Path, p: usize, all: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let fence = fence_keys(sidecar);
    let (lo, hi) = (&fence[p], fence.get(p + 1));
    let mut v: Vec<Vec<u8>> = all
        .iter()
        .filter(|k| *k >= lo && hi.is_none_or(|h| *k < h))
        .cloned()
        .collect();
    assert!(!v.is_empty());
    v.extend(all.iter().step_by(41).cloned());
    v
}

#[test]
fn a_flipped_byte_in_a_sidecar_is_never_an_answer() {
    // Bytes of the page region, one at a time, in a copy: a flip in a page
    // the attach samples is a refusal and the partition is read raw; a flip
    // in any other is met at the page's first decode, which reads the raw
    // form and counts it, marks the page bad and takes the sidecar out of
    // service. Either way every read answers as the model does, and the
    // rejection or the fallback is counted.
    let (dir, model, keys) = one_partition("flips-src", 4500, 0xF11B);
    let file = sidecar_files(&dir).remove(0);
    let good = std::fs::read(&file).unwrap();
    let t = Trailer::decode(&good[good.len() - ZTRAILER..]).unwrap();
    let pages = t.n_pages as usize;
    assert!(pages > 40, "{pages} pages");
    let sampled: Vec<usize> = (0..16).map(|k| k * (pages - 1) / 15).collect();
    let work = scratch("flips-work");
    copy_dir(&dir, &work);
    let target = sidecar_files(&work).remove(0);
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&target)
        .unwrap();
    use std::os::unix::fs::FileExt;
    let (mut refused, mut fell_back) = (0, 0);
    for at in (0..pages * ZPAGE).step_by(997) {
        let p = at / ZPAGE;
        f.write_at(&[good[at] ^ 0x40], at as u64).unwrap();
        let db = Db::open(&work, forced(|_| {})).unwrap();
        let probe = keys_of_page(&target, p, &keys);
        check(&db, &model, &probe, &format!("byte {at}"));
        let c = db.zpage_counts();
        if sampled.contains(&p) {
            assert_eq!(
                (c.rejected_damage, c.sidecars, c.reads),
                (1, 0, 0),
                "byte {at}: {c:?}"
            );
            assert_eq!(c.rawserves, 2 * probe.len() as u64, "byte {at}: {c:?}");
            assert!(
                db.zpage_last_reject().starts_with("damage:"),
                "{}",
                db.zpage_last_reject()
            );
            refused += 1;
        } else {
            assert_eq!(
                (c.rejected_damage, c.rejected_identity, c.sidecars),
                (0, 0, 1),
                "byte {at}: {c:?}"
            );
            // One read met the page and fell back; the sidecar is out of
            // service for every read after it. The counts are the reads
            // and the counts of the probe, each of them one of the three.
            assert_eq!(
                (c.fallbacks, c.bad_pages, c.damaged),
                (1, 1, 1),
                "byte {at}: {c:?}"
            );
            assert_eq!(
                c.reads + c.counts + c.fallbacks + c.rawserves,
                2 * probe.len() as u64,
                "every read is served, a fallback or served raw: {c:?}"
            );
            fell_back += 1;
        }
        drop(db);
        f.write_at(&[good[at]], at as u64).unwrap();
    }
    assert!(refused > 3 && fell_back > 20, "{refused} {fell_back}");
    // And the sound file reads from the sidecar again.
    let db = Db::open(&work, forced(|_| {})).unwrap();
    check(&db, &model, &keys[..300], "restored");
    let c = db.zpage_counts();
    assert_eq!(
        (c.rejected_damage, c.fallbacks, c.rawserves),
        (0, 0, 0),
        "{c:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn a_file_that_is_not_a_sidecar_is_refused_and_the_partition_is_read_raw() {
    let (dir, model, keys) = one_partition("notside-src", 3000, 0x6001);
    let size = std::fs::metadata(&sidecar_files(&dir)[0]).unwrap().len();
    type Change = Box<dyn Fn(&Path)>;
    let cases: Vec<(&str, Change)> = vec![
        ("empty", Box::new(|p| std::fs::write(p, b"").unwrap())),
        (
            "zeros",
            Box::new(move |p| std::fs::write(p, vec![0u8; size as usize]).unwrap()),
        ),
        (
            "a page short",
            Box::new(move |p| {
                let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
                f.set_len(size - ZPAGE as u64).unwrap();
            }),
        ),
        (
            "a byte long",
            Box::new(move |p| {
                let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
                f.set_len(size + 1).unwrap();
            }),
        ),
        (
            "text",
            Box::new(|p| std::fs::write(p, vec![b'x'; 8192]).unwrap()),
        ),
    ];
    for (name, change) in &cases {
        let db = open_changed(&dir, &format!("notside-{}", name.replace(' ', "-")), |p| {
            change(p)
        });
        let c = db.zpage_counts();
        assert_eq!(
            (c.rejected_damage, c.rejected_identity, c.sidecars),
            (1, 0, 0),
            "{name}: {c:?}"
        );
        check(&db, &model, &keys[..400], name);
        let c = db.zpage_counts();
        assert_eq!((c.reads, c.counts, c.fallbacks), (0, 0, 0), "{name}: {c:?}");
        assert_eq!(
            c.rawserves, 800,
            "{name}: every read of the partition is counted raw"
        );
        // A settle gives the partition the sidecar it lacks.
        let mut db = db;
        db.zpages_settle().unwrap();
        let c = db.zpage_counts();
        assert_eq!((c.sidecars, c.builds), (1, 1), "{name}: {c:?}");
        check(&db, &model, &keys[..400], name);
        assert_eq!(
            db.zpage_counts().reads,
            400,
            "{name}: served from the new one"
        );
    }
    // No file at all is no rejection.
    let copy = scratch("notside-none");
    copy_dir(&dir, &copy);
    std::fs::remove_file(&sidecar_files(&copy)[0]).unwrap();
    let db = Db::open(&copy, forced(|_| {})).unwrap();
    let c = db.zpage_counts();
    assert_eq!(
        (
            c.rejected_damage,
            c.rejected_identity,
            c.attached,
            c.sidecars
        ),
        (0, 0, 0, 0)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_field_of_the_trailer_wrong_is_refused_and_counted() {
    let (dir, model, keys) = one_partition("trailer-src", 3000, 0x7A11);
    type Edit = Box<dyn Fn(&mut Trailer)>;
    let wrong: Vec<(&str, Edit)> = vec![
        ("n_pages", Box::new(|t| t.n_pages += 1)),
        ("n_entries", Box::new(|t| t.n_entries += 1)),
        ("nkeys", Box::new(|t| t.nkeys += 1)),
        ("res_off", Box::new(|t| t.res_off += ZPAGE as u64)),
        ("res_len", Box::new(|t| t.res_len += 1)),
        ("keys_len", Box::new(|t| t.keys_len += 1)),
        ("filter_off", Box::new(|t| t.filter_off = 64)),
        ("filter_blocks", Box::new(|t| t.filter_blocks = 1)),
        ("res_crc", Box::new(|t| t.res_crc ^= 1)),
        ("filter", Box::new(|t| t.filter = true)),
        ("seg_recs_len", Box::new(|t| t.seg_recs_len += 4)),
        ("seg_crc_off", Box::new(|t| t.seg_crc_off += 4)),
        ("seg_key_off", Box::new(|t| t.seg_key_off += 4096)),
        ("seg_hash_cap", Box::new(|t| t.seg_hash_cap *= 2)),
        ("seg_file_len", Box::new(|t| t.seg_file_len += 4096)),
        ("seg_timestamp", Box::new(|t| t.seg_timestamp += 1)),
        ("seg_generation", Box::new(|t| t.seg_generation += 1)),
        ("seg_row_crc", Box::new(|t| t.seg_row_crc ^= 1)),
        ("rec_version", Box::new(|t| t.rec_version += 1)),
        ("rec_magic", Box::new(|t| t.rec_magic ^= 1)),
        ("seg_id", Box::new(|t| t.seg_id += 1)),
        ("seg_end_seq", Box::new(|t| t.seg_end_seq += 1)),
    ];
    let rewrite = |p: &Path, edit: &dyn Fn(&mut Trailer)| {
        let mut bytes = std::fs::read(p).unwrap();
        let at = bytes.len() - ZTRAILER;
        let mut t = Trailer::decode(&bytes[at..]).unwrap();
        edit(&mut t);
        bytes[at..].copy_from_slice(&t.encode());
        std::fs::write(p, bytes).unwrap();
    };
    for (field, edit) in &wrong {
        let mut db = open_changed(&dir, &format!("trailer-{field}"), |p| rewrite(p, edit));
        let c = db.zpage_counts();
        assert_eq!(c.rejected_damage + c.rejected_identity, 1, "{field}: {c:?}");
        assert_eq!((c.sidecars, c.attached), (0, 0), "{field}: {c:?}");
        assert!(!db.zpage_last_reject().is_empty(), "{field}");
        check(&db, &model, &keys[..200], field);
        assert_eq!(db.zpage_counts().rawserves, 400, "{field}");
        db.zpages_settle().unwrap();
        let c = db.zpage_counts();
        assert_eq!((c.sidecars, c.builds), (1, 1), "{field}: {c:?}");
    }
    // The fields no read depends on are accepted: the decoded bytes a trailer
    // sums for the ratio, and a filter's parameters, which a trailer with
    // none is not asked for.
    let harmless: Vec<(&str, Edit)> = vec![
        ("total_ulen", Box::new(|t| t.total_ulen += 1)),
        ("bloom_sig", Box::new(|t| t.bloom_sig = 5)),
        ("bloom_probes", Box::new(|t| t.bloom_probes = 5)),
        ("bloom_bits_per_key", Box::new(|t| t.bloom_bits_per_key = 5)),
    ];
    for (field, edit) in &harmless {
        let db = open_changed(&dir, &format!("trailer-ok-{field}"), |p| rewrite(p, edit));
        let c = db.zpage_counts();
        assert_eq!(
            (c.rejected_damage, c.rejected_identity, c.sidecars),
            (0, 0, 1),
            "{field}: {c:?}"
        );
        check(&db, &model, &keys[..200], field);
        assert_eq!(db.zpage_counts().reads, 200, "{field}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sidecar_of_another_segment_is_refused() {
    // Two stores, the same operations, other values: the same names, ids,
    // end sequences and key counts, and different records. One's sidecar is
    // planted under the other's name, which is what a segment rewritten
    // under the same id and end sequence would meet, and what an id handed
    // out again after a crash would. The file passes its own checks; it is
    // not this segment's.
    let (d1, _, _) = one_partition("other-1", 3000, 0x0111);
    let (d2, m2, k2) = one_partition("other-2", 3000, 0x0222);
    let (f1, f2) = (sidecar_files(&d1).remove(0), sidecar_files(&d2).remove(0));
    assert_eq!(
        f1.file_name(),
        f2.file_name(),
        "the same operations name their segments alike"
    );
    assert_ne!(std::fs::read(&f1).unwrap(), std::fs::read(&f2).unwrap());
    let mut db = open_changed(&d2, "other-planted", |p| {
        std::fs::copy(&f1, p).unwrap();
    });
    let c = db.zpage_counts();
    assert_eq!(
        (c.rejected_identity, c.rejected_damage, c.sidecars),
        (1, 0, 0),
        "{c:?}"
    );
    let why = db.zpage_last_reject();
    assert!(
        why.starts_with("identity:") && why.contains("checksum row"),
        "{why}"
    );
    // The other store's values are never served: this one's are read, raw.
    check(&db, &m2, &k2[..300], "planted");
    let c = db.zpage_counts();
    assert_eq!((c.reads, c.counts, c.rawserves), (0, 0, 600), "{c:?}");
    // And a settle replaces the stranger with a sidecar of its own.
    db.zpages_settle().unwrap();
    check(&db, &m2, &k2[..300], "rebuilt");
    let c = db.zpage_counts();
    assert_eq!((c.builds, c.sidecars, c.reads), (1, 1, 300), "{c:?}");
    let _ = std::fs::remove_dir_all(&d1);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn a_sidecar_under_another_partitions_name_is_refused() {
    // Two partitions, and the first's sidecar planted under the second's
    // name: the trailer names the segment it was made from.
    let dir = scratch("swapped");
    let mut db = Db::create(&dir, forced(|o| o.partition_bytes = Some(160 << 10))).unwrap();
    let model = ordered_load(&mut db, 6000, 0x51AB);
    db.zpages_settle().unwrap();
    assert!(db.zpage_counts().sidecars >= 2);
    db.close().unwrap();
    let files = sidecar_files(&dir);
    std::fs::copy(&files[0], &files[1]).unwrap();
    let db = Db::open(&dir, forced(|_| {})).unwrap();
    let c = db.zpage_counts();
    assert_eq!((c.rejected_identity, c.rejected_damage), (1, 0), "{c:?}");
    assert_eq!(c.sidecars, c.partitions - 1);
    let keys: Vec<Vec<u8>> = model.keys().step_by(7).cloned().collect();
    check(&db, &model, &keys, "swapped");
    let c = db.zpage_counts();
    assert!(c.rawserves > 0 && c.reads > 0, "{c:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- opening, building, declining ---------------------------------------------------

#[test]
fn a_reopened_store_attaches_the_sidecars_it_finds_and_off_leaves_them_alone() {
    let (dir, model, keys) = one_partition("reopen", 4000, 0x2E0);
    let file = sidecar_files(&dir).remove(0);
    let stamp = std::fs::metadata(&file).unwrap().modified().unwrap();
    // Under `Always`: attached at open, with nothing built, and read from.
    let db = Db::open(&dir, forced(|_| {})).unwrap();
    let c = db.zpage_counts();
    assert_eq!(
        (c.attached, c.builds, c.sidecars, c.rejected_damage),
        (1, 0, 1, 0),
        "{c:?}"
    );
    check(&db, &model, &keys[..500], "reopened");
    let c = db.zpage_counts();
    assert_eq!(
        (c.reads, c.counts, c.rawserves, c.fallbacks),
        (500, 500, 0, 0),
        "{c:?}"
    );
    db.close().unwrap();
    // Under `Off`: nothing attached, nothing read, the file untouched.
    let db = Db::open(&dir, Options::default()).unwrap();
    check(&db, &model, &keys[..500], "reopened under Off");
    let c = db.zpage_counts();
    assert_eq!(
        supdb::db::ZCounts { partitions: 0, ..c },
        Default::default(),
        "{c:?}"
    );
    db.close().unwrap();
    assert_eq!(sidecar_files(&dir), vec![file.clone()]);
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), stamp);
    // And a settle on a store that already holds its sidecars builds none.
    let mut db = Db::open(&dir, forced(|_| {})).unwrap();
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!((c.builds, c.attached, c.sidecars), (0, 1, 1), "{c:?}");
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), stamp);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_build_that_fails_is_counted_and_the_store_reads_raw() {
    let dir = scratch("failed");
    let mut db = Db::create(&dir, forced(|_| {})).unwrap();
    let model = ordered_load(&mut db, 3000, 0xFA11);
    db.fail_next_zpage_build();
    // Not an error for the store.
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!((c.build_failed, c.builds, c.sidecars), (1, 0, 0), "{c:?}");
    assert!(sidecar_files(&dir).is_empty(), "no file and no temp file");
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    check(&db, &model, &keys[..300], "failed");
    let c = db.zpage_counts();
    assert_eq!((c.reads, c.rawserves), (0, 600), "{c:?}");
    // The segment is not tried again by this process; a flush is not failed
    // by it; the next open gives the partition its sidecar.
    db.zpages_settle().unwrap();
    db.flush().unwrap();
    assert_eq!(db.zpage_counts().build_failed, 1);
    assert_eq!(db.zpage_counts().sidecars, 0);
    db.close().unwrap();
    let mut db = Db::open(&dir, forced(|_| {})).unwrap();
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!((c.builds, c.sidecars, c.build_failed), (1, 1, 0), "{c:?}");
    check(&db, &model, &keys[..300], "built");
    assert_eq!(db.zpage_counts().reads, 300);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_default_floor_declines_a_small_partition_and_says_how_much() {
    // Under `Always` with the engine's own floor, a partition of under two
    // mebibytes of records gets no file: counted, with its bytes, and read
    // raw.
    let dir = scratch("floor");
    let mut db = Db::create(
        &dir,
        Options {
            zpages: ZPages::Always,
            ..Options::default()
        },
    )
    .unwrap();
    let model = ordered_load(&mut db, 5000, 0xF100);
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(c.partitions >= 1 && c.sidecars == 0, "{c:?}");
    assert_eq!(c.declined_small, c.partitions, "{c:?}");
    // The gauge a pass asks whether the store is covered by: the live
    // partitions left raw for their size.
    assert_eq!(c.small_partitions, c.partitions, "{c:?}");
    assert!(
        c.declined_small_bytes > 400_000 && c.declined_small_bytes < 2 << 20,
        "{c:?}"
    );
    assert_eq!(c.builds + c.build_failed + c.attached, 0);
    assert!(sidecar_files(&dir).is_empty());
    let keys: Vec<Vec<u8>> = model.keys().step_by(5).cloned().collect();
    check(&db, &model, &keys, "small");
    let c = db.zpage_counts();
    assert_eq!((c.reads, c.counts), (0, 0));
    assert_eq!(c.rawserves, 2 * keys.len() as u64);
    // The engine's floor is the one the module names.
    assert_eq!(Options::default().zpages_min_bytes, 2 << 20);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_partition_with_a_run_in_a_block_is_declined_and_read_raw() {
    let dir = scratch("blocks");
    let mut db = Db::create(&dir, forced(|_| {})).unwrap();
    let mut model = ordered_load(&mut db, 2000, 0xB10C);
    // Above the engine's inline limit: this run goes to a block.
    let big = vec![b'z'; 2000];
    db.append(b"zzz-big", &big);
    model.insert(b"zzz-big".to_vec(), vec![big]);
    db.commit().unwrap();
    db.flush().unwrap();
    db.zpages_settle().unwrap();
    // The run is in the last partition; that one is declined and the
    // others, which hold no run in a block, are given sidecars.
    let c = db.zpage_counts();
    assert_eq!(c.declined_block_backed, 1, "{c:?}");
    assert_eq!(c.sidecars + 1, c.partitions, "{c:?}");
    // Left raw for what it holds, not for its size.
    assert_eq!(c.small_partitions, 0, "{c:?}");
    assert_eq!(sidecar_files(&dir).len() as u64, c.sidecars);
    let before = db.zpage_counts();
    check(&db, &model, &[b"zzz-big".to_vec()], "the big run");
    let after = db.zpage_counts();
    assert_eq!(
        after.rawserves - before.rawserves,
        2,
        "read raw, and counted"
    );
    assert_eq!(after.reads, before.reads);
    let keys: Vec<Vec<u8>> = model.keys().step_by(9).cloned().collect();
    check(&db, &model, &keys, "blocks");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_form_is_refused_beside_an_unpinned_writer_and_off_is_not() {
    let dir = scratch("refused");
    let unpinned = |z: ZPages| Options {
        zpages: z,
        writer_pins: false,
        publish_in_background: false,
        ..Options::default()
    };
    let e = match Db::create(&dir, unpinned(ZPages::Always)) {
        Ok(_) => panic!("Always beside an unpinned writer opened"),
        Err(e) => e,
    };
    assert!(e.to_string().contains("writer_pins"), "{e}");
    let dir = scratch("refused-off");
    assert!(Db::create(&dir, unpinned(ZPages::Off)).is_ok());
    // Pinned, with the segment work on a thread or inline, it opens.
    for background in [true, false] {
        let dir = scratch(&format!("refused-pinned-{background}"));
        let opts = Options {
            zpages: ZPages::Always,
            publish_in_background: background,
            ..Options::default()
        };
        assert!(Db::create(&dir, opts).is_ok());
    }
}

#[test]
fn the_default_is_off_and_an_off_store_makes_nothing() {
    assert_eq!(Options::default().zpages, ZPages::Off);
    assert_eq!(ZPages::default(), ZPages::Off);
    let dir = scratch("off");
    let mut db = Db::create(&dir, Options::default()).unwrap();
    let model = ordered_load(&mut db, 3000, 0x0FF);
    db.zpages_settle().unwrap();
    let keys: Vec<Vec<u8>> = model.keys().step_by(3).cloned().collect();
    check(&db, &model, &keys, "off");
    check_scan(&db, &model, "off");
    let c = db.zpage_counts();
    assert!(c.partitions >= 1);
    assert_eq!(
        supdb::db::ZCounts { partitions: 0, ..c },
        Default::default(),
        "{c:?}"
    );
    assert!(sidecar_files(&dir).is_empty());
    assert!(db.zpage_sidecars().is_empty());
}

// ---- what stays raw, and what is shared ---------------------------------------

#[test]
fn a_scan_of_a_covered_partition_is_walked_raw_and_counted() {
    let dir = scratch("scans");
    let mut db = Db::create(&dir, forced(|o| o.partition_bytes = Some(160 << 10))).unwrap();
    let model = ordered_load(&mut db, 6000, 0x5CA);
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(c.partitions >= 3 && c.sidecars == c.partitions, "{c:?}");
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    let before = db.zpage_counts();
    let mut scans = 0;
    for start in (0..keys.len()).step_by(500) {
        let mut got: Model = BTreeMap::new();
        let n = db
            .scan(&keys[start], 100, |k, v| {
                got.entry(k.to_vec()).or_default().push(v.to_vec())
            })
            .unwrap();
        let want: Model = model
            .range(keys[start].clone()..)
            .take(100)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!((n, &got), (100, &want), "scan from {start}");
        scans += 1;
    }
    check_scan(&db, &model, "all");
    scans += 1;
    let after = db.zpage_counts();
    assert_eq!(after.raw_scans - before.raw_scans, scans, "{after:?}");
    assert_eq!(after.reads, before.reads, "a scan reads no sidecar");
    assert_eq!(after.decodes, before.decodes);
    assert_eq!(after.verified_pages, 0, "and verifies none of its pages");
    // Under Off the scan is the same scan and nothing is counted.
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn handles_on_threads_read_through_the_sidecars_and_their_counts_sum() {
    let dir = scratch("threads");
    let mut db = Db::create(&dir, forced(|o| o.partition_bytes = Some(160 << 10))).unwrap();
    let model = std::sync::Arc::new(ordered_load(&mut db, 6000, 0x7D5));
    db.zpages_settle().unwrap();
    let keys: std::sync::Arc<Vec<Vec<u8>>> = std::sync::Arc::new(model.keys().cloned().collect());
    let per = 2000usize;
    let mut threads = Vec::new();
    for t in 0..3u64 {
        let r = db.reader().unwrap();
        let (model, keys) = (model.clone(), keys.clone());
        threads.push(std::thread::spawn(move || {
            let mut rng = Rng(0x1357 ^ (t + 1) * 0x9E37);
            for i in 0..per {
                let k = &keys[rng.below(keys.len())];
                if i % 2 == 0 {
                    assert_eq!(&read_vec(&r, k), &model[k]);
                } else {
                    assert_eq!(r.count(k).unwrap(), 1);
                }
            }
        }));
    }
    for t in threads {
        t.join().unwrap();
    }
    // The handles are gone; what they did is in the sum, on their slots'
    // lines, which nothing shared was written for.
    let c = db.zpage_counts();
    assert_eq!(c.reads, 3 * per as u64 / 2, "{c:?}");
    assert_eq!(c.counts, 3 * per as u64 / 2, "{c:?}");
    assert_eq!(c.rawserves + c.fallbacks, 0);
    assert!(c.verified_pages > 0 && c.verified_pages <= c.pages);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_callback_that_reads_again_leaves_the_value_it_was_lent_alone() {
    let dir = scratch("reentry");
    let mut db = Db::create(&dir, forced(|_| {})).unwrap();
    let model = ordered_load(&mut db, 5000, 0x4E4);
    db.zpages_settle().unwrap();
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    let (a, b) = (&keys[100], &keys[4000]);
    let mut calls = 0;
    db.read_all(a, |outer| {
        let before = outer.to_vec();
        assert_eq!(&before, &model[a][0]);
        // Another page, another read, inside the callback.
        assert_eq!(read_vec(&db, b), model[b]);
        assert_eq!(db.count(b).unwrap(), 1);
        assert_eq!(outer, &before[..], "the outer value was overwritten");
        calls += 1;
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(db.zpage_counts().reads, 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn covered_and_uncovered_partitions_agree_across_their_boundaries() {
    // The first partition built fails, the rest are given sidecars: reads
    // and scans cross from a partition read raw into one read from its
    // sidecar and answer as the model does.
    let dir = scratch("mixed");
    let mut db = Db::create(&dir, forced(|o| o.partition_bytes = Some(160 << 10))).unwrap();
    let model = ordered_load(&mut db, 6000, 0x30C);
    db.fail_next_zpage_build();
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(c.partitions >= 3, "{c:?}");
    assert_eq!((c.build_failed, c.sidecars), (1, c.partitions - 1), "{c:?}");
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    let mut around_edges: Vec<Vec<u8>> = Vec::new();
    for f in sidecar_files(&dir) {
        for fk in fence_keys(&f).into_iter().take(1) {
            around_edges.extend(around(&fk));
        }
    }
    around_edges.extend(keys.iter().step_by(11).cloned());
    check(&db, &model, &around_edges, "mixed");
    check_scan(&db, &model, "mixed");
    let c = db.zpage_counts();
    assert!(c.reads > 0 && c.rawserves > 0 && c.fallbacks == 0, "{c:?}");
    // Each probe was read and counted, and each of the two was served by
    // a sidecar or read raw.
    assert_eq!(
        c.reads + c.counts + c.rawserves,
        2 * around_edges.len() as u64,
        "{c:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a scan of `limit` keys from `start` answers, as the model has it,
/// and how many keys it visited. A scan visits every key an unsealed source
/// has touched whether or not a value is left to it, and its limit counts
/// the visit, so the model keeps a deleted key as a key with no values until
/// a flush drops it.
fn window(model: &Model, start: &[u8], limit: usize) -> Window {
    let mut out = Vec::new();
    let mut visited = 0;
    for (k, vs) in model.range(start.to_vec()..).take(limit) {
        visited += 1;
        out.extend(vs.iter().map(|v| (k.clone(), v.clone())));
    }
    (out, visited)
}

/// What `r` answers for the same scan, and the count it returns.
fn scanned(r: &Reader, start: &[u8], limit: usize) -> Window {
    let mut got = Vec::new();
    let n = r
        .scan(start, limit, |k, v| got.push((k.to_vec(), v.to_vec())))
        .unwrap();
    (got, n)
}

/// The entries of a scan and the count of keys it returned.
type Window = (Vec<(Vec<u8>, Vec<u8>)>, usize);

/// Two answers to a scan are the same, or the panic says where they part: a
/// list of byte arrays in an assertion's message is a failure nobody reads.
fn same_window(got: &Window, want: &Window, what: &str) {
    if got == want {
        return;
    }
    let at = got
        .0
        .iter()
        .zip(&want.0)
        .position(|(a, b)| a != b)
        .unwrap_or(got.0.len().min(want.0.len()));
    let show = |w: &Window| match w.0.get(at) {
        Some((k, v)) => format!("{:?} ({} bytes)", String::from_utf8_lossy(k), v.len()),
        None => "the end".to_string(),
    };
    panic!(
        "{what}: {} entries over {} keys, wanted {} over {}; they part at entry {at}, \
         got {}, wanted {}",
        got.0.len(),
        got.1,
        want.0.len(),
        want.1,
        show(got),
        show(want)
    );
}

#[test]
fn scans_agree_with_the_model_over_every_window_of_a_covered_store() {
    // The scans of a covered partition are walked raw, and have to answer as
    // if the form were not there, with the unsealed keys laid over it: updates,
    // inserts between records and deletes, in the live table and in a frozen
    // one. Windows begin at the fence keys, a byte either side of them, and
    // at the edges, and end after the counts a page and a block make
    // interesting.
    let dir = scratch("windows");
    let mut db = Db::create(&dir, forced(|o| o.partition_bytes = Some(160 << 10))).unwrap();
    let mut model = ordered_load(&mut db, 5000, 0x71D);
    db.zpages_settle().unwrap();
    assert!(db.zpage_counts().sidecars >= 3);
    // The seal is held short of its landing: a piece landed is merged back
    // into its partition by the segment work, which drops the tombstones
    // that the scans are held to counting, and the model would be racing
    // the merge.
    db.hold_seal_landing(true);
    let mut rng = Rng(0x71D1);
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    // Updates and deletes of records, and keys between records.
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
            // The first two rounds' writes are in a frozen table when the
            // third begins.
            db.seal().unwrap();
        }
    }
    assert_eq!(db.frozen_tables(), 1, "the seal is held, its table frozen");
    let mut starts: Vec<Vec<u8>> =
        vec![b"".to_vec(), b"a".to_vec(), b"zzz".to_vec(), vec![0xff; 8]];
    for f in sidecar_files(&dir) {
        for fk in fence_keys(&f).into_iter().step_by(7) {
            starts.extend(around(&fk));
        }
    }
    let limits = [1usize, 2, 3, 5, 17, 56, 57, 58, 114, usize::MAX];
    let latest = db.reader().unwrap();
    let before = db.zpage_counts();
    let mut windows = 0u64;
    for start in &starts {
        for &limit in &limits {
            let cap = limit.min(1500);
            let want = window(&model, start, cap);
            let what = format!("from {:?} for {cap}", String::from_utf8_lossy(start));
            same_window(&scanned(&db, start, cap), &want, &format!("writer, {what}"));
            same_window(
                &scanned(&latest, start, cap),
                &want,
                &format!("handle, {what}"),
            );
            windows += 1;
        }
    }
    // A snapshot keeps the answers it was taken at, whatever the writer
    // does after.
    let snap = db.reader().unwrap();
    snap.snapshot();
    let at_snapshot = model.clone();
    for k in keys.iter().step_by(5).take(200) {
        db.put(k, b"after the snapshot");
        model.insert(k.clone(), vec![b"after the snapshot".to_vec()]);
    }
    db.commit().unwrap();
    for start in starts.iter().step_by(3) {
        for &limit in &[3usize, 57, 114] {
            let what = format!("from {:?} for {limit}", String::from_utf8_lossy(start));
            same_window(
                &scanned(&snap, start, limit),
                &window(&at_snapshot, start, limit),
                &format!("snapshot, {what}"),
            );
            same_window(
                &scanned(&db, start, limit),
                &window(&model, start, limit),
                &format!("writer after, {what}"),
            );
        }
    }
    check(&db, &model, &keys, "after the windows");
    let after = db.zpage_counts();
    assert!(after.raw_scans - before.raw_scans >= windows, "{after:?}");
    assert_eq!(after.fallbacks + after.rejected_damage, 0, "{after:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sidecar a segment named `seg` has.
fn zpg_of(seg: &str) -> String {
    let rest = seg
        .strip_prefix("par-")
        .or_else(|| seg.strip_prefix("seg-"))
        .or_else(|| seg.strip_prefix("pcs-"))
        .unwrap();
    let mut f = rest.split('-');
    let id: u64 = f.next().unwrap().parse().unwrap();
    let end: u64 = f.next().unwrap().parse().unwrap();
    format!("zpg-{id:08}-{end:016}.zpg")
}

#[test]
fn a_merge_takes_the_sidecar_of_the_segment_it_retires() {
    // Writes spread over every range merge the partitions again; the
    // sidecars of the segments a merge retires go with them, and a
    // promotion, which keeps a segment's id and end sequence, keeps its
    // file. After a settle, the sidecars on disk are the live partitions'.
    let dir = scratch("retire");
    let mut db = Db::create(
        &dir,
        forced(|o| {
            o.partition_bytes = Some(32 << 10);
            o.seal_bytes = 16 << 10;
            o.l0_trigger = 2;
        }),
    )
    .unwrap();
    let mut model = ordered_load(&mut db, 4000, 0xAE7);
    db.zpages_settle().unwrap();
    let first = db.zpage_counts();
    assert!(
        first.sidecars >= 3 && first.sidecars == first.partitions,
        "{first:?}"
    );
    let mut rng = Rng(0xAE71);
    let keys: Vec<Vec<u8>> = model.keys().cloned().collect();
    for round in 0..40 {
        for _ in 0..150 {
            let k = keys[rng.below(keys.len())].clone();
            let v = format!("round-{round}-{}", rng.next() % 1000).into_bytes();
            db.append(&k, &v);
            model.get_mut(&k).unwrap().push(v);
        }
        db.commit().unwrap();
    }
    db.flush().unwrap();
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(
        c.builds > first.builds,
        "the merges' outputs were given sidecars: {c:?}"
    );
    assert_eq!(c.sidecars + c.declined_block_backed.min(0), c.sidecars);
    let live: Vec<String> = db
        .zpage_sidecars()
        .into_iter()
        .map(|(seg, ..)| zpg_of(&seg))
        .collect();
    let mut on_disk: Vec<String> = sidecar_files(&dir)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let mut want = live.clone();
    want.sort();
    on_disk.sort();
    assert_eq!(on_disk, want, "no sidecar outlives its segment: {c:?}");
    assert_eq!(live.len() as u64, c.sidecars);
    check(&db, &model, &keys, "after the merges");
    let _ = std::fs::remove_dir_all(&dir);
}
