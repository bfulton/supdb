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
