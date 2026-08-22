//! A table of live readers, so the writer knows what it may not reuse.
//!
//! Reuse is what destroys a state, and any reader takes non-zero time to walk
//! the index it opened. Holding freed space for a fixed number of checkpoints
//! bounds that, but the bound is a guess: measured here, no window at all
//! failed 41% of concurrent reads, a window of one failed twelve, and eight
//! failed none -- on this machine, for this workload, today.
//!
//! Publishing instead of guessing removes the constant. A reader claims a slot
//! and records the generation it is reading; the writer reuses only space
//! freed before the oldest generation any reader still holds. This is what
//! LMDB does, and for the same reason.
//!
//! The table lives in the reserved first page, so it is shared by every
//! process that opens the file. A reader that dies without releasing its slot
//! would pin space forever, so each slot carries a heartbeat and is treated as
//! abandoned once it goes stale.

use std::sync::atomic::{AtomicU64, Ordering};

pub const TABLE_OFF: usize = 1024;
pub const SLOTS: usize = 64;
const WORDS: usize = 4; // generation, heartbeat, owner, reserved
pub const TABLE_BYTES: usize = SLOTS * WORDS * 8;

/// A slot is considered abandoned once its heartbeat is this far behind.
pub const STALE_MILLIS: u64 = 30_000;

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The table as a slice of atomics over the mapping.
///
/// # Safety
/// `base` must point at TABLE_BYTES of mapped, correctly aligned memory that
/// stays valid for `'a`.
pub unsafe fn slots<'a>(base: *const u8) -> &'a [AtomicU64] {
    std::slice::from_raw_parts(base.add(TABLE_OFF) as *const AtomicU64, SLOTS * WORDS)
}

/// Claim a slot and publish the generation being read. Returns its index.
pub fn acquire(t: &[AtomicU64], generation: u64) -> Option<usize> {
    let now = now_millis();
    for i in 0..SLOTS {
        let gen = &t[i * WORDS];
        let beat = &t[i * WORDS + 1];
        let hb = beat.load(Ordering::Acquire);
        let free =
            gen.load(Ordering::Acquire) == 0 || (hb != 0 && now.saturating_sub(hb) > STALE_MILLIS);
        if !free {
            continue;
        }
        // claim by heartbeat first: another reader seeing a fresh beat will
        // pass over this slot, and the generation is published second so the
        // writer never sees a claimed slot without one
        if beat
            .compare_exchange(hb, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            gen.store(generation, Ordering::Release);
            return Some(i);
        }
    }
    None
}

pub fn release(t: &[AtomicU64], slot: usize) {
    t[slot * WORDS].store(0, Ordering::Release);
    t[slot * WORDS + 1].store(0, Ordering::Release);
}

/// The oldest generation any live reader still holds, if any.
pub fn oldest(t: &[AtomicU64]) -> Option<u64> {
    let now = now_millis();
    let mut min = None;
    for i in 0..SLOTS {
        let gen = t[i * WORDS].load(Ordering::Acquire);
        if gen == 0 {
            continue;
        }
        let hb = t[i * WORDS + 1].load(Ordering::Acquire);
        if hb != 0 && now.saturating_sub(hb) > STALE_MILLIS {
            continue; // abandoned
        }
        min = Some(min.map_or(gen, |m: u64| m.min(gen)));
    }
    min
}
