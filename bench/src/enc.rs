//! The encrypted comparators (feature `enc`): what a user who needs a store
//! encrypted at rest would otherwise pick, to be read beside `supdb-cipher`.
//!
//! Two, each the comparator's own mechanism under a cipher the application
//! supplies, as the engine's is:
//!
//! - **LMDB** encrypts on its 1.0 line (`mdb.master3`): a page is handed to
//!   a hook with a key and a nonce the store derives, and comes back
//!   ciphertext and a tag. `lmdb3-enc` is that under AES-256-GCM
//!   (`shim/lmdb_enc.c`), and `lmdb3` the same library with no hook, so the
//!   pair prices the encryption and `lmdb3` against the released line's
//!   `lmdb` (the default build's) what the 1.0 line itself moved.
//! - **RocksDB** encrypts through an `Env` that wraps every file in a
//!   cipher stream. `rocksdb-tuned-enc` is `rocksdb-tuned` on that Env under
//!   AES-256-CTR (`shim/rocks_enc.cc`): unauthenticated, which is what the
//!   mechanism is, where the other two stores authenticate every byte.
//!
//! Both ciphers are OpenSSL's, the library their deployments run them
//! under; the engine's is `ring`'s, the one its own tests link. One key for
//! all of them, fixed: nothing here is a secret, and a key drawn a run would
//! be a difference between runs.

use super::{consume, dir_size, Engine, Features, ReaderOpener, Res, ThreadReader};
use lmdb_master3_sys as mdb;
use std::ffi::{c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The key every encrypted comparator runs under.
pub(super) const KEY: [u8; 32] = [0x5a; 32];

// ----------------------------------------------------------------- lmdb 3 --

extern "C" {
    /// LMDB's page hook under AES-256-GCM (`shim/lmdb_enc.c`).
    fn supdb_bench_lmdb_encfunc(
        src: *const mdb::MDB_val,
        dst: *mut mdb::MDB_val,
        key: *const mdb::MDB_val,
        encdec: c_int,
    ) -> c_int;
}

/// The tag LMDB keeps room for at a page's end.
const TAG: u32 = 16;

fn check(rc: c_int) -> Res<()> {
    if rc == 0 {
        return Ok(());
    }
    // SAFETY: LMDB answers a static string for any code.
    let msg = unsafe { std::ffi::CStr::from_ptr(mdb::mdb_strerror(rc)) };
    Err(msg.to_string_lossy().into_owned())
}

fn val(bytes: &[u8]) -> mdb::MDB_val {
    mdb::MDB_val {
        mv_size: bytes.len(),
        mv_data: bytes.as_ptr() as *mut c_void,
    }
}

/// What LMDB lent, for as long as the transaction it was read under makes
/// no other call.
///
/// # Safety
/// `v` is a value LMDB just filled under a live transaction.
unsafe fn lent<'a>(v: &mdb::MDB_val) -> &'a [u8] {
    std::slice::from_raw_parts(v.mv_data as *const u8, v.mv_size)
}

/// An open environment and its one database, closed at the last handle's
/// drop.
///
/// On the C API and not through a wrapper crate, for the hook: the wrapper
/// over these bindings (`heed3`, at 0.22 and 0.23) copies a page's
/// ciphertext into the output and decrypts it there, and LMDB lets two
/// transactions decrypt one page into the buffer they share, so a reader
/// thread meets a page of ciphertext for the length of another's
/// decryption (`shim/lmdb_enc.c` has the whole of it). It also makes its
/// cipher from the key at every page. Neither is LMDB's, and an arm that
/// charged LMDB for them would be measuring the wrapper.
struct Env3 {
    env: *mut mdb::MDB_env,
    dbi: mdb::MDB_dbi,
}

// SAFETY: an LMDB environment is shared between threads by design; what is
// a thread's own is a transaction, which `Read3` begins where it is used.
unsafe impl Send for Env3 {}
unsafe impl Sync for Env3 {}

impl Drop for Env3 {
    fn drop(&mut self) {
        // SAFETY: the last handle: every transaction holds one.
        unsafe { mdb::mdb_env_close(self.env) };
    }
}

impl Env3 {
    fn open(path: &Path, map_gb: usize, sync: bool, enc: bool) -> Res<Env3> {
        use std::os::unix::ffi::OsStrExt;
        let cpath =
            std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let mut env = std::ptr::null_mut();
        // SAFETY: the calls LMDB documents for opening an environment, in
        // their order, on a handle nothing else has; closed on any failure.
        unsafe {
            check(mdb::mdb_env_create(&mut env))?;
            let opened = (|| {
                if enc {
                    // LMDB copies the key.
                    let key = val(&KEY);
                    check(mdb::mdb_env_set_encrypt(
                        env,
                        Some(supdb_bench_lmdb_encfunc),
                        &key,
                        TAG,
                    ))?;
                }
                check(mdb::mdb_env_set_mapsize(env, map_gb << 30))?;
                check(mdb::mdb_env_set_maxdbs(env, 4))?;
                let flags = if sync { 0 } else { mdb::MDB_NOSYNC };
                check(mdb::mdb_env_open(env, cpath.as_ptr(), flags, 0o600))?;
                let mut txn = std::ptr::null_mut();
                check(mdb::mdb_txn_begin(env, std::ptr::null_mut(), 0, &mut txn))?;
                let mut dbi = 0;
                if let Err(e) = check(mdb::mdb_dbi_open(txn, std::ptr::null(), 0, &mut dbi)) {
                    mdb::mdb_txn_abort(txn);
                    return Err(e);
                }
                check(mdb::mdb_txn_commit(txn))?;
                Ok(dbi)
            })();
            match opened {
                Ok(dbi) => Ok(Env3 { env, dbi }),
                Err(e) => {
                    mdb::mdb_env_close(env);
                    Err(e)
                }
            }
        }
    }
}

/// A read transaction, begun on the thread that reads through it -- LMDB
/// keeps a reader's slot in the thread's own storage -- and ended at its
/// drop, before the environment it holds a handle on.
struct Read3 {
    txn: *mut mdb::MDB_txn,
    env: Arc<Env3>,
}

impl Drop for Read3 {
    fn drop(&mut self) {
        // SAFETY: a live read transaction, ended once.
        unsafe { mdb::mdb_txn_abort(self.txn) };
    }
}

impl Read3 {
    fn begin(env: &Arc<Env3>) -> Res<Read3> {
        let mut txn = std::ptr::null_mut();
        // SAFETY: a read transaction on an open environment.
        check(unsafe {
            mdb::mdb_txn_begin(env.env, std::ptr::null_mut(), mdb::MDB_RDONLY, &mut txn)
        })?;
        Ok(Read3 {
            txn,
            env: env.clone(),
        })
    }

    fn get(&self, key: &[u8]) -> Res<usize> {
        let (mut k, mut v) = (val(key), val(&[]));
        // SAFETY: a lookup under a live transaction; the value is lent from
        // the mapping, or from the transaction's decrypted page, and the
        // caller's copy is `consume`, made before any other call.
        match unsafe { mdb::mdb_get(self.txn, self.env.dbi, &mut k, &mut v) } {
            0 => Ok(consume(unsafe { lent(&v) })),
            mdb::MDB_NOTFOUND => Ok(0),
            rc => check(rc).map(|()| 0),
        }
    }

    fn range(&self, from: &[u8], n: usize) -> Res<usize> {
        let mut cur = std::ptr::null_mut();
        // SAFETY: a cursor of a live transaction, closed below on every
        // path; each value is copied out before the cursor moves.
        check(unsafe { mdb::mdb_cursor_open(self.txn, self.env.dbi, &mut cur) })?;
        let (mut k, mut v) = (val(from), val(&[]));
        let (mut bytes, mut seen) = (0usize, 0usize);
        let mut rc = if n == 0 {
            mdb::MDB_NOTFOUND
        } else {
            unsafe { mdb::mdb_cursor_get(cur, &mut k, &mut v, mdb::MDB_SET_RANGE) }
        };
        while rc == 0 {
            bytes += consume(unsafe { lent(&v) });
            seen += 1;
            if seen == n {
                break;
            }
            rc = unsafe { mdb::mdb_cursor_get(cur, &mut k, &mut v, mdb::MDB_NEXT) };
        }
        unsafe { mdb::mdb_cursor_close(cur) };
        match rc {
            0 | mdb::MDB_NOTFOUND => Ok(bytes),
            rc => check(rc).map(|()| 0),
        }
    }
}

/// LMDB's 1.0 line, plain (`lmdb3`) or encrypted (`lmdb3-enc`), each with
/// its buffered twin (`-nosync`): `Lmdb`, the released line's arm, again in
/// every respect but the library under it and the hook -- one database, a
/// batch a write transaction, a read transaction held across reads and
/// dropped at a write, a cursor a scan.
///
/// The encrypted arm runs the suite's passes to three hundred thousand
/// keys and not to a million. An encrypted environment reads through a
/// table of decrypted chunks a transaction, sixteen pages each, which it
/// purges of the chunks nothing refers to once two thousand are in it, and
/// a write transaction of a thousand keys in arrival order over a store of
/// a million reaches that: the commit then answers `MDB_PROBLEM`, or a put
/// `MDB_CURSOR_FULL`, where the plain line and transactions of a hundred
/// keys answer nothing (`a_write_transaction_over_two_thousand_chunks`,
/// ignored, prints each). That is the line's own, at the commit the
/// binding crate vendors, with nothing of the suite in the reproduction;
/// the arm is not given smaller transactions to get past it, since a batch
/// a transaction is the guarantee every arm of its group is matched on.
pub struct Lmdb3 {
    /// Declared before the environment it reads, and dropped first.
    txn: Option<Read3>,
    env: Arc<Env3>,
    path: PathBuf,
    nosync: bool,
    enc: bool,
}

impl Lmdb3 {
    pub fn create(path: &Path, map_gb: usize, sync: bool, enc: bool) -> Res<Lmdb3> {
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        Ok(Lmdb3 {
            txn: None,
            env: Arc::new(Env3::open(path, map_gb, sync, enc)?),
            path: path.to_path_buf(),
            nosync: !sync,
            enc,
        })
    }

    /// The held read transaction, begun if there is not one.
    fn snapshot(&mut self) -> Res<&Read3> {
        if self.txn.is_none() {
            self.txn = Some(Read3::begin(&self.env)?);
        }
        Ok(self.txn.as_ref().expect("just filled"))
    }
}

impl ThreadReader for Read3 {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        Read3::get(self, key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        Read3::range(self, from, n)
    }
}

impl Engine for Lmdb3 {
    fn name(&self) -> &'static str {
        match (self.enc, self.nosync) {
            (false, false) => "lmdb3",
            (false, true) => "lmdb3-nosync",
            (true, false) => "lmdb3-enc",
            (true, true) => "lmdb3-nosync-enc",
        }
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: !self.nosync,
            transactions: true,
            // Matched off, as every arm's is: an encrypted page carries a
            // tag, which the suite reads as the cipher's and not as a
            // checksum the plain arms could have turned on.
            checksums: false,
            reopen_for_write: true,
            read_your_writes: true,
            ordered_scan: true,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        // A held read transaction pins the version it was begun at.
        self.txn = None;
        let mut w = std::ptr::null_mut();
        // SAFETY: one write transaction, committed or aborted here.
        unsafe {
            check(mdb::mdb_txn_begin(
                self.env.env,
                std::ptr::null_mut(),
                0,
                &mut w,
            ))?;
            for &(k, v) in items {
                let (mut k, mut v) = (val(k), val(v));
                if let Err(e) = check(mdb::mdb_put(w, self.env.dbi, &mut k, &mut v, 0)) {
                    mdb::mdb_txn_abort(w);
                    return Err(e);
                }
            }
            check(mdb::mdb_txn_commit(w))
        }
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        self.snapshot()?.get(key)
    }
    fn range(&mut self, from: &[u8], n: usize) -> Res<usize> {
        self.snapshot()?.range(from, n)
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        // The environment is shared and the transaction is the thread's,
        // begun on it after the load's last commit.
        let env = self.env.clone();
        Ok(Box::new(move || {
            Ok(Box::new(Read3::begin(&env)?) as Box<dyn ThreadReader>)
        }))
    }
    fn sync(&mut self) -> Res<()> {
        self.txn = None;
        // SAFETY: a forced sync of an open environment.
        check(unsafe { mdb::mdb_env_sync(self.env.env, 1) })
    }
    fn size_bytes(&self) -> u64 {
        dir_size(&self.path)
    }
}

// ---------------------------------------------------------------- rocksdb --

extern "C" {
    fn supdb_bench_rocks_encrypted_env(key: *const u8) -> *mut std::ffi::c_void;
    #[cfg(test)]
    fn supdb_bench_rocks_counter_at(iv: *const u8, blocks: u64, out: *mut u8);
}

/// What `rocksdb::Env` holds behind its `Arc`, which the crate keeps to
/// itself: the C API's handle on an Env, and nothing else.
#[repr(C)]
struct EnvHandle {
    inner: *mut std::ffi::c_void,
}

// SAFETY: the handle is RocksDB's Env, which is thread-safe; the crate's
// own wrapper of the same pointer says the same.
unsafe impl Send for EnvHandle {}
unsafe impl Sync for EnvHandle {}

/// RocksDB's encrypted Env under `KEY` (`shim/rocks_enc.cc`), as the value
/// `rocksdb::Options::set_env` takes.
///
/// The crate offers the default Env and an in-memory one and no way to hand
/// it another: its `Env` is a private `Arc` over a private struct of one
/// field, the C API's handle. So the handle the shim makes is given that
/// shape and the value reinterpreted. What that rests on, and what holds
/// each: the two values are one pointer wide, which the transmute refuses
/// to compile without; the crate's struct is the handle alone, which the
/// pin on the crate's version keeps and `the_encrypted_rocksdb_keeps_its_
/// keys_from_the_device` would fail loudly without, since a handle read
/// from the wrong place is no Env at all; and its drop is the crate's own,
/// `rocksdb_env_destroy` on the handle, which is how the shim's Env is
/// freed.
pub(super) fn rocks_env() -> rocksdb::Env {
    // SAFETY: a 32-byte key, read once by the callee.
    let inner = unsafe { supdb_bench_rocks_encrypted_env(KEY.as_ptr()) };
    assert!(!inner.is_null(), "the shim made an Env");
    let handle = std::sync::Arc::new(EnvHandle { inner });
    // SAFETY: see above.
    unsafe { std::mem::transmute::<std::sync::Arc<EnvHandle>, rocksdb::Env>(handle) }
}

#[cfg(test)]
mod tests {
    use super::super::{open, Engine};
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("supdb-bench-enc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Whether any file under `dir` holds `needle`.
    fn on_device(dir: &Path, needle: &[u8]) -> bool {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let bytes = std::fs::read(&p).unwrap();
                if bytes.windows(needle.len()).any(|w| w == needle) {
                    return true;
                }
            }
        }
        false
    }

    /// A store of `n` keys whose keys and values each carry a marker, read
    /// back whole through the arm's own reads and through a reader thread's.
    fn load_and_read(arm: &str, dir: &Path, n: usize) -> Box<dyn Engine> {
        let mut e = open(arm, dir, 1).unwrap();
        assert_eq!(e.name(), arm);
        let rows: Vec<(Vec<u8>, Vec<u8>)> = (0..n)
            .map(|i| {
                (
                    format!("KEYMARK{i:09}").into_bytes(),
                    format!("VALMARK{i:09}-{}", "v".repeat(80)).into_bytes(),
                )
            })
            .collect();
        for chunk in rows.chunks(500) {
            let items: Vec<(&[u8], &[u8])> = chunk.iter().map(|(k, v)| (&k[..], &v[..])).collect();
            e.write_batch(&items).unwrap();
        }
        e.sync().unwrap();
        let len = rows[0].1.len();
        for (k, _) in rows.iter().step_by(7) {
            assert_eq!(e.get(k).unwrap(), len, "{arm}: a key read back");
        }
        assert_eq!(e.get(b"KEYMARK-absent").unwrap(), 0, "{arm}: an absent key");
        assert_eq!(
            e.range(&rows[n / 2].0, 100).unwrap(),
            100 * len,
            "{arm}: a scan of a hundred"
        );
        let opener = e.thread_reader().unwrap();
        let probe = rows[n / 3].0.clone();
        let from = rows[n / 4].0.clone();
        let (got, scanned) = std::thread::scope(|s| {
            s.spawn(move || {
                let mut r = opener().unwrap();
                (r.get(&probe).unwrap(), r.range(&from, 50).unwrap())
            })
            .join()
            .unwrap()
        });
        assert_eq!((got, scanned), (len, 50 * len), "{arm}: a reader thread");
        e
    }

    /// The encrypted arms are their plain twins with the keys and values
    /// off the device: each reads back what it wrote, the plain twin's
    /// files hold the markers -- the control, since a search that finds
    /// nothing anywhere holds nothing -- and the encrypted arm's hold
    /// neither, in a table file or in a log.
    #[test]
    fn the_encrypted_comparators_keep_their_keys_from_the_device() {
        let root = scratch("device");
        for (plain, enc) in [
            ("lmdb3", "lmdb3-enc"),
            ("lmdb3-nosync", "lmdb3-nosync-enc"),
            ("rocksdb-tuned", "rocksdb-tuned-enc"),
            ("rocksdb-tuned-nosync", "rocksdb-tuned-nosync-enc"),
        ] {
            let (dp, de) = (root.join(plain), root.join(enc));
            let p = load_and_read(plain, &dp, 3000);
            let e = load_and_read(enc, &de, 3000);
            assert_eq!(p.features().unmatched(&e.features()), Vec::<&str>::new());
            for needle in [&b"KEYMARK0000"[..], &b"VALMARK0000"[..]] {
                assert!(
                    on_device(&dp, needle),
                    "{plain}: the control finds its marker"
                );
                assert!(!on_device(&de, needle), "{enc}: a marker on the device");
            }
            // The runner's sequence: a pass drops its store, removes the
            // directory and opens a second at the same path, so an arm's
            // drop closes what its open made -- nothing under the path
            // still open, and the second store without the first's keys.
            drop((p, e));
            for (arm, dir) in [(plain, &dp), (enc, &de)] {
                std::fs::remove_dir_all(dir).unwrap();
                #[cfg(target_os = "linux")]
                {
                    let held: Vec<_> = std::fs::read_dir("/proc/self/fd")
                        .unwrap()
                        .filter_map(|f| std::fs::read_link(f.ok()?.path()).ok())
                        .filter(|p| p.starts_with(dir))
                        .collect();
                    assert!(held.is_empty(), "{arm}: dropped and still open: {held:?}");
                }
                let mut again = open(arm, dir, 1).unwrap();
                assert_eq!(
                    again.get(b"KEYMARK000000007").unwrap(),
                    0,
                    "{arm}: the second store read the first's key"
                );
                drop(again);
                drop(load_and_read(arm, dir, 500));
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An encrypted RocksDB flushed and compacted reads back through its
    /// table files: the reads above may be the memtable's, and the stream's
    /// arithmetic at an offset is a table read's.
    #[test]
    fn the_encrypted_rocksdb_reads_its_table_files() {
        let dir = scratch("tables");
        let mut o = rocksdb::Options::default();
        o.create_if_missing(true);
        o.set_compression_type(rocksdb::DBCompressionType::None);
        o.set_env(&rocks_env());
        let db = rocksdb::DB::open(&o, &dir).unwrap();
        let val = |i: usize| format!("VALMARK{i:09}-{}", "w".repeat(i % 97)).into_bytes();
        for i in 0..20_000usize {
            db.put(format!("KEYMARK{i:09}"), val(i)).unwrap();
        }
        db.flush().unwrap();
        db.compact_range::<&[u8], &[u8]>(None, None);
        assert!(
            std::fs::read_dir(&dir).unwrap().any(|e| e
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "sst")),
            "a table file on the device"
        );
        drop(db);
        // Reopened, so that nothing read is a memtable's or a cache's.
        let db = rocksdb::DB::open(&o, &dir).unwrap();
        for i in (0..20_000usize).step_by(13) {
            assert_eq!(
                db.get(format!("KEYMARK{i:09}")).unwrap().as_deref(),
                Some(&val(i)[..]),
                "key {i} through a table file"
            );
        }
        assert!(!on_device(&dir, b"KEYMARK0000"), "a key on the device");
        assert!(!on_device(&dir, b"VALMARK0000"), "a value on the device");
        // The same files without the key are not a store.
        drop(db);
        let mut plain = rocksdb::Options::default();
        plain.create_if_missing(false);
        assert!(
            rocksdb::DB::open(&plain, &dir).is_err(),
            "an encrypted store opened with no cipher"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What an encrypted environment makes of write transactions that
    /// reach across the store: keys of the suite's shape in a fixed
    /// shuffled order, a transaction a batch, into the plain line and the
    /// encrypted one, at two sizes and two batch lengths. It asserts the
    /// plain line alone, which is the control; the encrypted line's
    /// answers are printed, since they are LMDB's to change:
    ///
    ///   cargo test --release --no-default-features --features enc \
    ///     --target-dir target-enc --lib enc::tests::a_write_transaction \
    ///     -- --ignored --nocapture
    #[test]
    #[ignore = "a probe of the comparator, a minute and a half; run by name"]
    fn a_write_transaction_over_two_thousand_chunks() {
        fn load(dir: &Path, enc: bool, n: usize, batch: usize) -> Res<()> {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let env = Env3::open(dir, 4, true, enc)?;
            // A fixed permutation of the keys' indexes.
            let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
            let mut order: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                order.swap(i, (x % (i as u64 + 1)) as usize);
            }
            let value = [7u8; 100];
            for (b, chunk) in order.chunks(batch).enumerate() {
                let mut w = std::ptr::null_mut();
                // SAFETY: one write transaction, committed or aborted here.
                unsafe {
                    check(mdb::mdb_txn_begin(env.env, std::ptr::null_mut(), 0, &mut w))?;
                    for &i in chunk {
                        let k = format!("{i:016}");
                        let (mut k, mut v) = (val(k.as_bytes()), val(&value));
                        if let Err(e) = check(mdb::mdb_put(w, env.dbi, &mut k, &mut v, 0)) {
                            mdb::mdb_txn_abort(w);
                            return Err(format!("a put of batch {b}: {e}"));
                        }
                    }
                    check(mdb::mdb_txn_commit(w))
                        .map_err(|e| format!("the commit of batch {b}: {e}"))?;
                }
            }
            Ok(())
        }
        let root = scratch("chunks");
        for (n, batch) in [
            (300_000usize, 1000usize),
            (1_000_000, 1000),
            (1_000_000, 100),
        ] {
            let plain = load(&root.join("plain"), false, n, batch);
            assert_eq!(plain, Ok(()), "the plain line, {n} keys by {batch}");
            let enc = load(&root.join("enc"), true, n, batch);
            eprintln!("{n} keys in transactions of {batch}: plain Ok, encrypted {enc:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The counter block of the stream: the IV plus the block's index as
    /// 128-bit big-endian numbers, carried through every byte.
    #[test]
    fn the_streams_counter_is_the_iv_plus_the_block() {
        let at = |iv: [u8; 16], blocks: u64| {
            let mut out = [0u8; 16];
            // SAFETY: two 16-byte buffers.
            unsafe { supdb_bench_rocks_counter_at(iv.as_ptr(), blocks, out.as_mut_ptr()) };
            u128::from_be_bytes(out)
        };
        for (iv, blocks) in [
            (0u128, 0u64),
            (0, 1),
            (0xff, 1),
            (u128::MAX, 1),
            (u128::MAX - 5, 9),
            (0x0123_4567_89ab_cdef_0123_4567_89ab_cdef, u64::MAX),
            ((1u128 << 64) - 1, 1),
            (0xffff_ffff_ffff_ffff_ffff_ffff_0000_0000, 0x1_0000_0000),
        ] {
            assert_eq!(
                at(iv.to_be_bytes(), blocks),
                iv.wrapping_add(blocks as u128),
                "iv {iv:#x} + {blocks}"
            );
        }
    }

    /// The hook is AES-256-GCM as it says, read against `ring`'s: a page
    /// sealed by the hook under LMDB's own arguments -- the key, the
    /// page's number and transaction as the vector, room for a tag --
    /// opens through `ring` under the nonce the hook documents, and comes
    /// back through the hook; a changed byte, tag or page number is
    /// refused; and the input is left as it was. That the output never
    /// holds anything but the plaintext on the way is the threaded test's
    /// to hold, below: a hook that decrypts in its output passes this one.
    #[test]
    fn the_lmdb_hook_is_aes_256_gcm_written_from_the_input() {
        let page: Vec<u8> = (0..4064u32).map(|i| (i * 31 % 251) as u8).collect();
        let (pgno, txnid) = (0x1_2345_6789u64, 77u64);
        let hook = |input: &[u8], iv: &[u8; 16], tag: &mut [u8; 16], enc: bool| {
            let mut out = vec![0xEEu8; input.len()];
            let keys = [val(&KEY), val(iv), val(&tag[..])];
            let (src, mut dst) = (val(input), val(&out));
            // SAFETY: LMDB's own call: three values, the tag's writable.
            let rc = unsafe {
                supdb_bench_lmdb_encfunc(&src, &mut dst, keys.as_ptr(), c_int::from(enc))
            };
            out.truncate(input.len());
            (rc, out)
        };
        let mut iv = [0u8; 16];
        iv[..8].copy_from_slice(&pgno.to_ne_bytes());
        iv[8..].copy_from_slice(&txnid.to_ne_bytes());
        let mut tag = [0u8; 16];
        let (rc, sealed) = hook(&page, &iv, &mut tag, true);
        assert_eq!(rc, 0);
        assert_ne!(sealed, page);
        assert_ne!(tag, [0u8; 16]);
        // `ring`, under the nonce the hook documents.
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&(pgno as u32).to_ne_bytes());
        nonce[4..].copy_from_slice(&txnid.to_ne_bytes());
        let direct = ring::aead::LessSafeKey::new(
            ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, &KEY).unwrap(),
        );
        let mut whole = sealed.clone();
        whole.extend_from_slice(&tag);
        let opened = direct
            .open_in_place(
                ring::aead::Nonce::assume_unique_for_key(nonce),
                ring::aead::Aad::empty(),
                &mut whole,
            )
            .expect("the hook's page opens under ring");
        assert_eq!(opened, &page[..]);
        // Back through the hook, the input untouched.
        let before = sealed.clone();
        let (rc, back) = hook(&sealed, &iv, &mut tag.clone(), false);
        assert_eq!((rc, &back, &sealed), (0, &page, &before));
        // Refused: a byte, the tag, the page it is of.
        let mut bent = sealed.clone();
        bent[100] ^= 1;
        assert_ne!(hook(&bent, &iv, &mut tag.clone(), false).0, 0);
        let mut tag2 = tag;
        tag2[0] ^= 1;
        assert_ne!(hook(&sealed, &iv, &mut tag2, false).0, 0);
        let mut iv2 = iv;
        iv2[0] ^= 1;
        assert_ne!(hook(&sealed, &iv2, &mut tag.clone(), false).0, 0);
    }

    /// Reader threads over an encrypted LMDB read what was written: every
    /// key by point read and every run of a hundred by scan, three threads
    /// at once from a cold environment, compared byte for byte. With a hook
    /// that decrypts in its output, a thread reads a page another is
    /// decrypting and LMDB answers `MDB_PAGE_NOTFOUND`, or a value that is
    /// not the key's.
    #[test]
    fn reader_threads_over_an_encrypted_lmdb_read_what_was_written() {
        let dir = scratch("threads");
        let n = 60_000usize;
        let key = |i: usize| format!("KEYMARK{i:09}").into_bytes();
        let value = |i: usize| format!("VALMARK{i:09}-{}", "v".repeat(40 + i % 50)).into_bytes();
        {
            let mut e = Lmdb3::create(&dir, 1, true, true).unwrap();
            let rows: Vec<(Vec<u8>, Vec<u8>)> = (0..n).map(|i| (key(i), value(i))).collect();
            for chunk in rows.chunks(1000) {
                let items: Vec<(&[u8], &[u8])> =
                    chunk.iter().map(|(k, v)| (&k[..], &v[..])).collect();
                e.write_batch(&items).unwrap();
            }
            e.sync().unwrap();
        }
        for round in 0..3 {
            // Reopened each round, so that no page is decrypted yet.
            let env = Arc::new(Env3::open(&dir, 1, true, true).unwrap());
            std::thread::scope(|s| {
                for t in 0..3usize {
                    let env = env.clone();
                    s.spawn(move || {
                        let r = Read3::begin(&env).unwrap();
                        let mut v = val(&[]);
                        for i in (t..n).step_by(3) {
                            let kb = key(i);
                            let mut k = val(&kb);
                            // SAFETY: a lookup under the thread's transaction.
                            let rc = unsafe { mdb::mdb_get(r.txn, env.dbi, &mut k, &mut v) };
                            assert_eq!(rc, 0, "round {round} thread {t}: key {i}: {:?}", check(rc));
                            assert_eq!(
                                unsafe { lent(&v) },
                                &value(i)[..],
                                "round {round} thread {t}: key {i}"
                            );
                        }
                        let mut cur = std::ptr::null_mut();
                        // SAFETY: a cursor of the thread's transaction.
                        unsafe {
                            check(mdb::mdb_cursor_open(r.txn, env.dbi, &mut cur)).unwrap();
                            let first = key(0);
                            let mut k = val(&first);
                            let mut rc =
                                mdb::mdb_cursor_get(cur, &mut k, &mut v, mdb::MDB_SET_RANGE);
                            for i in 0..n {
                                assert_eq!(
                                    rc,
                                    0,
                                    "round {round} thread {t}: scan at {i}: {:?}",
                                    check(rc)
                                );
                                assert_eq!(
                                    lent(&k),
                                    &key(i)[..],
                                    "round {round} thread {t}: scan key {i}"
                                );
                                assert_eq!(
                                    lent(&v),
                                    &value(i)[..],
                                    "round {round} thread {t}: scan value {i}"
                                );
                                rc = mdb::mdb_cursor_get(cur, &mut k, &mut v, mdb::MDB_NEXT);
                            }
                            assert_eq!(rc, mdb::MDB_NOTFOUND, "round {round} thread {t}: the end");
                            mdb::mdb_cursor_close(cur);
                        }
                    });
                }
            });
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
