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
}
