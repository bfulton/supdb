//! A probe's structure, never the suite's: an in-memory table of the
//! suite's records written in place, to be measured beside FASTER in one
//! process (`examples/point_ops.rs`, arm `proto`) before anything of it goes
//! into the engine.
//!
//! One flat array of 128-byte slots, each the record itself: a sequence
//! word, the 16-byte key and the 100-byte value. A key's slot is the top
//! bits of a folded multiply of its two words, and a collision walks to the
//! next slot. So a cold get takes one line's miss where FASTER takes two,
//! its bucket and then its record, and the next line of the slot is the
//! spatial prefetcher's.
//!
//! One writer, which overwrites a value in place: the sequence word odd
//! while it copies and moved on by two after, so a reader on another thread
//! copies the value between two equal, even readings of it -- FASTER's
//! `GenLock` pattern, single-writer, so no compare-and-swap. A key is
//! written once, before its slot's word leaves zero, and never moves. No
//! log, no checkpoint, no growth past the capacity it was opened with:
//! what the structure costs an operation is the question, and the promise
//! is FASTER's between two checkpoints.
use std::sync::atomic::{fence, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::Arc;

use crate::engines::{Engine, Features, ReaderOpener, Res, ThreadReader};

/// The value the slots hold, the suite's.
pub const VALUE: usize = 100;

#[repr(C, align(64))]
struct Slot {
    /// 0: empty. Even: at rest. Odd: the writer is copying a value in.
    seq: AtomicU64,
    key: [u64; 2],
    val: [u8; VALUE],
    _pad: [u8; 4],
}

const _: () = assert!(std::mem::size_of::<Slot>() == 128);

struct Table {
    slots: *mut Slot,
    cap: usize,
    shift: u32,
    len: AtomicUsize,
}

// The slots are shared by one writer and the readers under the sequence
// word's protocol.
unsafe impl Send for Table {}
unsafe impl Sync for Table {}

impl Drop for Table {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.slots as *mut u8, Table::layout(self.cap)) }
    }
}

#[inline(always)]
fn words(k: &[u8]) -> (u64, u64) {
    let a = u64::from_le_bytes(k[..8].try_into().unwrap());
    let b = u64::from_le_bytes(k[8..16].try_into().unwrap());
    (a, b)
}

/// A folded multiply of the key's two words.
#[inline(always)]
fn hash(a: u64, b: u64) -> u64 {
    let m = ((a ^ 0x9E37_79B9_7F4A_7C15) as u128) * ((b ^ 0xD6E8_FEB8_6659_FD93) as u128);
    (m as u64) ^ ((m >> 64) as u64)
}

impl Table {
    fn layout(cap: usize) -> std::alloc::Layout {
        std::alloc::Layout::from_size_align(cap * 128, 64).unwrap()
    }

    fn new(cap: usize) -> Table {
        let cap = cap.next_power_of_two().max(1024);
        let slots = unsafe { std::alloc::alloc_zeroed(Table::layout(cap)) } as *mut Slot;
        assert!(!slots.is_null());
        Table {
            slots,
            cap,
            shift: 64 - cap.trailing_zeros(),
            len: AtomicUsize::new(0),
        }
    }

    /// The key's slot, or the empty slot that ends its walk.
    #[inline(always)]
    fn walk(&self, a: u64, b: u64) -> (&Slot, bool) {
        let mask = self.cap - 1;
        let mut i = (hash(a, b) >> self.shift) as usize;
        loop {
            let s = unsafe { &*self.slots.add(i) };
            if s.seq.load(Acquire) == 0 {
                return (s, false);
            }
            if s.key == [a, b] {
                return (s, true);
            }
            i = (i + 1) & mask;
        }
    }

    /// The value between two equal, even readings of the sequence word.
    #[inline(always)]
    fn read(&self, k: &[u8], out: &mut [u8; VALUE]) -> bool {
        let (a, b) = words(k);
        let (s, found) = self.walk(a, b);
        if !found {
            return false;
        }
        loop {
            let q1 = s.seq.load(Acquire);
            if q1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            unsafe { std::ptr::copy_nonoverlapping(s.val.as_ptr(), out.as_mut_ptr(), VALUE) };
            fence(Acquire);
            if s.seq.load(Relaxed) == q1 {
                return true;
            }
        }
    }

    /// The writer's upsert: in place where the key is, else into the empty
    /// slot its walk ended at, key and value before the word leaves zero.
    #[inline(always)]
    fn upsert(&self, k: &[u8], v: &[u8]) -> Res<()> {
        let (a, b) = words(k);
        let (s, found) = self.walk(a, b);
        let s = s as *const Slot as *mut Slot;
        unsafe {
            if found {
                let q = (*s).seq.load(Relaxed);
                (*s).seq.store(q + 1, Relaxed);
                fence(Release);
                std::ptr::copy_nonoverlapping(v.as_ptr(), (*s).val.as_mut_ptr(), VALUE);
                (*s).seq.store(q + 2, Release);
            } else {
                if (self.len.load(Relaxed) + 1) * 8 > self.cap * 7 {
                    return Err("the probe's table is full".to_string());
                }
                (*s).key = [a, b];
                std::ptr::copy_nonoverlapping(v.as_ptr(), (*s).val.as_mut_ptr(), VALUE);
                (*s).seq.store(2, Release);
                self.len.fetch_add(1, Relaxed);
            }
        }
        Ok(())
    }

    /// Each key's distance from its home slot, walked over the table: how
    /// many slots a get of it reads.
    fn displacement(&self) -> (f64, usize) {
        let mask = self.cap - 1;
        let (mut sum, mut max, mut n) = (0usize, 0usize, 0usize);
        for i in 0..self.cap {
            let s = unsafe { &*self.slots.add(i) };
            if s.seq.load(Acquire) == 0 {
                continue;
            }
            let home = (hash(s.key[0], s.key[1]) >> self.shift) as usize;
            let d = (i.wrapping_sub(home)) & mask;
            sum += d;
            max = max.max(d);
            n += 1;
        }
        (sum as f64 / n.max(1) as f64, max)
    }
}

thread_local! {
    /// Where a get's value lands, as FASTER's arm's does: the caller's copy.
    static OUT: std::cell::RefCell<[u8; VALUE]> = const { std::cell::RefCell::new([0u8; VALUE]) };
}

#[inline(always)]
fn get(t: &Table, key: &[u8]) -> Res<usize> {
    if key.len() != 16 {
        return Err(format!("the probe takes 16-byte keys, not {}", key.len()));
    }
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        if t.read(key, &mut o) {
            std::hint::black_box(o.as_slice());
            Ok(VALUE)
        } else {
            Ok(0)
        }
    })
}

/// The table as an engine of the suite's, so a probe drives it as it drives
/// FASTER.
pub struct Hot {
    t: Arc<Table>,
}

impl Hot {
    /// A table for `keys` keys: twice as many slots, rounded up to a power
    /// of two, or `PROTO_CAP_X` times as many where that is set.
    pub fn create(keys: u64) -> Hot {
        let x: f64 = std::env::var("PROTO_CAP_X")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2.0);
        Hot {
            t: Arc::new(Table::new((keys as f64 * x) as usize)),
        }
    }
}

struct HotReader {
    t: Arc<Table>,
}

impl ThreadReader for HotReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        get(&self.t, key)
    }
    fn range(&mut self, _from: &[u8], _n: usize) -> Res<usize> {
        Err("the probe's table has no ordered scan".to_string())
    }
}

impl Engine for Hot {
    fn name(&self) -> &'static str {
        "proto"
    }
    fn features(&self) -> Features {
        Features {
            durable_commit: false,
            transactions: false,
            checksums: false,
            reopen_for_write: false,
            read_your_writes: true,
            ordered_scan: false,
        }
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        for &(k, v) in items {
            if k.len() != 16 || v.len() != VALUE {
                return Err(format!(
                    "the probe takes 16-byte keys and {VALUE}-byte values, not {} and {}",
                    k.len(),
                    v.len()
                ));
            }
            self.t.upsert(k, v)?;
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        get(&self.t, key)
    }
    fn range(&mut self, _from: &[u8], _n: usize) -> Res<usize> {
        Err("the probe's table has no ordered scan".to_string())
    }
    fn sync(&mut self) -> Res<()> {
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        (self.t.cap * 128) as u64
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        let t = self.t.clone();
        Ok(Box::new(move || {
            Ok(Box::new(HotReader { t }) as Box<dyn ThreadReader>)
        }))
    }
    fn counters(&self) -> Vec<(&'static str, f64)> {
        let (mean, max) = self.t.displacement();
        vec![
            ("proto_keys", self.t.len.load(Relaxed) as f64),
            ("proto_slots", self.t.cap as f64),
            ("proto_displacement_mean", mean),
            ("proto_displacement_max", max as f64),
        ]
    }
}

/// FASTER's layout with the inline table's code: an index of 8-byte words,
/// each a 16-bit tag and a record's address, over records packed in an
/// arena at their own lengths -- a 16-byte header (the sequence word, the
/// key's length and the value's), the key, the value, rounded to eight. A
/// cold get takes the slot's line and then the record's, as FASTER's does,
/// and nothing in it depends on the record's size: the key is compared at
/// its length and the value copied at its own. An overwrite of the same
/// length is in place under the record's sequence word; any other is a new
/// record at the arena's end and a store of the slot. So the inline
/// table's lead over FASTER is the slot's layout only as far as this one
/// gives it back.
struct Packed {
    slots: Box<[AtomicU64]>,
    shift: u32,
    arena: *mut u8,
    arena_cap: usize,
    tail: AtomicUsize,
    len: AtomicUsize,
}

unsafe impl Send for Packed {}
unsafe impl Sync for Packed {}

impl Drop for Packed {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(
                self.arena,
                std::alloc::Layout::from_size_align(self.arena_cap, 64).unwrap(),
            )
        }
    }
}

const ADDR: u64 = (1 << 48) - 1;
const HDR: usize = 16;

#[repr(C)]
struct RecHdr {
    seq: AtomicU64,
    klen: u32,
    vlen: u32,
}

impl Packed {
    fn new(keys: usize, x: f64) -> Packed {
        let cap = ((keys as f64 * x) as usize).next_power_of_two().max(1024);
        // Room for every key's record twice over: the probe's loads and
        // inserts, and an overwrite of another length, never fill it.
        let arena_cap = (keys.max(1024) * (HDR + 16 + VALUE + 8) * 2).next_multiple_of(4096);
        let arena = unsafe {
            std::alloc::alloc_zeroed(std::alloc::Layout::from_size_align(arena_cap, 64).unwrap())
        };
        assert!(!arena.is_null());
        Packed {
            slots: (0..cap).map(|_| AtomicU64::new(0)).collect(),
            shift: 64 - cap.trailing_zeros(),
            arena,
            arena_cap,
            tail: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn rec(&self, at: u64) -> *mut RecHdr {
        unsafe { self.arena.add((at - 1) as usize) as *mut RecHdr }
    }

    /// The key's record and its slot, or the empty slot its walk ended at.
    #[inline(always)]
    fn walk(&self, key: &[u8]) -> (usize, Option<*mut RecHdr>, u64) {
        let (a, b) = if key.len() >= 16 {
            words(key)
        } else {
            let mut kb = [0u8; 16];
            kb[..key.len()].copy_from_slice(key);
            words(&kb)
        };
        let h = hash(a ^ key.len() as u64, b);
        let tag = (h & 0xffff) << 48;
        let mask = self.slots.len() - 1;
        let mut i = (h >> self.shift) as usize;
        loop {
            let s = self.slots[i].load(Acquire);
            if s == 0 {
                return (i, None, tag);
            }
            if s & !ADDR == tag {
                let r = self.rec(s & ADDR);
                let k = unsafe {
                    std::slice::from_raw_parts((r as *const u8).add(HDR), (*r).klen as usize)
                };
                if k == key {
                    return (i, Some(r), tag);
                }
            }
            i = (i + 1) & mask;
        }
    }

    #[inline(always)]
    fn read(&self, key: &[u8], out: &mut [u8; VALUE]) -> Option<usize> {
        let (_, r, _) = self.walk(key);
        let r = r?;
        unsafe {
            loop {
                let q1 = (*r).seq.load(Acquire);
                if q1 & 1 == 1 {
                    std::hint::spin_loop();
                    continue;
                }
                let (klen, vlen) = ((*r).klen as usize, (*r).vlen as usize);
                let n = vlen.min(VALUE);
                std::ptr::copy_nonoverlapping(
                    (r as *const u8).add(HDR + klen),
                    out.as_mut_ptr(),
                    n,
                );
                fence(Acquire);
                if (*r).seq.load(Relaxed) == q1 {
                    return Some(n);
                }
            }
        }
    }

    fn append(&self, key: &[u8], v: &[u8]) -> Res<u64> {
        let need = (HDR + key.len() + v.len()).next_multiple_of(8);
        let at = self.tail.load(Relaxed);
        if at + need > self.arena_cap {
            return Err("the probe's arena is full".to_string());
        }
        unsafe {
            let r = self.arena.add(at) as *mut RecHdr;
            (*r).klen = key.len() as u32;
            (*r).vlen = v.len() as u32;
            let p = (r as *mut u8).add(HDR);
            std::ptr::copy_nonoverlapping(key.as_ptr(), p, key.len());
            std::ptr::copy_nonoverlapping(v.as_ptr(), p.add(key.len()), v.len());
            (*r).seq.store(2, Relaxed);
        }
        self.tail.store(at + need, Relaxed);
        Ok(at as u64 + 1)
    }

    fn upsert(&self, key: &[u8], v: &[u8]) -> Res<()> {
        let (i, r, tag) = self.walk(key);
        match r {
            Some(r) if unsafe { (*r).vlen } as usize == v.len() => unsafe {
                let q = (*r).seq.load(Relaxed);
                (*r).seq.store(q + 1, Relaxed);
                fence(Release);
                let p = (r as *mut u8).add(HDR + key.len());
                std::ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
                (*r).seq.store(q + 2, Release);
            },
            Some(_) => {
                let at = self.append(key, v)?;
                self.slots[i].store(tag | at, Release);
            }
            None => {
                if (self.len.load(Relaxed) + 1) * 8 > self.slots.len() * 7 {
                    return Err("the probe's index is full".to_string());
                }
                let at = self.append(key, v)?;
                self.slots[i].store(tag | at, Release);
                self.len.fetch_add(1, Relaxed);
            }
        }
        Ok(())
    }
}

#[inline(always)]
fn packed_get(t: &Packed, key: &[u8]) -> Res<usize> {
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        match t.read(key, &mut o) {
            Some(n) => {
                std::hint::black_box(o.as_slice());
                Ok(n)
            }
            None => Ok(0),
        }
    })
}

/// The packed layout as an engine of the suite's (`point_ops proto-log`).
pub struct Logged {
    t: Arc<Packed>,
}

impl Logged {
    /// An index of twice as many slots as `keys`, rounded up to a power of
    /// two, or `PROTO_CAP_X` times as many where that is set.
    pub fn create(keys: u64) -> Logged {
        let x: f64 = std::env::var("PROTO_CAP_X")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2.0);
        Logged {
            t: Arc::new(Packed::new(keys as usize, x)),
        }
    }
}

struct LoggedReader {
    t: Arc<Packed>,
}

impl ThreadReader for LoggedReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        packed_get(&self.t, key)
    }
    fn range(&mut self, _from: &[u8], _n: usize) -> Res<usize> {
        Err("the probe's table has no ordered scan".to_string())
    }
}

impl Engine for Logged {
    fn name(&self) -> &'static str {
        "proto-log"
    }
    fn features(&self) -> Features {
        Hot::create(0).features()
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        for &(k, v) in items {
            self.t.upsert(k, v)?;
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        packed_get(&self.t, key)
    }
    fn range(&mut self, _from: &[u8], _n: usize) -> Res<usize> {
        Err("the probe's table has no ordered scan".to_string())
    }
    fn sync(&mut self) -> Res<()> {
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        (self.t.slots.len() * 8 + self.t.tail.load(Relaxed)) as u64
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        let t = self.t.clone();
        Ok(Box::new(move || {
            Ok(Box::new(LoggedReader { t }) as Box<dyn ThreadReader>)
        }))
    }
    fn counters(&self) -> Vec<(&'static str, f64)> {
        vec![
            ("proto_keys", self.t.len.load(Relaxed) as f64),
            ("proto_slots", self.t.slots.len() as f64),
            ("proto_arena_bytes", self.t.tail.load(Relaxed) as f64),
        ]
    }
}

/// The engine's live table in the shape it keeps, written by every writer
/// thread at once with no lock, over a read-only base that stands for the
/// partitions a load leaves (`point_ops` and `bench ab` arm `proto-shared`):
/// what a shared live table would cost the threaded writers' mixes before
/// any of it goes into the engine.
///
/// The live table is the engine's hashed memtable: an index of words, each
/// the high half of the key's hash over an entry's number, at load half at
/// most and never grown; entries of twenty-four bytes, the chain's head, the
/// hash, the key's place and length and a version word; the keys in an arena
/// and the values in chunks of the engine's format, a put a tombstone and
/// then the value, each chunk naming the one before it. A get probes the
/// index, compares the key, and copies the head's value between two equal,
/// even readings of the entry's version, and a key the table does not hold
/// is read from the base. A put of a key the table holds takes the version
/// odd by a compare-and-swap, writes the value over the head's where the
/// head is a put's value of its length (`Options::put_in_place`'s rule) and
/// lays a tombstone and the value at the arena's end otherwise, and moves
/// the version on. A put of a key it does not hold claims an entry, lays the
/// key and both chunks down, and publishes the entry by a compare-and-swap
/// of the vacant slot its probe ended at; a writer that loses the slot to
/// the same key takes the winner's entry, and its own is left unused. Each
/// writer claims the arena and the entries in strides of its own, so the
/// one shared word a put of a new key touches is the slot. No freeze, no
/// log, no growth: a table that fills refuses the put.
pub struct Shared {
    live: Arc<Live>,
    base: Arc<Packed>,
    /// Writes go to the base until the load's sync, and to the live table
    /// after it.
    loaded: bool,
    local: Local,
}

/// A chunk's length word for a tombstone, the engine's `TOMB_LEN`.
const TOMB: u32 = u32::MAX;
/// A chunk's header, the previous chunk's offset and the value's length:
/// the engine's `CHUNK_HDR`.
const CHDR: usize = 12;
/// The arena a writer claims at a time.
const ARENA_STRIDE: usize = 64 << 10;
/// The entries a writer claims at a time.
const ENTRY_STRIDE: usize = 64;
const NO_CHUNK: u64 = u64::MAX;

#[repr(C)]
struct SEntry {
    /// Offset+1 of the newest chunk; written under the version.
    head: AtomicU64,
    hash: u32,
    key_off: u32,
    /// Even at rest, odd while a writer holds the entry.
    ver: std::sync::atomic::AtomicU32,
    klen: u16,
}

const _: () = assert!(std::mem::size_of::<SEntry>() == 24);

struct Live {
    slots: *mut AtomicU64,
    mask: usize,
    entries: *mut SEntry,
    entry_cap: usize,
    next_entry: AtomicUsize,
    arena: *mut u8,
    arena_cap: usize,
    tail: AtomicUsize,
    inserts: AtomicU64,
    inplace: AtomicU64,
    appends: AtomicU64,
    races: AtomicU64,
}

// Shared by the writers and the readers under the slots' publication and
// the entries' versions.
unsafe impl Send for Live {}
unsafe impl Sync for Live {}

fn zeroed<T>(n: usize) -> *mut T {
    let layout = std::alloc::Layout::array::<T>(n.max(1))
        .unwrap()
        .align_to(64)
        .unwrap();
    let p = unsafe { std::alloc::alloc_zeroed(layout) } as *mut T;
    assert!(!p.is_null(), "the probe's table: out of memory");
    p
}

unsafe fn unzeroed<T>(p: *mut T, n: usize) {
    let layout = std::alloc::Layout::array::<T>(n.max(1))
        .unwrap()
        .align_to(64)
        .unwrap();
    unsafe { std::alloc::dealloc(p as *mut u8, layout) }
}

impl Drop for Live {
    fn drop(&mut self) {
        unsafe {
            unzeroed(self.slots, self.mask + 1);
            unzeroed(self.entries, self.entry_cap);
            unzeroed(self.arena, self.arena_cap);
        }
    }
}

/// A writer's own: its strides of the arena and the entries, an entry it
/// laid down and has not published yet, and its counts.
#[derive(Default)]
struct Local {
    cur: usize,
    end: usize,
    ent: usize,
    ent_end: usize,
    pending: Option<usize>,
    inserts: u64,
    inplace: u64,
    appends: u64,
    races: u64,
}

/// The live table's hash of a key: the engine's fold of a sixteen-byte key
/// (`fold_hash16`), FNV-1a for any other.
#[inline(always)]
fn live_hash(key: &[u8]) -> u64 {
    if key.len() == 16 {
        let (a, b) = words(key);
        let m = ((a ^ 0x9E37_79B9_7F4A_7C15) as u128) * ((b ^ 0xD6E8_FEB8_6659_FD93) as u128);
        return (m as u64) ^ ((m >> 64) as u64);
    }
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &c in key {
        h = (h ^ c as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

impl Live {
    fn new(keys: usize) -> Live {
        let entry_cap = keys + keys / 4 + 64 * ENTRY_STRIDE;
        let slots = (entry_cap * 2).next_power_of_two();
        let arena_cap =
            (entry_cap * (16 + 2 * CHDR + VALUE + 8) + 64 * ARENA_STRIDE).next_multiple_of(4096);
        assert!(
            arena_cap < u32::MAX as usize,
            "the probe's arena is addressed in 32 bits"
        );
        Live {
            slots: zeroed(slots),
            mask: slots - 1,
            entries: zeroed(entry_cap),
            entry_cap,
            next_entry: AtomicUsize::new(0),
            arena: zeroed(arena_cap),
            arena_cap,
            tail: AtomicUsize::new(0),
            inserts: AtomicU64::new(0),
            inplace: AtomicU64::new(0),
            appends: AtomicU64::new(0),
            races: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    fn slot(&self, i: usize) -> &AtomicU64 {
        unsafe { &*self.slots.add(i) }
    }

    #[inline(always)]
    fn entry(&self, n: usize) -> &SEntry {
        unsafe { &*self.entries.add(n) }
    }

    #[inline(always)]
    fn rd_u64(&self, off: usize) -> u64 {
        unsafe { std::ptr::read_unaligned(self.arena.add(off) as *const u64) }
    }

    #[inline(always)]
    fn rd_u32(&self, off: usize) -> u32 {
        unsafe { std::ptr::read_unaligned(self.arena.add(off) as *const u32) }
    }

    /// Writer: `bytes` at `off`, a range only this writer has claimed or
    /// holds the version of.
    #[inline(always)]
    fn wr(&self, off: usize, bytes: &[u8]) {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.arena.add(off), bytes.len()) }
    }

    #[inline(always)]
    fn key_of(&self, e: &SEntry) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.arena.add(e.key_off as usize), e.klen as usize) }
    }

    fn reserve(&self, l: &mut Local, n: usize) -> Res<usize> {
        if l.cur + n > l.end {
            let stride = ARENA_STRIDE.max(n);
            let at = self.tail.fetch_add(stride, Relaxed);
            if at + stride > self.arena_cap {
                return Err("the probe's live arena is full".to_string());
            }
            l.cur = at;
            l.end = at + stride;
        }
        let off = l.cur;
        l.cur += n;
        Ok(off)
    }

    fn claim(&self, l: &mut Local) -> Res<usize> {
        if l.ent == l.ent_end {
            let at = self.next_entry.fetch_add(ENTRY_STRIDE, Relaxed);
            if at + ENTRY_STRIDE > self.entry_cap {
                return Err("the probe's live entries are full".to_string());
            }
            l.ent = at;
            l.ent_end = at + ENTRY_STRIDE;
        }
        let n = l.ent;
        l.ent += 1;
        Ok(n)
    }

    /// The key's entry, or the vacant slot its probe ended at.
    #[inline(always)]
    fn find(&self, hash: u64, key: &[u8]) -> Result<usize, usize> {
        let tag = hash >> 32;
        let mut i = tag as usize & self.mask;
        loop {
            let w = self.slot(i).load(Acquire);
            if w == 0 {
                return Err(i);
            }
            if w >> 32 == tag {
                let n = (w & 0xffff_ffff) as usize - 1;
                let e = self.entry(n);
                if e.klen as usize == key.len() && self.key_of(e) == key {
                    return Ok(n);
                }
            }
            i = (i + 1) & self.mask;
        }
    }

    /// The key's value copied out between two equal, even readings of its
    /// entry's version: `Some(len)`, `Some(0)` for a tombstone, `None`
    /// where the table holds nothing of the key.
    #[inline(always)]
    fn read(&self, hash: u64, key: &[u8], out: &mut [u8; VALUE]) -> Option<usize> {
        let n = self.find(hash, key).ok()?;
        let e = self.entry(n);
        loop {
            let v1 = e.ver.load(Acquire);
            if v1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let head = (e.head.load(Acquire) - 1) as usize;
            let len = self.rd_u32(head + 8);
            let got = if len == TOMB {
                0
            } else {
                let len = (len as usize).min(VALUE);
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        self.arena.add(head + CHDR),
                        out.as_mut_ptr(),
                        len,
                    )
                };
                len
            };
            fence(Acquire);
            if e.ver.load(Relaxed) == v1 {
                return Some(got);
            }
        }
    }

    /// Writer: the entry's version taken odd, spinning while another
    /// writer holds it; what it was, for `unlock`.
    #[inline(always)]
    fn lock(&self, e: &SEntry) -> u32 {
        let v = loop {
            let v = e.ver.load(Relaxed);
            if v & 1 == 0
                && e.ver
                    .compare_exchange_weak(v, v + 1, Acquire, Relaxed)
                    .is_ok()
            {
                break v;
            }
            std::hint::spin_loop();
        };
        // The odd version before any byte the hold writes.
        fence(Release);
        v
    }

    #[inline(always)]
    fn unlock(&self, e: &SEntry, v: u32) {
        e.ver.store(v + 2, Release);
    }

    /// Writer, holding the entry's version: `value` over the head's where
    /// the head is a put's value of its length, a tombstone and `value`
    /// laid down at the arena's end and made the head otherwise.
    fn write_head(&self, l: &mut Local, e: &SEntry, head: usize, value: &[u8]) -> Res<()> {
        let prev = self.rd_u64(head);
        if self.rd_u32(head + 8) as usize == value.len()
            && head >= CHDR
            && prev == (head - CHDR) as u64
            && self.rd_u32(prev as usize + 8) == TOMB
        {
            self.wr(head + CHDR, value);
            l.inplace += 1;
            return Ok(());
        }
        let off = self.reserve(l, 2 * CHDR + value.len())?;
        self.wr(off, &(head as u64).to_le_bytes());
        self.wr(off + 8, &TOMB.to_le_bytes());
        self.wr(off + CHDR, &(off as u64).to_le_bytes());
        self.wr(off + CHDR + 8, &(value.len() as u32).to_le_bytes());
        self.wr(off + 2 * CHDR, value);
        e.head.store((off + CHDR) as u64 + 1, Release);
        l.appends += 1;
        Ok(())
    }

    /// A put of a key the table holds, under the entry's version.
    fn update(&self, l: &mut Local, n: usize, value: &[u8]) -> Res<()> {
        let e = self.entry(n);
        let v = self.lock(e);
        let head = (e.head.load(Relaxed) - 1) as usize;
        let out = self.write_head(l, e, head, value);
        self.unlock(e, v);
        out
    }

    /// A put from writer `l`: an update where the table holds the key, and
    /// a new entry published at the probe's vacant slot otherwise.
    fn put(&self, l: &mut Local, key: &[u8], value: &[u8]) -> Res<()> {
        if key.len() > u16::MAX as usize {
            return Err("the probe's keys are at most 64 KiB".to_string());
        }
        let hash = live_hash(key);
        loop {
            match self.find(hash, key) {
                Ok(n) => {
                    // An entry laid down for a slot another writer's put
                    // of the same key took is left unused.
                    l.pending = None;
                    return self.update(l, n, value);
                }
                Err(i) => {
                    let n = match l.pending {
                        Some(n) => n,
                        None => {
                            let n = self.claim(l)?;
                            let off = self.reserve(l, key.len() + 2 * CHDR + value.len())?;
                            let tomb = off + key.len();
                            self.wr(off, key);
                            self.wr(tomb, &NO_CHUNK.to_le_bytes());
                            self.wr(tomb + 8, &TOMB.to_le_bytes());
                            self.wr(tomb + CHDR, &(tomb as u64).to_le_bytes());
                            self.wr(tomb + CHDR + 8, &(value.len() as u32).to_le_bytes());
                            self.wr(tomb + 2 * CHDR, value);
                            let e = unsafe { &mut *self.entries.add(n) };
                            e.hash = (hash >> 32) as u32;
                            e.key_off = off as u32;
                            e.klen = key.len() as u16;
                            *e.ver.get_mut() = 0;
                            *e.head.get_mut() = (tomb + CHDR) as u64 + 1;
                            n
                        }
                    };
                    let word = (hash >> 32) << 32 | (n as u64 + 1);
                    if self
                        .slot(i)
                        .compare_exchange(0, word, Release, Acquire)
                        .is_ok()
                    {
                        l.pending = None;
                        l.inserts += 1;
                        return Ok(());
                    }
                    l.pending = Some(n);
                    l.races += 1;
                }
            }
        }
    }

    /// A read-modify-write from writer `l`, as one operation: the key's
    /// value copied into `out` and `value` written in its place under one
    /// hold of the entry's version, one probe for both. A key the table
    /// does not hold is read from the base and then put.
    fn rmw(
        &self,
        l: &mut Local,
        base: &Packed,
        key: &[u8],
        value: &[u8],
        out: &mut [u8; VALUE],
    ) -> Res<usize> {
        let hash = live_hash(key);
        let Ok(n) = self.find(hash, key) else {
            let got = base.read(key, out).unwrap_or(0);
            self.put(l, key, value)?;
            return Ok(got);
        };
        let e = self.entry(n);
        let v = self.lock(e);
        let head = (e.head.load(Relaxed) - 1) as usize;
        let len = self.rd_u32(head + 8);
        let got = if len == TOMB {
            0
        } else {
            let len = (len as usize).min(VALUE);
            unsafe {
                std::ptr::copy_nonoverlapping(self.arena.add(head + CHDR), out.as_mut_ptr(), len)
            };
            len
        };
        let done = self.write_head(l, e, head, value);
        self.unlock(e, v);
        done.map(|()| got)
    }

    fn fold(&self, l: &mut Local) {
        self.inserts
            .fetch_add(std::mem::take(&mut l.inserts), Relaxed);
        self.inplace
            .fetch_add(std::mem::take(&mut l.inplace), Relaxed);
        self.appends
            .fetch_add(std::mem::take(&mut l.appends), Relaxed);
        self.races.fetch_add(std::mem::take(&mut l.races), Relaxed);
    }
}

/// A get through the live table and then the base.
#[inline(always)]
fn shared_get(live: &Live, base: &Packed, key: &[u8]) -> Res<usize> {
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        let n = match live.read(live_hash(key), key, &mut o) {
            Some(n) => n,
            None => base.read(key, &mut o).unwrap_or(0),
        };
        std::hint::black_box(o.as_slice());
        Ok(n)
    })
}

impl Shared {
    /// A base of `keys` keys in the packed layout, and a live table with
    /// room for every one of them and a quarter more.
    pub fn create(keys: u64) -> Shared {
        let keys = keys.max(1024) as usize;
        Shared {
            live: Arc::new(Live::new(keys)),
            base: Arc::new(Packed::new(keys, 2.0)),
            loaded: false,
            local: Local::default(),
        }
    }
}

struct SharedReader {
    live: Arc<Live>,
    base: Arc<Packed>,
}

impl ThreadReader for SharedReader {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        shared_get(&self.live, &self.base, key)
    }
    fn range(&mut self, _from: &[u8], _n: usize) -> Res<usize> {
        Err("the probe's table has no ordered scan".to_string())
    }
}

/// A writer thread's: its own strides and counts, folded into the table's
/// at its drop.
struct SharedWriter {
    live: Arc<Live>,
    base: Arc<Packed>,
    local: Local,
}

impl Drop for SharedWriter {
    fn drop(&mut self) {
        self.live.fold(&mut self.local);
    }
}

impl crate::engines::ThreadWriter for SharedWriter {
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        shared_get(&self.live, &self.base, key)
    }
    fn update_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        for &(k, v) in items {
            self.live.put(&mut self.local, k, v)?;
        }
        Ok(())
    }
    fn rmw(&mut self, key: &[u8], value: &[u8]) -> Option<Res<usize>> {
        // Read once: a read of the environment takes the process's lock
        // over it, and every thread asking at every operation is a mix of
        // that lock's.
        static NO_RMW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *NO_RMW.get_or_init(|| std::env::var_os("PROTO_NO_RMW").is_some()) {
            return None;
        }
        let (live, base, local) = (&self.live, &self.base, &mut self.local);
        Some(OUT.with(|o| live.rmw(local, base, key, value, &mut o.borrow_mut())))
    }
}

impl Engine for Shared {
    fn name(&self) -> &'static str {
        "proto-shared"
    }
    fn features(&self) -> Features {
        Hot::create(0).features()
    }
    fn write_batch(&mut self, items: &[(&[u8], &[u8])]) -> Res<()> {
        for &(k, v) in items {
            if self.loaded {
                self.live.put(&mut self.local, k, v)?;
            } else {
                self.base.upsert(k, v)?;
            }
        }
        Ok(())
    }
    fn get(&mut self, key: &[u8]) -> Res<usize> {
        shared_get(&self.live, &self.base, key)
    }
    fn range(&mut self, _from: &[u8], _n: usize) -> Res<usize> {
        Err("the probe's table has no ordered scan".to_string())
    }
    fn sync(&mut self) -> Res<()> {
        self.loaded = true;
        Ok(())
    }
    fn size_bytes(&self) -> u64 {
        (self.base.slots.len() * 8
            + self.base.tail.load(Relaxed)
            + (self.live.mask + 1) * 8
            + self.live.next_entry.load(Relaxed) * 24
            + self.live.tail.load(Relaxed)) as u64
    }
    fn thread_reader(&self) -> Res<ReaderOpener> {
        let (live, base) = (self.live.clone(), self.base.clone());
        Ok(Box::new(move || {
            Ok(Box::new(SharedReader { live, base }) as Box<dyn ThreadReader>)
        }))
    }
    fn thread_writers(&mut self, n: usize) -> Res<Vec<crate::engines::WriterOpener<'_>>> {
        Ok((0..n)
            .map(|_| {
                let (live, base) = (self.live.clone(), self.base.clone());
                Box::new(move || {
                    Ok(Box::new(SharedWriter {
                        live,
                        base,
                        local: Local::default(),
                    })
                        as Box<dyn crate::engines::ThreadWriter>)
                }) as crate::engines::WriterOpener<'_>
            })
            .collect())
    }
    fn counters(&self) -> Vec<(&'static str, f64)> {
        let l = &self.local;
        let get = |a: &AtomicU64, own: u64| (a.load(Relaxed) + own) as f64;
        vec![
            ("proto_live_inserts", get(&self.live.inserts, l.inserts)),
            ("proto_live_inplace", get(&self.live.inplace, l.inplace)),
            ("proto_live_appends", get(&self.live.appends, l.appends)),
            ("proto_live_races", get(&self.live.races, l.races)),
            (
                "proto_live_entries",
                self.live.next_entry.load(Relaxed) as f64,
            ),
            (
                "proto_live_arena_bytes",
                self.live.tail.load(Relaxed) as f64,
            ),
            ("proto_base_keys", self.base.len.load(Relaxed) as f64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reads_back_the_value_it_was_last_written_with() {
        let mut h = Hot::create(1000);
        let mut kb = [0u8; 16];
        for i in 0..1500u64 {
            crate::workload::db_key_into(i % 1000, &mut kb);
            let v = [(i % 251) as u8; VALUE];
            h.write_batch(&[(&kb, &v)]).unwrap();
        }
        for i in 0..1000u64 {
            crate::workload::db_key_into(i, &mut kb);
            assert_eq!(h.get(&kb).unwrap(), VALUE);
            let want = if i < 500 { (i + 1000) % 251 } else { i % 251 } as u8;
            OUT.with(|o| assert_eq!(o.borrow()[..], [want; VALUE][..]));
        }
        crate::workload::db_key_into(5000, &mut kb);
        assert_eq!(h.get(&kb).unwrap(), 0);
        assert_eq!(h.t.len.load(Relaxed), 1000);
    }

    #[test]
    fn the_packed_layout_reads_back_values_of_any_length() {
        // Keys of eight to sixteen bytes, each distinct at its own length.
        let key = |j: u64| format!("key{j:05}{}", "x".repeat((j % 9) as usize)).into_bytes();
        let mut h = Logged::create(1000);
        for i in 0..1500u64 {
            let len = if i >= 1000 && i % 3 == 0 { 40 } else { VALUE };
            let v = vec![(i % 251) as u8; len];
            h.write_batch(&[(&key(i % 1000), &v)]).unwrap();
        }
        for j in 0..1000u64 {
            let last = if j < 500 { j + 1000 } else { j };
            let len = if last >= 1000 && last % 3 == 0 {
                40
            } else {
                VALUE
            };
            assert_eq!(h.get(&key(j)).unwrap(), len, "key {j}");
            OUT.with(|o| assert_eq!(o.borrow()[..len], vec![(last % 251) as u8; len][..]));
        }
        assert_eq!(h.get(b"nope").unwrap(), 0);
        assert_eq!(h.t.len.load(Relaxed), 1000);
    }

    /// The shared table answers each key with its last write, the base
    /// answers what the table holds nothing of, and each of a put's three
    /// paths -- a new entry, a value written over one of its length, a
    /// tombstone and a value laid down for another length -- is taken
    /// where it should be and counted. A read-modify-write hands back the
    /// value it replaced.
    #[test]
    fn the_shared_table_reads_the_last_write_of_each_key_over_its_base() {
        let mut t = Shared::create(2000);
        let mut kb = [0u8; 16];
        let key = |i: u64, kb: &mut [u8; 16]| crate::workload::db_key_into(i, kb);
        for i in 0..1000u64 {
            key(i, &mut kb);
            t.write_batch(&[(&kb, &[1u8; VALUE])]).unwrap();
        }
        t.sync().unwrap();
        let put = |t: &mut Shared, i: u64, v: &[u8]| {
            let mut kb = [0u8; 16];
            crate::workload::db_key_into(i, &mut kb);
            t.write_batch(&[(&kb, v)]).unwrap();
        };
        for i in 0..500 {
            put(&mut t, i, &[2u8; VALUE]);
        }
        for i in 0..250 {
            put(&mut t, i, &[3u8; VALUE]);
        }
        for i in 250..300 {
            put(&mut t, i, &[4u8; 40]);
        }
        for i in 260..270 {
            put(&mut t, i, &[5u8; VALUE]);
        }
        let want = |i: u64| -> Vec<u8> {
            match i {
                0..250 => vec![3; VALUE],
                260..270 => vec![5; VALUE],
                250..300 => vec![4; 40],
                300..500 => vec![2; VALUE],
                _ => vec![1; VALUE],
            }
        };
        for i in 0..1000u64 {
            key(i, &mut kb);
            let w = want(i);
            assert_eq!(t.get(&kb).unwrap(), w.len(), "key {i}");
            OUT.with(|o| assert_eq!(o.borrow()[..w.len()], w[..], "key {i}"));
        }
        key(5000, &mut kb);
        assert_eq!(t.get(&kb).unwrap(), 0);
        let c: std::collections::HashMap<_, _> = t.counters().into_iter().collect();
        assert_eq!(c["proto_live_inserts"], 500.0);
        assert_eq!(c["proto_live_inplace"], 250.0);
        assert_eq!(
            c["proto_live_appends"], 60.0,
            "the forty-byte puts and the puts after them"
        );
        assert_eq!(c["proto_live_races"], 0.0);
        // A read-modify-write: of a key the table holds, and of one only
        // the base does, each handing back what it replaced.
        let mut ws = t.thread_writers(1).unwrap();
        let mut w = (ws.pop().unwrap())().unwrap();
        for (i, was) in [(0u64, 3u8), (700, 1)] {
            key(i, &mut kb);
            assert_eq!(w.rmw(&kb, &[9u8; VALUE]).unwrap().unwrap(), VALUE);
            OUT.with(|o| assert_eq!(o.borrow()[..], [was; VALUE][..], "key {i}"));
            assert_eq!(w.get(&kb).unwrap(), VALUE);
            OUT.with(|o| assert_eq!(o.borrow()[..], [9u8; VALUE][..], "key {i}"));
        }
    }

    /// Writers on four threads, released together, put and read-modify-
    /// write one set of keys in one order, the first round of each a race
    /// to make the keys' entries, while readers on two threads read them:
    /// every value a reader or an RMW sees is one write's whole -- each
    /// write a run of one byte, so a copy torn between two shows two --
    /// every key has one entry in the index and one insert counted whatever
    /// the races, and each key ends with a value some writer wrote last of
    /// its own. The insert race is a matter of timing, so the store is made
    /// again until one has been run with a race in it, ten at most.
    #[test]
    fn writers_on_threads_make_one_entry_a_key_and_nobody_reads_a_torn_value() {
        let mut raced = false;
        for _ in 0..10 {
            raced |= shared_writers_round() > 0;
            if raced {
                break;
            }
        }
        assert!(raced, "no store of ten raced two writers to a key's entry");
    }

    /// One store of `writers_on_threads_make_one_entry_a_key_...`: its
    /// checks, and the insert races its writers counted.
    fn shared_writers_round() -> u64 {
        const KEYS: u64 = 4096;
        const ROUNDS: u64 = 20;
        const WRITERS: usize = 4;
        let mut t = Shared::create(4 * KEYS);
        let mut kb = [0u8; 16];
        for i in 0..KEYS {
            crate::workload::db_key_into(i, &mut kb);
            t.write_batch(&[(&kb, &[0u8; VALUE])]).unwrap();
        }
        t.sync().unwrap();
        let (live, base) = (t.live.clone(), t.base.clone());
        let stop = std::sync::atomic::AtomicBool::new(false);
        let go = std::sync::Barrier::new(WRITERS);
        let uniform = |v: &[u8]| v.iter().all(|&b| b == v[0]);
        let openers = t.thread_writers(WRITERS).unwrap();
        std::thread::scope(|s| {
            let readers: Vec<_> = (0..2)
                .map(|_| {
                    let (live, base, stop) = (live.clone(), base.clone(), &stop);
                    s.spawn(move || {
                        let mut kb = [0u8; 16];
                        let mut reads = 0u64;
                        while !stop.load(Relaxed) {
                            for i in 0..KEYS {
                                crate::workload::db_key_into(i, &mut kb);
                                assert_eq!(shared_get(&live, &base, &kb).unwrap(), VALUE);
                                OUT.with(|o| assert!(uniform(&o.borrow()[..]), "a torn read"));
                                reads += 1;
                            }
                        }
                        reads
                    })
                })
                .collect();
            let writers: Vec<_> = openers
                .into_iter()
                .enumerate()
                .map(|(w, open)| {
                    let go = &go;
                    s.spawn(move || {
                        let mut wr = open().unwrap();
                        let mut kb = [0u8; 16];
                        go.wait();
                        for round in 0..ROUNDS {
                            let v = [(1 + w as u64 * ROUNDS + round) as u8; VALUE];
                            for i in 0..KEYS {
                                crate::workload::db_key_into(i, &mut kb);
                                if round % 2 == 1 {
                                    assert_eq!(wr.rmw(&kb, &v).unwrap().unwrap(), VALUE);
                                    OUT.with(|o| assert!(uniform(&o.borrow()[..]), "a torn RMW"));
                                } else {
                                    wr.update_batch(&[(&kb, &v)]).unwrap();
                                }
                            }
                        }
                    })
                })
                .collect();
            // The readers stopped before a writer's failure is passed on,
            // so that a failure ends the test rather than leaving them to
            // read for ever.
            let done: Vec<_> = writers.into_iter().map(|h| h.join()).collect();
            stop.store(true, Relaxed);
            let reads: Vec<_> = readers.into_iter().map(|h| h.join()).collect();
            for d in done {
                if let Err(p) = d {
                    std::panic::resume_unwind(p);
                }
            }
            for r in reads {
                match r {
                    Ok(n) => assert!(n > 0, "a reader read nothing"),
                    Err(p) => std::panic::resume_unwind(p),
                }
            }
        });
        // Every key once in the index.
        let mut seen = std::collections::HashSet::new();
        for i in 0..=live.mask {
            let w = live.slot(i).load(Relaxed);
            if w != 0 {
                let e = live.entry((w & 0xffff_ffff) as usize - 1);
                assert!(
                    seen.insert(live.key_of(e).to_vec()),
                    "a key with two entries"
                );
            }
        }
        assert_eq!(seen.len(), KEYS as usize);
        // Each key's last value is some writer's last round.
        let last: Vec<u8> = (0..WRITERS as u64)
            .map(|w| (w * ROUNDS + ROUNDS) as u8)
            .collect();
        for i in 0..KEYS {
            crate::workload::db_key_into(i, &mut kb);
            assert_eq!(t.get(&kb).unwrap(), VALUE);
            OUT.with(|o| {
                let v = o.borrow();
                assert!(uniform(&v[..]) && last.contains(&v[0]), "key {i}: {}", v[0]);
            });
        }
        let c: std::collections::HashMap<_, _> = t.counters().into_iter().collect();
        assert_eq!(c["proto_live_inserts"], KEYS as f64, "one insert a key");
        assert_eq!(c["proto_live_appends"], 0.0);
        c["proto_live_races"] as u64
    }
}
