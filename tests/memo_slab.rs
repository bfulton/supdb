//! A handle's memo of decoded groups in a slab of its own
//! (`Options::zpages_memo_slab`): the same answers, the cells taken, and
//! the slab's mapping gone with the handle.
//!
//! Its own test binary, with one test, because what it reads is the
//! process's: the count of cells the memos have taken and the bytes the
//! engine has mapped, which every store in the process moves.

use supdb::{Db, Options, ZPages, ZStride};

mod common;
use common::{kv, vf};

const KEYS: u32 = 40_000;

fn key(k: u32) -> Vec<u8> {
    format!("key-{k:012}").into_bytes()
}

/// What one store's reads came to: every key's value length as a handle
/// read it twice over, the scan's count, the cells the memos took, what
/// the handle's memo mapped while it lived, the handle's memo hits, and
/// the groups its memo took, each at a point read's second touch of it.
struct Life {
    read: Vec<usize>,
    scanned: usize,
    cells: usize,
    mapped_by_handle: usize,
    hits: u64,
    puts: u64,
}

/// A store of forty thousand keys served by its block-coded form files,
/// read through a handle key by key twice and scanned once, the handle
/// dropped before the store closes.
fn life(name: &str, slab: bool) -> Life {
    let d = std::env::temp_dir().join(format!("supdb-next-memoslab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // The form named and not the tests' switch's: the memo is the block
    // form's, and a stride file decodes nothing into it.
    let opts = Options {
        zpages: ZPages::Always,
        zpages_min_bytes: 0,
        zpages_stride: ZStride::Off,
        zpages_memo_slab: slab,
        ..Options::default()
    };
    let cells_before = Db::memo_cells_taken();
    let mut db = Db::create(&d, opts).unwrap();
    for k in 0..KEYS {
        db.append(&key(k), &[b'v'; 100]);
        if k % 1000 == 999 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert!(
        c.form_files >= 1 && c.form_files == c.partitions && c.stride_files == 0,
        "{name}: every partition served by a block-coded form file: {c:?}"
    );
    let mapped_before = Db::mapped_bytes();
    let r = db.reader().unwrap();
    let mut read = Vec::with_capacity(2 * KEYS as usize);
    for pass in 0..2 {
        // A stride coprime to the count, so the reads land on the groups
        // in no order and each group is asked for again and again.
        for i in 0..KEYS {
            let k = (u64::from(i) * 7919 % u64::from(KEYS)) as u32;
            let mut len = usize::MAX;
            let n = r.read_all(&key(k), vf(&mut |v| len = v.len())).unwrap();
            assert_eq!(n, 1, "{name}: pass {pass} key {k}");
            read.push(len);
        }
    }
    let mut scanned = 0usize;
    r.scan(b"", usize::MAX, kv(&mut |_, v| scanned += v.len()))
        .unwrap();
    let (hits, puts) = {
        let c = r.zpage_counts();
        (c.memo_hits, c.point_admits)
    };
    let mapped_by_handle = Db::mapped_bytes() - mapped_before;
    let c = db.zpage_counts();
    assert!(
        c.reads >= 2 * u64::from(KEYS) && c.fallbacks == 0 && c.rawserves == 0,
        "{name}: every read through the form: {c:?}"
    );
    drop(r);
    assert_eq!(
        Db::mapped_bytes(),
        mapped_before,
        "{name}: what the handle's memo mapped went with the handle"
    );
    let cells = Db::memo_cells_taken() - cells_before;
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(&d);
    Life {
        read,
        scanned,
        cells,
        mapped_by_handle,
        hits,
        puts,
    }
}

/// With `Options::zpages_memo_slab` a handle's memo takes a cell of its
/// slab for every group it decodes -- five megabytes of groups here, a
/// cell each -- the slab is a mapping of the engine's for as long as the
/// handle lives and is unmapped at its drop, and every read and scan
/// answers what it answers with the groups in the allocator's buffers,
/// the memo hit as often. Without it no cell is taken and the memo maps
/// nothing.
#[test]
fn a_handles_memo_in_a_slab_reads_the_same_and_goes_with_the_handle() {
    let mapped_start = Db::mapped_bytes();
    let heap = life("heap", false);
    assert_eq!(heap.cells, 0, "off, no cell is taken");
    assert_eq!(heap.mapped_by_handle, 0, "off, the memo maps nothing");
    assert_eq!(Db::mapped_bytes(), mapped_start, "the heap store closed");
    let slab = life("slab", true);
    // Some two thousand groups, each taken by the memo at a read's second
    // touch and again after another group of its slot displaced it: a
    // cell for every slot that came to hold one, and no more cells than
    // the slab has, however many times the memo took a group, since a
    // displaced group's cell is the next one's.
    assert!(slab.puts > 2000, "groups the memo took: {}", slab.puts);
    assert!(
        (1000..=4096 + 16).contains(&slab.cells),
        "on, the memo's groups are in cells, used again: {} for {} puts",
        slab.cells,
        slab.puts
    );
    assert!(
        slab.mapped_by_handle >= 8 << 20 && slab.mapped_by_handle <= 12 << 20,
        "on, the handle's slab is one mapping of a cell a slot: {}",
        slab.mapped_by_handle
    );
    assert_eq!(Db::mapped_bytes(), mapped_start, "the slab store closed");
    assert!(
        heap.read.iter().all(|&n| n == 100),
        "every key reads its value"
    );
    assert_eq!(heap.read, slab.read, "the same reads");
    assert_eq!(heap.scanned, 100 * KEYS as usize);
    assert_eq!(heap.scanned, slab.scanned, "the same scan");
    // The slots are cut from a form file's identity, which is each
    // store's own, so the two memos' collisions are not the same ones:
    // each is held to hitting on the second pass, not to the other's
    // count.
    assert!(heap.hits > u64::from(KEYS), "the second pass hits the memo");
    assert!(slab.hits > u64::from(KEYS), "and with the slab");
    assert!(heap.puts > 2000);
}
