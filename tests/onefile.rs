//! A store kept in one file (`Options::one_file`): its segments, their
//! ordered indexes and their form files extents of one data file, the
//! manifest naming each one's place.
//!
//! What it is held to: it reads as the store of files it would have been,
//! across a close, a kill and a crash at a landing; nothing of a segment
//! is in the directory under a name of its own; the room a merge retires
//! is written over and the file stops growing; a reader that holds a
//! retired segment reads it until it lets go, whatever was written since;
//! a slot that names room since written over is refused, as a slot that
//! names a missing file is; and a checkpoint of it is a store of files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use supdb::{Db, Options};

mod common;
use common::{kv, vf};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-onefile-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A store in one file that seals and merges at a few tens of kilobytes,
/// the segment work on a thread of its own or inline.
fn small(background: bool) -> Options {
    Options {
        one_file: true,
        seal_bytes: 32 << 10,
        partition_bytes: Some(32 << 10),
        l0_trigger: 2,
        publish_in_background: background,
        ..common::options()
    }
}

fn key(i: u32) -> Vec<u8> {
    format!("key-{i:06}").into_bytes()
}

/// `n` keys in an order that is no run, each with `tag` and its number.
fn load(db: &mut Db, model: &mut BTreeMap<Vec<u8>, Vec<u8>>, n: u32, tag: &str) {
    for j in 0..n {
        let i = (j.wrapping_mul(7919)) % n;
        let v = format!("{tag}-{i}").into_bytes();
        db.put(&key(i), &v);
        model.insert(key(i), v);
        if j % 200 == 199 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
}

fn dump(db: &Db) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut out = BTreeMap::new();
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            out.insert(k.to_vec(), v.to_vec());
        }),
    )
    .unwrap();
    out
}

fn value(db: &Db, k: &[u8]) -> Option<Vec<u8>> {
    let mut out = None;
    db.read_all(k, vf(&mut |v| out = Some(v.to_vec()))).unwrap();
    out
}

/// The names in `d` that are a segment's, an ordered index's or a form
/// file's under a name of their own.
fn own_files(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sup") || n.ends_with(".oidx") || n.ends_with(".zpg"))
        .collect();
    v.sort();
    v
}

/// Every name in `d` with its length, sorted: what a refused open has to
/// leave as it found it.
fn listing(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| {
            format!(
                "{} {}",
                e.file_name().to_string_lossy(),
                e.metadata().map(|m| m.len()).unwrap_or(0)
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_store_in_one_file_reads_as_itself_and_keeps_no_segment_under_a_name() {
    for background in [true, false] {
        let d = dir(if background {
            "reads-bg"
        } else {
            "reads-inline"
        });
        let mut model = BTreeMap::new();
        let mut db = Db::create(&d, small(background)).unwrap();
        load(&mut db, &mut model, 4000, "a");
        db.flush().unwrap();
        db.settle().unwrap();
        assert!(db.levels().0 > 1, "partitions, so merges ran");
        assert_eq!(dump(&db), model);
        let c = db.one_counts().expect("a store in one file");
        assert!(c.files > 0 && c.held > 0, "{c:?}");
        assert_eq!(own_files(&d), Vec::<String>::new(), "{c:?}");
        assert!(d.join("store.one").is_file());
        // Closed and opened with nothing asked: the store says how it is
        // kept, not the options.
        db.close().unwrap();
        let db = Db::open(&d, Options::default()).unwrap();
        let c = db.one_counts().expect("a store in one file");
        // The table is of the slot the open took: what that slot names
        // waits for a later one before its room is another file's.
        assert!(c.durable > 0 && c.listed == c.files, "{c:?}");
        assert_eq!(dump(&db), model);
        for i in [0u32, 1, 1999, 3999] {
            assert_eq!(value(&db, &key(i)), model.get(&key(i)).cloned());
        }
        assert_eq!(
            db.ord_rebuilt(),
            0,
            "every ordered index was in the manifest"
        );
    }
}

#[test]
fn room_a_merge_retires_is_written_over_and_the_file_stops_growing() {
    let d = dir("reuse");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    let mut ends = Vec::new();
    for round in 0..10 {
        load(&mut db, &mut model, 3000, &format!("r{round}"));
        db.flush().unwrap();
        db.settle().unwrap();
        ends.push(db.one_counts().unwrap().end);
    }
    let c = db.one_counts().unwrap();
    assert!(c.reused > 0, "no room was used twice: {c:?}");
    assert_eq!(
        ends[4], ends[9],
        "the file grew through rounds of the same store: {ends:?}"
    );
    assert_eq!(dump(&db), model);
    db.close().unwrap();
    assert_eq!(dump(&Db::open(&d, small(true)).unwrap()), model);
}

#[test]
fn a_reader_holding_a_retired_segment_reads_it_whatever_was_written_since() {
    let d = dir("pinned");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 3000, "old");
    db.flush().unwrap();
    db.settle().unwrap();
    let old = model.clone();
    let held = db.reader().unwrap();
    held.snapshot();
    // Every segment the snapshot reads is retired, several times over,
    // and its room is the first the store would take.
    for round in 0..6 {
        load(&mut db, &mut model, 3000, &format!("new{round}"));
        db.flush().unwrap();
        db.settle().unwrap();
    }
    let mut seen = BTreeMap::new();
    held.scan(
        b"",
        usize::MAX,
        kv(&mut |k, v| {
            seen.insert(k.to_vec(), v.to_vec());
        }),
    )
    .unwrap();
    assert_eq!(seen, old, "the snapshot read room written over");
    let waiting = db.one_counts().unwrap();
    drop(held);
    load(&mut db, &mut model, 100, "last");
    db.flush().unwrap();
    db.settle().unwrap();
    assert!(
        db.one_counts().unwrap().free > waiting.free,
        "the snapshot's room never came back"
    );
    assert_eq!(dump(&db), model);
}

#[test]
fn a_store_killed_at_a_landing_reads_every_commit() {
    for background in [true, false] {
        let d = dir(if background { "kill-bg" } else { "kill-inline" });
        let mut model = BTreeMap::new();
        let mut db = Db::create(&d, small(background)).unwrap();
        load(&mut db, &mut model, 3000, "a");
        // Seals and merges in flight, nothing settled, and the store
        // gone as a process's death takes it.
        load(&mut db, &mut model, 3000, "b");
        db.crash();
        let mut db = Db::open(&d, small(background)).unwrap();
        assert_eq!(dump(&db), model);
        // And it goes on: what the dead store's threads had in flight is
        // free room, and nothing of theirs is written into it.
        load(&mut db, &mut model, 3000, "c");
        db.flush().unwrap();
        db.settle().unwrap();
        assert_eq!(dump(&db), model);
        db.close().unwrap();
        assert_eq!(dump(&Db::open(&d, small(background)).unwrap()), model);
    }
}

#[test]
fn a_slot_that_names_room_since_written_over_is_refused() {
    let d = dir("fallback");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 3000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    // Both slots as they stand, then rounds that retire what they name
    // and write over it.
    let slots: Vec<Vec<u8>> = ["manifest", "manifest.alt"]
        .iter()
        .map(|s| std::fs::read(d.join(s)).unwrap())
        .collect();
    for round in 0..8 {
        load(&mut db, &mut model, 3000, &format!("r{round}"));
        db.flush().unwrap();
        db.settle().unwrap();
    }
    assert!(db.one_counts().unwrap().reused > 0);
    db.close().unwrap();
    // The slots of then, over the data file of now: damage of the kind
    // a newest slot lost leaves, the open falling back to one that names
    // what the store has since given away.
    for (s, bytes) in ["manifest", "manifest.alt"].iter().zip(&slots) {
        std::fs::write(d.join(s), bytes).unwrap();
    }
    let before: Vec<String> = listing(&d);
    let e = Db::open(&d, small(true))
        .err()
        .expect("a slot of another time");
    assert!(
        e.to_string().contains("refused") && e.to_string().contains("missing"),
        "{e}"
    );
    assert_eq!(
        listing(&d),
        before,
        "a refused open leaves the directory as it was"
    );
}

#[test]
fn a_checkpoint_of_a_store_in_one_file_is_a_store_of_files() {
    let root = dir("checkpoint");
    let (d, cp) = (root.join("store"), root.join("copy"));
    std::fs::create_dir_all(&d).unwrap();
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 3000, "a");
    db.flush().unwrap();
    db.checkpoint(&cp).unwrap();
    let at = model.clone();
    assert!(!cp.join("store.one").exists());
    assert!(
        !own_files(&cp).is_empty(),
        "the copy holds its segments as files"
    );
    // The origin rewritten and gone; the copy reads what it was made of.
    for round in 0..4 {
        load(&mut db, &mut model, 3000, &format!("r{round}"));
        db.flush().unwrap();
        db.settle().unwrap();
    }
    db.close().unwrap();
    std::fs::remove_dir_all(&d).unwrap();
    let copy = Db::open(&cp, Options::default()).unwrap();
    assert!(copy.one_counts().is_none());
    assert_eq!(dump(&copy), at);
}

#[test]
fn the_option_is_refused_beside_the_manifest_shapes_it_cannot_keep() {
    for (name, o) in [
        (
            "pending",
            Options {
                manifest_pending: true,
                ..small(true)
            },
        ),
        (
            "rename",
            Options {
                manifest_slots: false,
                ..small(true)
            },
        ),
    ] {
        let d = dir(&format!("refuse-{name}"));
        let e = Db::create(&d, o).err().expect("refused");
        assert!(e.to_string().contains("one_file"), "{name}: {e}");
        assert!(!d.join("store.one").exists(), "{name}: nothing was made");
    }
}

/// `from`'s files copied to `to`, as a process's death leaves them.
fn copy_files(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.file_type().unwrap().is_file() {
            match std::fs::copy(e.path(), to.join(e.file_name())) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => panic!("copying {:?}: {err}", e.path()),
            }
        }
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let t = std::time::Instant::now();
    while !done() {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(60),
            "waited a minute for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn a_seal_cut_between_its_landing_and_its_sync_leaves_room_nobody_names() {
    let root = dir("unsynced");
    let (d, whole, cut) = (root.join("store"), root.join("whole"), root.join("cut"));
    std::fs::create_dir_all(&d).unwrap();
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 2000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    let before = db.one_counts().unwrap();
    let len_before = std::fs::metadata(d.join("store.one")).unwrap().len();
    // A seal's segments in place and read from, their sync not begun: the
    // manifest on the device names nothing of them, and the log holds
    // their writes.
    db.hold_seal_durable(true);
    let segs = db.segments();
    load(&mut db, &mut model, 2000, "b");
    wait_for("the seal's readable landing", || db.segments() > segs);
    assert_eq!(dump(&db), model);
    let paused = db.pause_segment_work();
    copy_files(&d, &whole);
    drop(paused);
    db.hold_seal_durable(false);
    db.flush().unwrap();
    db.settle().unwrap();
    assert_eq!(dump(&db), model);
    db.close().unwrap();
    // The copy as the page cache held it: the seal's bytes are there and
    // nothing names them.
    let w = Db::open(&whole, small(true)).unwrap();
    assert_eq!(dump(&w), model);
    let c = w.one_counts().unwrap();
    assert!(
        c.held <= before.held,
        "the open took an unnamed seal's room: {c:?} {before:?}"
    );
    drop(w);
    // And as a device that never took the file's growth holds it: the
    // data file its length before the seal.
    copy_files(&whole, &cut);
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(cut.join("store.one"))
        .unwrap();
    assert!(f.metadata().unwrap().len() >= len_before);
    f.set_len(len_before).unwrap();
    drop(f);
    let c = Db::open(&cut, small(true)).unwrap();
    assert_eq!(dump(&c), model);
}

#[test]
fn a_data_file_cut_under_a_place_its_manifest_names_is_refused() {
    let d = dir("cut-named");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 3000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    db.close().unwrap();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(d.join("store.one"))
        .unwrap();
    // The head and nothing behind it: every place the manifest names is
    // past the file's end.
    f.set_len(1 << 20).unwrap();
    drop(f);
    let before = listing(&d);
    let e = Db::open(&d, small(true))
        .err()
        .expect("a data file cut short");
    let e = e.to_string();
    assert!(
        e.contains("refused")
            && e.contains("outside the data file")
            && e.contains("nothing was removed"),
        "{e}"
    );
    assert_eq!(listing(&d), before);
}

#[test]
fn a_data_file_beside_no_slot_is_refused_and_a_creations_alone_is_swept() {
    let d = dir("no-slot");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 3000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    db.close().unwrap();
    for s in ["manifest", "manifest.alt"] {
        std::fs::write(d.join(s), b"").unwrap();
    }
    let before = listing(&d);
    let e = Db::open(&d, small(true))
        .err()
        .expect("no slot beside a data file");
    let e = e.to_string();
    assert!(
        e.contains("refused") && e.contains("store.one") && e.contains("nothing was removed"),
        "{e}"
    );
    assert_eq!(listing(&d), before);
    // A store whose creation wrote its data file and no manifest held
    // nothing: it opens as a store of files, and the data file goes.
    let d = dir("no-slot-new");
    Db::create(&d, small(true)).unwrap().crash();
    for s in ["manifest", "manifest.alt"] {
        std::fs::write(d.join(s), b"").unwrap();
    }
    let mut db = Db::open(&d, small(true)).unwrap();
    assert!(db.one_counts().is_none());
    assert!(!d.join("store.one").exists());
    let mut model = BTreeMap::new();
    load(&mut db, &mut model, 500, "a");
    db.flush().unwrap();
    assert_eq!(dump(&db), model);
    assert!(!own_files(&d).is_empty());
}

#[test]
fn a_store_of_files_stays_one_whatever_the_option_says() {
    let d = dir("files");
    let mut model = BTreeMap::new();
    let mut db = Db::create(
        &d,
        Options {
            one_file: false,
            ..small(true)
        },
    )
    .unwrap();
    load(&mut db, &mut model, 2000, "a");
    db.flush().unwrap();
    db.close().unwrap();
    let mut db = Db::open(&d, small(true)).unwrap();
    assert!(db.one_counts().is_none());
    load(&mut db, &mut model, 2000, "b");
    db.flush().unwrap();
    db.settle().unwrap();
    assert_eq!(dump(&db), model);
    assert!(!d.join("store.one").exists());
    assert!(!own_files(&d).is_empty());
}

#[test]
fn a_store_in_one_file_is_refused_under_the_manifest_shapes_it_cannot_keep() {
    let d = dir("refuse-open");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 1000, "a");
    db.flush().unwrap();
    db.close().unwrap();
    let before = listing(&d);
    for (name, o) in [
        (
            "pending",
            Options {
                one_file: false,
                manifest_pending: true,
                ..small(true)
            },
        ),
        (
            "rename",
            Options {
                one_file: false,
                manifest_slots: false,
                ..small(true)
            },
        ),
    ] {
        let e = Db::open(&d, o).err().expect("refused");
        let e = e.to_string();
        assert!(
            e.contains("refused") && e.contains("one file") && e.contains("nothing was removed"),
            "{name}: {e}"
        );
        assert_eq!(listing(&d), before, "{name}");
    }
    assert_eq!(dump(&Db::open(&d, Options::default()).unwrap()), model);
}

#[test]
fn form_files_are_kept_in_the_data_file_and_found_by_the_open_after_a_kill() {
    let d = dir("forms");
    let mut model = BTreeMap::new();
    let forced = || Options {
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        ..small(true)
    };
    let mut db = Db::create(&d, forced()).unwrap();
    load(&mut db, &mut model, 4000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    db.zpages_settle().unwrap();
    let built = db.zpage_counts();
    assert!(built.builds > 0, "{built:?}");
    assert_eq!(own_files(&d), Vec::<String>::new());
    assert_eq!(dump(&db), model);
    // A form file built since the last landing is listed by a manifest of
    // its own once a sync has covered it, which the segment work makes for
    // one that rode where the store has been quiet a second: from there
    // the store gone as a kill takes it opens with every form file it had.
    wait_for("every file of the table listed", || {
        let c = db.one_counts().unwrap();
        c.listed == c.files
    });
    db.crash();
    let mut db = Db::open(&d, forced()).unwrap();
    db.zpages_settle().unwrap();
    let again = db.zpage_counts();
    assert_eq!(again.builds, 0, "a form file was built again: {again:?}");
    assert!(again.attached > 0, "{again:?}");
    assert_eq!(dump(&db), model);
    for i in [0u32, 17, 3999] {
        assert_eq!(value(&db, &key(i)), model.get(&key(i)).cloned());
    }
}

/// An open that publishes tells the table its manifest is on the device,
/// as the segment work's publishes do (`SlotChoice::write_shape`). Here
/// the open names the piece a killed writer's run became, so the slot it
/// took is replaced by one the device holds, and the newest sequence the
/// table knows durable is the open's own.
#[test]
fn an_opens_publish_tells_the_table_its_manifest_is_on_the_device() {
    for background in [true, false] {
        let d = dir(if background {
            "open-publish-bg"
        } else {
            "open-publish-inline"
        });
        let mut model = BTreeMap::new();
        let mut db = Db::create(&d, small(background)).unwrap();
        load(&mut db, &mut model, 2000, "a");
        db.flush().unwrap();
        db.settle().unwrap();
        db.close().unwrap();
        // Opened at rest, the table is of the slot the open took and the
        // open publishes nothing.
        let mut db = Db::open(&d, small(background)).unwrap();
        let taken = db.one_counts().unwrap().durable;
        // A run of keys above the store's greatest, committed and killed:
        // a file of the directory's own under its temp name, which the
        // next open finishes as a piece and names in a manifest.
        for i in 0..300u32 {
            let k = format!("run-{i:06}").into_bytes();
            let v = format!("r-{i}").into_bytes();
            db.append(&k, &v);
            model.insert(k, v);
        }
        db.commit().unwrap();
        let at_kill = db.one_counts().unwrap().durable;
        assert_eq!(at_kill, taken, "the premise: nothing published since");
        db.crash();
        let db = Db::open(&d, small(background)).unwrap();
        assert_eq!(dump(&db), model);
        let c = db.one_counts().unwrap();
        assert!(
            c.durable > at_kill,
            "the open's own manifest is on the device: {c:?}, the slot taken {at_kill}"
        );
    }
}

/// An ordered index is in the manifest that names its segment. A seal's
/// index rides the sync of its piece (`vault::File::ride`) and a merge's
/// is written ahead of its piece's sync, so the manifest a landing writes
/// finds both durable; a store killed straight after its landings, with
/// no quiet turn of the segment work to list what was left over, opens
/// with no index to compose again.
#[test]
fn an_index_is_named_by_the_manifest_that_names_its_segment() {
    let d = dir("index-with-piece");
    let mut model = BTreeMap::new();
    // The segment work inside the writer's own operations: every landing
    // a commit drove has written its manifest when the commit returns,
    // and nothing runs between two of the writer's calls.
    let mut db = Db::create(&d, small(false)).unwrap();
    load(&mut db, &mut model, 4000, "a");
    db.flush().unwrap();
    assert!(db.levels().0 > 1, "partitions, so merges landed");
    let c = db.one_counts().unwrap();
    db.crash();
    let mut db = Db::open(&d, small(false)).unwrap();
    assert_eq!(dump(&db), model);
    assert_eq!(
        db.ord_rebuilt(),
        0,
        "an index the manifest that names its segment did not name: {c:?}"
    );
    // And a seal's: a piece left standing, under the trigger that would
    // merge it, its index written beside it and riding its sync.
    for i in (0..4000u32).step_by(53) {
        let v = format!("piece-{i}").into_bytes();
        db.put(&key(i), &v);
        model.insert(key(i), v);
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    assert!(db.levels().1 >= 1, "a piece stands: {:?}", db.levels());
    let c = db.one_counts().unwrap();
    db.crash();
    let db = Db::open(&d, small(false)).unwrap();
    assert_eq!(dump(&db), model);
    assert_eq!(
        db.ord_rebuilt(),
        0,
        "a piece's index its seal's manifest did not name: {c:?}"
    );
}

/// A close lists what was built since the last landing. A form file is
/// built behind the landing of its segment and rides no sync of that
/// segment's, so it is in no manifest until the segment work looks for
/// what is left over (`Maint::one_look`): on its thread's turn where it
/// has one, and at its stop, which with the segment work inside the
/// writer's operations is the only look there is. A store closed
/// straight after its builds opens with every form file attached and
/// none to build.
#[test]
fn a_close_lists_what_was_built_since_the_last_landing() {
    let d = dir("close-lists");
    let opts = || Options {
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        scan_block_cache: false,
        ..small(false)
    };
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, opts()).unwrap();
    load(&mut db, &mut model, 4000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    db.zpages_settle().unwrap();
    let built = db.zpage_counts();
    assert!(
        built.form_files > 1 && built.form_files == built.partitions,
        "{built:?}"
    );
    db.close().unwrap();
    let mut db = Db::open(&d, opts()).unwrap();
    db.zpages_settle().unwrap();
    let c = db.zpage_counts();
    assert_eq!(
        (c.builds, c.form_files),
        (0, built.form_files),
        "a form file the close left out of its manifest was built again: {c:?}"
    );
    assert_eq!(db.ord_rebuilt(), 0);
    assert_eq!(dump(&db), model);
}

/// A place whose first page is not the file the slot listed there is
/// refused by the slot's own row (`One::on_device`), in the words a
/// missing file is refused in, before a segment is opened: what an open
/// that falls back past a damaged slot finds where the room has been
/// another file's since. Here the page is another segment's, written
/// over a named one's with the store closed.
#[test]
fn a_place_holding_another_files_first_page_is_refused_by_its_row() {
    let d = dir("head-check");
    let mut model = BTreeMap::new();
    let mut db = Db::create(&d, small(true)).unwrap();
    load(&mut db, &mut model, 3000, "a");
    db.flush().unwrap();
    db.settle().unwrap();
    let segs: Vec<String> = common::seg_names(&d)
        .into_iter()
        .filter(|n| Db::one_place(&d, n).is_some())
        .collect();
    assert!(segs.len() >= 2, "two segments in the data file: {segs:?}");
    let (a, _) = Db::one_place(&d, &segs[0]).unwrap();
    let (b, _) = Db::one_place(&d, &segs[1]).unwrap();
    db.close().unwrap();
    {
        use std::os::unix::fs::FileExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(d.join("store.one"))
            .unwrap();
        let mut page = [0u8; 4096];
        f.read_exact_at(&mut page, b).unwrap();
        f.write_all_at(&page, a).unwrap();
        f.sync_all().unwrap();
    }
    let before = listing(&d);
    let e = Db::open(&d, small(true))
        .err()
        .expect("a place written over");
    let e = e.to_string();
    assert!(
        e.contains("refused: the manifest slot")
            && e.contains(&format!("names {}", segs[0]))
            && e.contains("nothing was removed"),
        "{e}"
    );
    assert_eq!(
        listing(&d),
        before,
        "a refused open leaves the directory as it was"
    );
}
