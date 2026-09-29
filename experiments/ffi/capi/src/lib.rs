//! A native C ABI over the engine, for one experiment: what a language
//! boundary costs a point read, with the value borrowed against copied.
//!
//! The shape is the suite's `supdb` arm -- the engine's defaults with
//! segment checksums off and compact records, read through the writer's own
//! handle -- and the read is the suite's: `Reader::read_all`, which lends
//! each value to a callback. A borrow hands the callback's pointer out
//! across the boundary, as `mdb_get` does; it stays valid until the next
//! commit, flush or close, which is LMDB's rule with the transaction
//! replaced by the store's quiescence. A copy fills a caller's buffer.
//!
//! Every function takes a handle from `supdb_capi_open`. Errors return a
//! negative status and leave the message in `supdb_capi_error`.

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::time::Instant;

use supdb::{Db, Options, SegmentOptions};

pub struct Handle {
    db: Db,
}

thread_local! {
    static ERR: RefCell<CString> = RefCell::new(CString::default());
}

fn fail<T>(msg: impl Into<Vec<u8>>, v: T) -> T {
    ERR.with(|e| *e.borrow_mut() = CString::new(msg).unwrap_or_default());
    v
}

/// The last error on this thread, as a C string that stays valid until the
/// next failing call.
#[no_mangle]
pub extern "C" fn supdb_capi_error() -> *const c_char {
    ERR.with(|e| e.borrow().as_ptr())
}

/// The suite's `supdb` arm, stated once here: the engine's defaults with
/// segment checksums off (LMDB has none) and compact records.
fn arm_options() -> Options {
    Options {
        segment: SegmentOptions {
            checksums: false,
            compact_records: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Open the store at `dir`, creating it when `create` is nonzero. Null on
/// failure.
///
/// # Safety
/// `dir` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_open(dir: *const c_char, create: i32) -> *mut Handle {
    let dir = match CStr::from_ptr(dir).to_str() {
        Ok(s) => Path::new(s),
        Err(e) => return fail(e.to_string(), std::ptr::null_mut()),
    };
    let r = if create != 0 {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return fail(e.to_string(), std::ptr::null_mut());
        }
        Db::create(dir, arm_options())
    } else {
        Db::open(dir, arm_options())
    };
    match r {
        Ok(db) => Box::into_raw(Box::new(Handle { db })),
        Err(e) => fail(e.to_string(), std::ptr::null_mut()),
    }
}

/// # Safety
/// `h` came from `supdb_capi_open` and is not used after this.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_close(h: *mut Handle) -> i32 {
    if h.is_null() {
        return 0;
    }
    let h = Box::from_raw(h);
    match h.db.close() {
        Ok(()) => 0,
        Err(e) => fail(e.to_string(), -1),
    }
}

/// Stage one record for the next commit.
///
/// # Safety
/// `key` and `val` point at `klen` and `vlen` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_append(
    h: *mut Handle,
    key: *const u8,
    klen: usize,
    val: *const u8,
    vlen: usize,
) {
    let h = &mut *h;
    h.db.append(
        std::slice::from_raw_parts(key, klen),
        std::slice::from_raw_parts(val, vlen),
    );
}

/// Commit what is staged: one WAL append and one fdatasync.
///
/// # Safety
/// `h` is live.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_commit(h: *mut Handle) -> i32 {
    match (*h).db.commit() {
        Ok(()) => 0,
        Err(e) => fail(e.to_string(), -1),
    }
}

/// Drain: seal the memtable and partition what it sealed, which is what the
/// arm's `sync` does before the suite reads.
///
/// # Safety
/// `h` is live.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_flush(h: *mut Handle) -> i32 {
    match (*h).db.flush() {
        Ok(()) => 0,
        Err(e) => fail(e.to_string(), -1),
    }
}

/// The first value of `key`, borrowed: `*out` and `*len` describe bytes the
/// store owns, valid until the next commit, flush or close. Returns 1 when
/// the key has a value, 0 when it has none, -1 on error.
///
/// # Safety
/// `key` points at `klen` readable bytes; `out` and `len` are writable.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_get_borrow(
    h: *mut Handle,
    key: *const u8,
    klen: usize,
    out: *mut *const u8,
    len: *mut usize,
) -> i32 {
    let key = std::slice::from_raw_parts(key, klen);
    let mut got: Option<(*const u8, usize)> = None;
    match (*h).db.read_all(key, |v| {
        if got.is_none() {
            got = Some((v.as_ptr(), v.len()));
        }
    }) {
        Ok(_) => match got {
            Some((p, n)) => {
                *out = p;
                *len = n;
                1
            }
            None => 0,
        },
        Err(e) => fail(e.to_string(), -1),
    }
}

/// The first value of `key`, copied into `buf` (at most `cap` bytes; `*len`
/// is the value's whole length). Returns 1, 0 or -1 as `get_borrow`.
///
/// # Safety
/// `key` points at `klen` readable bytes; `buf` at `cap` writable ones.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_get_copy(
    h: *mut Handle,
    key: *const u8,
    klen: usize,
    buf: *mut u8,
    cap: usize,
    len: *mut usize,
) -> i32 {
    let key = std::slice::from_raw_parts(key, klen);
    let mut got = false;
    match (*h).db.read_all(key, |v| {
        if !got {
            got = true;
            let n = v.len().min(cap);
            std::ptr::copy_nonoverlapping(v.as_ptr(), buf, n);
            *len = v.len();
        }
    }) {
        Ok(_) => i32::from(got),
        Err(e) => fail(e.to_string(), -1),
    }
}

/// Every value of `key`, each lent to `cb` for the duration of the call --
/// the read as the engine offers it. Returns the count, or -1.
///
/// # Safety
/// `key` points at `klen` readable bytes; `cb` is a valid function.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_read_all(
    h: *mut Handle,
    key: *const u8,
    klen: usize,
    cb: extern "C" fn(*mut c_void, *const u8, usize),
    ctx: *mut c_void,
) -> i64 {
    let key = std::slice::from_raw_parts(key, klen);
    match (*h).db.read_all(key, |v| cb(ctx, v.as_ptr(), v.len())) {
        Ok(n) => n as i64,
        Err(e) => fail(e.to_string(), -1),
    }
}

/// xorshift64*, the suite's generator, so a C or Python caller draws the
/// keys the suite draws.
#[inline]
fn xs(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// The suite's read pass, run inside the library with no boundary crossed
/// per read: `n` uniform keys below `keyspace` from `seed`, sixteen decimal
/// digits each, through `read_all` counting value bytes as the suite does.
/// With `touch` nonzero each value's first byte is read as well. Returns
/// elapsed nanoseconds; `*bytes` gets the byte count.
///
/// # Safety
/// `h` is live; `bytes` is writable.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_bench_native(
    h: *mut Handle,
    n: u64,
    keyspace: u64,
    seed: u64,
    touch: i32,
    bytes: *mut u64,
) -> u64 {
    let db = &(*h).db;
    let mut s = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
    let mut kb = [0u8; 16];
    let mut total = 0u64;
    let mut sum = 0u64;
    let t = Instant::now();
    if touch != 0 {
        for _ in 0..n {
            let mut v = xs(&mut s) % keyspace.max(1);
            for i in (0..16).rev() {
                kb[i] = b'0' + (v % 10) as u8;
                v /= 10;
            }
            if db
                .read_all(&kb, |v| {
                    total += v.len() as u64;
                    sum += v.first().copied().unwrap_or(0) as u64;
                })
                .is_err()
            {
                break;
            }
        }
    } else {
        for _ in 0..n {
            let mut v = xs(&mut s) % keyspace.max(1);
            for i in (0..16).rev() {
                kb[i] = b'0' + (v % 10) as u8;
                v /= 10;
            }
            if db.read_all(&kb, |v| total += v.len() as u64).is_err() {
                break;
            }
        }
    }
    let ns = t.elapsed().as_nanos() as u64;
    *bytes = total;
    std::hint::black_box(sum);
    ns
}

/// The same pass over a caller's table of `n` sixteen-byte keys, so the
/// loop draws no keys: what a read costs with nothing else in the loop.
///
/// # Safety
/// `keys` points at `n * 16` readable bytes; `bytes` is writable.
#[no_mangle]
pub unsafe extern "C" fn supdb_capi_bench_native_keys(
    h: *mut Handle,
    keys: *const u8,
    n: u64,
    bytes: *mut u64,
) -> u64 {
    let db = &(*h).db;
    let table = std::slice::from_raw_parts(keys, (n * 16) as usize);
    let mut total = 0u64;
    let t = Instant::now();
    for k in table.chunks_exact(16) {
        if db.read_all(k, |v| total += v.len() as u64).is_err() {
            break;
        }
    }
    let ns = t.elapsed().as_nanos() as u64;
    *bytes = total;
    ns
}
