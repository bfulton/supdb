//! A store under a cipher the caller supplies (`Encryption::Cipher`,
//! `supdb::db::vault`), under a real one.
//!
//! `src/db/vault.rs` holds the layer's own pieces to its unit tests, under a
//! stand-in cipher: the memory files, the envelopes, the sealed manifest and
//! the sealed log. What is held here is the store: that it reads as a plain
//! store reads, that nothing it was given is in its directory in the clear,
//! names included, that it refuses what is not it, that a byte changed
//! anywhere is refused or is the log's end, and that what a crash leaves
//! opens.
//!
//! Linux only, as the store under a cipher is: elsewhere the binary holds
//! no test.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use supdb::{Db, Encryption, Options, StoreCipher};

mod common;
use common::{kv, vf, Gcm};

fn cipher(n: u8) -> StoreCipher {
    StoreCipher::new(Gcm::new(&[n; 32]), Gcm::new).key_id(u64::from(n))
}

fn under(n: u8) -> Options {
    Options {
        encryption: Encryption::Cipher(cipher(n)),
        ..small()
    }
}

/// Seals and partitions of a quarter megabyte, so that a test's few
/// megabytes leave a log, pieces, several merged partitions -- whose names
/// carry their fences -- and their indexes behind.
fn small() -> Options {
    Options {
        seal_bytes: 256 << 10,
        partition_bytes: Some(256 << 10),
        l0_trigger: 2,
        ..Options::default()
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-cipher-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

const MARK: &str = "IN-THE-CLEAR";

/// What every key begins with: short, so that the keys differ inside the
/// sixteen bytes a partition's fence is cut to and a plain store's
/// partitions are named by them.
const KEY_MARK: &str = "k-CLR-";

fn k(i: u32) -> Vec<u8> {
    format!("{KEY_MARK}{i:08}").into_bytes()
}

fn v(i: u32, round: u32) -> Vec<u8> {
    format!(
        "value {i} of round {round}, {MARK}, {}",
        "x".repeat((i % 50) as usize)
    )
    .into_bytes()
}

/// A few megabytes through every way the store writes: keys in order, the
/// same keys again out of order, and enough of both for seals and the
/// merges behind them.
fn fill(db: &mut Db, n: u32) {
    for i in 0..n {
        db.append(&k(i), &v(i, 0));
        if i % 500 == 499 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    for i in (0..n).rev().step_by(3) {
        db.put(&k(i), &v(i, 1));
        if i % 500 < 3 {
            db.commit().unwrap();
        }
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
}

/// What `fill` left, read back: every key's value, and a scan of them all.
fn holds(db: &Db, n: u32) {
    for i in (0..n).step_by(7) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec())))
            .unwrap();
        let round = u32::from((n - 1 - i).is_multiple_of(3));
        assert_eq!(got, [v(i, round)], "key {i}");
    }
    let mut seen = 0u32;
    db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
        .unwrap();
    assert_eq!(seen, n);
}

/// Every file in `dir` that holds `needle`, by name.
fn files_holding(dir: &Path, needle: &[u8]) -> Vec<String> {
    names(dir)
        .into_iter()
        .filter(|n| {
            let bytes = std::fs::read(dir.join(n)).unwrap();
            bytes.windows(needle.len()).any(|w| w == needle)
        })
        .collect()
}

/// A store under a cipher reads as itself, before and after it is
/// reopened, and nothing it was given is in its directory: not a key, not a
/// value, and not in a file's name, where a plain store's partitions carry
/// their fences. The search has a control, the same store in the clear,
/// where it has to find all three.
#[test]
fn a_store_under_a_cipher_reads_as_itself_and_is_ciphertext_in_its_directory() {
    let n = 30_000;
    let dir = scratch("reads");
    {
        let mut db = Db::create(&dir, under(1)).unwrap();
        fill(&mut db, n);
        holds(&db, n);
        let c = db.cipher_counts().unwrap();
        assert!(c.persisted > 2, "segments reached the directory: {c:?}");
        assert_eq!(c.loaded, 0, "and none was read back from it");
    }
    for needle in [MARK, KEY_MARK] {
        assert!(
            files_holding(&dir, needle.as_bytes()).is_empty(),
            "{needle} in {:?}",
            files_holding(&dir, needle.as_bytes())
        );
    }
    let fence: String = KEY_MARK.bytes().map(|b| format!("{b:02x}")).collect();
    for name in names(&dir) {
        let known = name == "supdb.cipher"
            || name == "manifest"
            || name == "manifest.alt"
            || (name.starts_with("wal-") && name[4..].bytes().all(|b| b.is_ascii_digit()))
            || (name.starts_with("x-")
                && name[2..].trim_end_matches(".o").len() == 32
                && name[2..]
                    .trim_end_matches(".o")
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit()));
        assert!(known, "a file named {name}");
        assert!(!name.contains(&fence), "{name} carries a fence");
    }
    {
        let db = Db::open(&dir, under(1)).unwrap();
        holds(&db, n);
        let c = db.cipher_counts().unwrap();
        assert!(c.loaded > 0, "the open decrypted what it read: {c:?}");
        assert_eq!(c.swept, 0, "a clean close leaves nothing to sweep");
    }
    // The control: in the clear, the same search finds the keys and the
    // values in the files and a fence in a name.
    let plain = scratch("reads-plain");
    {
        let mut db = Db::create(&plain, small()).unwrap();
        fill(&mut db, n);
    }
    for needle in [MARK, KEY_MARK] {
        assert!(!files_holding(&plain, needle.as_bytes()).is_empty());
    }
    assert!(
        names(&plain).iter().any(|name| name.contains(&fence)),
        "no fence {fence} in {:?}",
        names(&plain)
    );
}

/// A directory's files, each by name with its bytes.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    names(dir)
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(dir.join(&n)).unwrap();
            (n, bytes)
        })
        .collect()
}

/// `dir` made to hold exactly `files`.
fn restore(dir: &Path, files: &[(String, Vec<u8>)]) {
    for n in names(dir) {
        std::fs::remove_file(dir.join(n)).unwrap();
    }
    for (n, bytes) in files {
        std::fs::write(dir.join(n), bytes).unwrap();
    }
}

/// A store under a cipher opens under that cipher and no other way, and a
/// cipher opens no store that is not under it; each refusal leaves the
/// directory as it found it, byte for byte, and the store opens after
/// them all.
#[test]
fn a_store_under_a_cipher_opens_under_no_other() {
    let n = 4_000;
    let dir = scratch("refusals");
    {
        let mut db = Db::create(&dir, under(1)).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&dir);
    let refused = |r: std::io::Result<Db>, what: &str| {
        let e = r.err().unwrap_or_else(|| panic!("{what} was not refused"));
        assert_eq!(snapshot(&dir), before, "{what} changed the directory");
        e.to_string()
    };
    let e = refused(Db::open(&dir, small()), "an open with no cipher");
    assert!(e.contains("cipher"), "{e}");
    let e = refused(Db::open(&dir, under(2)), "an open under another key");
    assert!(e.contains("key is wrong"), "{e}");
    refused(Db::create(&dir, small()), "a plain store made over it");
    refused(Db::create(&dir, under(1)), "a second store made over it");
    assert_eq!(supdb::cipher_key_id(&dir).unwrap(), Some(1));
    holds(&Db::open(&dir, under(1)).unwrap(), n);

    // And the other way: a cipher given to a store in the clear.
    let plain = scratch("refusals-plain");
    {
        let mut db = Db::create(&plain, small()).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&plain);
    let e = Db::open(&plain, under(1))
        .err()
        .expect("a cipher on a plain store")
        .to_string();
    assert!(e.contains("in the clear"), "{e}");
    assert!(Db::create(&plain, under(1)).is_err());
    assert_eq!(snapshot(&plain), before);
    assert_eq!(supdb::cipher_key_id(&plain).unwrap(), None);
    holds(&Db::open(&plain, small()).unwrap(), n);
}

fn mix(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn digest_pair(h: u64, key: &[u8], value: &[u8]) -> u64 {
    let h = mix(mix(h, &(key.len() as u32).to_le_bytes()), key);
    mix(mix(h, &(value.len() as u32).to_le_bytes()), value)
}

/// Everything a store holds, as one number.
fn digest_of(db: &Db) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    db.scan(
        b"",
        usize::MAX,
        kv(&mut |key, value| h = digest_pair(h, key, value)),
    )
    .unwrap();
    h
}

fn digest_of_model(model: &BTreeMap<Vec<u8>, Vec<u8>>) -> u64 {
    model
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, (key, value)| {
            digest_pair(h, key, value)
        })
}

/// Seals and partitions of sixteen kilobytes, the segment work on the
/// writer's thread: a store of a hundred kilobytes with several segments
/// and a log, small enough to open hundreds of times.
fn tiny(n: u8) -> Options {
    Options {
        encryption: Encryption::Cipher(cipher(n)),
        seal_bytes: 16 << 10,
        partition_bytes: Some(16 << 10),
        l0_trigger: 2,
        ..Options::default()
    }
}

/// A store with a history, left as a crash leaves it: sixty commits of
/// puts over seven hundred keys, flushed into segments, then forty small
/// commits that stay under a seal and so are the log's alone when the
/// store is dropped. What the store held after each commit, as a digest:
/// every state it ever acknowledged.
fn with_a_history(dir: &Path, n: u8) -> Vec<u64> {
    let mut db = Db::create(dir, tiny(n)).unwrap();
    let mut model = BTreeMap::new();
    let mut history = vec![digest_of_model(&model)];
    let mut commit = |db: &mut Db, c: u32, puts: u32| {
        for j in 0..puts {
            let i = (c * 37 + j * 101) % 700;
            db.put(&k(i), &v(i, c));
            model.insert(k(i), v(i, c));
        }
        db.commit().unwrap();
        history.push(digest_of_model(&model));
    };
    for c in 0..60 {
        commit(&mut db, c, 30);
    }
    db.flush().unwrap();
    db.settle().unwrap();
    for c in 60..100 {
        commit(&mut db, c, 2);
    }
    assert_eq!(digest_of(&db), *history.last().unwrap());
    drop(db);
    history
}

/// A byte changed anywhere in the directory is refused, or is where the
/// log ends: whatever opens afterwards holds a state the store
/// acknowledged, the last or one before it, and never anything else.
/// Every file is walked, and each of the three outcomes has to occur --
/// a store refused, a store that lost its log's tail, and a store whole
/// where the byte was in a file nothing durable rests on.
#[test]
fn a_changed_byte_is_refused_or_is_where_the_log_ends() {
    let dir = scratch("tamper");
    let history = with_a_history(&dir, 3);
    let pristine = snapshot(&dir);
    let (mut refused, mut earlier, mut whole) = (0, 0, 0);
    for (name, bytes) in &pristine {
        let stride = (bytes.len() / 24).max(1);
        for at in (0..bytes.len()).step_by(stride) {
            restore(&dir, &pristine);
            let mut bad = bytes.clone();
            bad[at] ^= 1;
            std::fs::write(dir.join(name), &bad).unwrap();
            match Db::open(&dir, tiny(3)) {
                Err(_) => refused += 1,
                Ok(db) => {
                    let d = digest_of(&db);
                    assert!(
                        history.contains(&d),
                        "{name}, byte {at}: opened as a state the store never held"
                    );
                    if d == *history.last().unwrap() {
                        whole += 1;
                    } else {
                        earlier += 1;
                    }
                }
            }
        }
    }
    assert!(
        refused > 0 && earlier > 0 && whole > 0,
        "refused {refused}, earlier {earlier}, whole {whole}"
    );
    // And untouched, it opens whole.
    restore(&dir, &pristine);
    assert_eq!(
        digest_of(&Db::open(&dir, tiny(3)).unwrap()),
        *history.last().unwrap()
    );
}

/// The log cut at any byte, as a power loss cuts it, opens: as the commits
/// whose records are whole before the cut, each a state the store
/// acknowledged, later for a later cut, and the whole store for the whole
/// log.
#[test]
fn a_log_cut_anywhere_opens_as_the_commits_before_the_cut() {
    let dir = scratch("cut");
    let history = with_a_history(&dir, 4);
    let pristine = snapshot(&dir);
    let (log, bytes) = pristine
        .iter()
        .rfind(|(n, _)| n.starts_with("wal-"))
        .expect("a log");
    let mut last = 0;
    let mut states = std::collections::BTreeSet::new();
    for cut in (0..bytes.len()).step_by(7).chain([bytes.len()]) {
        restore(&dir, &pristine);
        std::fs::write(dir.join(log), &bytes[..cut]).unwrap();
        let db = Db::open(&dir, tiny(4)).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
        let d = digest_of(&db);
        let at = history
            .iter()
            .position(|&h| h == d)
            .unwrap_or_else(|| panic!("cut at {cut}: a state the store never held"));
        assert!(at >= last, "cut at {cut}: state {at} after state {last}");
        last = at;
        states.insert(at);
    }
    assert_eq!(last, history.len() - 1, "the whole log is the whole store");
    assert!(states.len() > 3, "the cuts fell in {} states", states.len());
}

/// What a crash leaves in the directory beside the store: an envelope no
/// manifest names is removed at the open, and the store is whole without
/// it; a named one missing is refused.
#[test]
fn an_envelope_nobody_names_is_swept_and_one_missing_is_refused() {
    let n = 4_000;
    let dir = scratch("orphan");
    {
        let mut db = Db::create(&dir, under(5)).unwrap();
        fill(&mut db, n);
    }
    let segment = names(&dir)
        .into_iter()
        .find(|name| name.starts_with("x-") && !name.ends_with(".o"))
        .expect("an envelope");
    let orphan = format!("x-{}", "ab".repeat(16));
    std::fs::copy(dir.join(&segment), dir.join(&orphan)).unwrap();
    std::fs::write(dir.join(format!("{orphan}.o")), b"torn").unwrap();
    {
        let db = Db::open(&dir, under(5)).unwrap();
        holds(&db, n);
        assert_eq!(db.cipher_counts().unwrap().swept, 2);
    }
    assert!(!dir.join(&orphan).exists());
    assert!(!dir.join(format!("{orphan}.o")).exists());
    // An ordered index missing is composed again from its segment, as a
    // plain store's is, and written back with the next manifest.
    let index = format!("{segment}.o");
    std::fs::remove_file(dir.join(&index)).unwrap();
    {
        let mut db = Db::open(&dir, under(5)).unwrap();
        holds(&db, n);
        assert!(db.ord_rebuilt() > 0);
        db.put(&k(0), &v(0, 1));
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
    }
    std::fs::remove_file(dir.join(&segment)).ok();
    if !dir.join(&segment).exists() {
        // The flush above may have merged the segment away; whichever
        // envelope the store rests on now, one of them gone is refused.
        let any = names(&dir)
            .into_iter()
            .find(|name| name.starts_with("x-") && !name.ends_with(".o"));
        if let Some(any) = any {
            std::fs::remove_file(dir.join(any)).unwrap();
        }
    }
    assert!(Db::open(&dir, under(5)).is_err());
}

/// A store dropped without its close, its memory gone with it, opens from
/// its directory as everything it had committed; and a copy of the
/// directory taken while the store is open opens beside it, under the same
/// cipher, as the store at that moment.
#[test]
fn a_crash_and_a_copy_open_as_what_was_committed() {
    let n = 12_000;
    let dir = scratch("crash");
    {
        let mut db = Db::create(&dir, under(6)).unwrap();
        for i in 0..n {
            db.append(&k(i), &v(i, 0));
            if i % 250 == 249 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        for i in (0..n).rev().step_by(3) {
            db.put(&k(i), &v(i, 1));
            if i % 250 < 3 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        // No flush, no settle, no close: seals in flight, the tail in the
        // log, and every memory file gone at the drop.
    }
    // And again before anything is written: what the first open replayed
    // out of the log is in memory alone, so the log it replayed has to be
    // there for the second.
    holds(&Db::open(&dir, under(6)).unwrap(), n);
    let mut db = Db::open(&dir, under(6)).unwrap();
    holds(&db, n);
    db.flush().unwrap();
    db.settle().unwrap();
    let copy = scratch("crash-copy");
    std::fs::create_dir_all(&copy).unwrap();
    restore(&copy, &snapshot(&dir));
    let twin = Db::open(&copy, under(6)).unwrap();
    holds(&twin, n);
    holds(&db, n);
}

/// What a store under a cipher runs without, it runs without whatever it
/// is asked for: ordered ingest's run, the compact form and the recycled
/// log each rest on a file the engine syncs itself, which under a cipher
/// is a memory file. Asked for all three, the store keeps what it commits
/// across a crash, and its directory holds a store under a cipher's files
/// and no other.
#[test]
fn a_store_under_a_cipher_runs_without_what_it_cannot_seal() {
    let n = 12_000;
    let dir = scratch("forced");
    let asked = || Options {
        direct_ingest: true,
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        recycle_wal: true,
        ..under(7)
    };
    {
        let mut db = Db::create(&dir, asked()).unwrap();
        // Ordered, committed, and dropped: a run of ordered ingest would
        // have been a memory file synced at each commit, and gone.
        for i in 0..n {
            db.append(&k(i), &v(i, 0));
            if i % 250 == 249 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
    }
    {
        let mut db = Db::open(&dir, asked()).unwrap();
        let mut seen = 0u32;
        db.scan(b"", usize::MAX, kv(&mut |_k, _v| seen += 1))
            .unwrap();
        assert_eq!(seen, n, "what was committed before the crash");
        for i in (0..n).rev().step_by(3) {
            db.put(&k(i), &v(i, 1));
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        holds(&db, n);
        assert!(
            db.zpage_form_files().is_empty(),
            "no form file under a cipher"
        );
        assert_eq!(db.zpage_counts().build_starts, 0, "and none begun");
    }
    for name in names(&dir) {
        let known = name == "supdb.cipher"
            || name.starts_with("manifest")
            || name.starts_with("wal-")
            || name.starts_with("x-");
        assert!(known, "a file named {name}");
    }
    for needle in [MARK, KEY_MARK] {
        assert!(files_holding(&dir, needle.as_bytes()).is_empty());
    }
    holds(&Db::open(&dir, asked()).unwrap(), n);
}

/// The caller's key rotated under a store with data in it
/// (`supdb::cipher_rewrap`): the store opens under the new key and holds
/// what it held, the old key opens nothing, and no file but the marker
/// changed.
#[test]
fn a_store_handed_to_another_key_opens_under_that_one() {
    let n = 4_000;
    let dir = scratch("rewrap");
    {
        let mut db = Db::create(&dir, under(8)).unwrap();
        fill(&mut db, n);
    }
    let before = snapshot(&dir);
    supdb::cipher_rewrap(&dir, &cipher(8), &cipher(9)).unwrap();
    let after = snapshot(&dir);
    for ((name, was), (_, is)) in before.iter().zip(&after) {
        assert_eq!(was == is, name != "supdb.cipher", "{name}");
    }
    assert_eq!(supdb::cipher_key_id(&dir).unwrap(), Some(9));
    assert!(Db::open(&dir, under(8)).is_err());
    assert_eq!(snapshot(&dir), after, "a refused open changes nothing");
    holds(&Db::open(&dir, under(9)).unwrap(), n);
}
