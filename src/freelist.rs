//! Internally reusable space.
//!
//! Punching holes hands space back to the filesystem, so the store immediately
//! asks for more and the file grows sparse and scattered -- 2,222 MB written
//! and 1,249 MB punched to hold 390 MB of live data. It also depends on a
//! syscall that varies by platform and is simply absent on the network
//! filesystems this design otherwise suits.
//!
//! Reusing the space ourselves avoids all of that: the file stays dense, its
//! length is bounded by live data rather than by total writes, and the
//! mechanism is identical everywhere. Offsets never move, so no index rewrite
//! is ever forced -- freed space is reused in place rather than compacted out
//! from under the readers.

/// Slots are rounded to size classes so that a freed block is interchangeable
/// with the next request of similar size. Without classes, reuse degenerates
/// into the classic best-fit fragmentation problem.
const CLASS_MIN_SHIFT: u32 = 12; // 4 KiB
const OCTAVES: usize = 20; // up to 2 GiB
/// Sub-classes per octave. Rounding a slot to the next power of two wastes up
/// to half of it -- a 47 KiB block reserving 64 KiB throws away 26%, which
/// lands straight on the one axis still being lost. Eight steps per octave
/// caps the waste at 12.5% and costs only a longer class table.
const SUB: usize = 8;
const CLASSES: usize = OCTAVES * SUB;

pub struct FreeList {
    /// Freed slots per size class, each tagged with the generation it was
    /// released in and held back until enough checkpoints have passed.
    ///
    /// Reuse is what actually destroys an older state, and a reader that
    /// opened a moment ago is holding one. Quarantining a slot for a few
    /// generations gives every reader a bounded window in which the data it
    /// can see is guaranteed to still be there; a reader that outlives the
    /// window still fails loudly rather than reading someone else's bytes.
    classes: Vec<Vec<(u64, u32, u64)>>,
    free_bytes: u64,
    reused: u64,
    reused_bytes: u64,
}

fn class_of(len: u32) -> usize {
    let l = (len.max(1 << CLASS_MIN_SHIFT)) as u64;
    let oct = (64 - (l - 1).leading_zeros() as usize).saturating_sub(1);
    let oct = oct.saturating_sub(CLASS_MIN_SHIFT as usize);
    let base = 1u64 << (CLASS_MIN_SHIFT as usize + oct);
    // which eighth of this octave does it fall into
    let step = base / SUB as u64;
    let sub = if step == 0 {
        0
    } else {
        (((l - 1) - base) / step) as usize + 1
    };
    let sub = sub.min(SUB - 1);
    (oct * SUB + sub).min(CLASSES - 1)
}

/// Capacity actually reserved for a block of this size, rounded up to its
/// class boundary rather than to the next power of two.
pub fn capacity_for(len: u32) -> u32 {
    let c = class_of(len);
    let oct = c / SUB;
    let sub = c % SUB;
    let base = 1u64 << (CLASS_MIN_SHIFT as usize + oct);
    let cap = base + (base / SUB as u64) * sub as u64;
    cap.max(len as u64) as u32
}

impl FreeList {
    pub fn new() -> Self {
        FreeList {
            classes: vec![Vec::new(); CLASSES],
            free_bytes: 0,
            reused: 0,
            reused_bytes: 0,
        }
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_bytes
    }

    pub fn reused(&self) -> (u64, u64) {
        (self.reused, self.reused_bytes)
    }

    /// Give a slot back. `cap` is what was reserved, not what was written.
    pub fn release(&mut self, off: u64, cap: u32, generation: u64) {
        let c = class_of(cap);
        self.classes[c].push((off, cap, generation));
        self.free_bytes += cap as u64;
    }

    /// Consume exactly this slot, splitting the remainder back in. Used by
    /// defragmentation, which chooses a destination rather than accepting
    /// whichever slot the size class offers.
    pub fn take_at(&mut self, off: u64, len: u32) {
        for ci in 0..self.classes.len() {
            if let Some(i) = self.classes[ci].iter().position(|(o, _, _)| *o == off) {
                let (o, l, gen) = self.classes[ci].remove(i);
                self.free_bytes -= l as u64;
                if l > len {
                    let rest = l - len;
                    let c2 = class_of(rest);
                    self.classes[c2].push((o + len as u64, rest, gen));
                    self.free_bytes += rest as u64;
                }
                return;
            }
        }
    }

    /// Take a slot big enough for `len`, if one is waiting.
    ///
    /// Only the exact class is consulted. Searching larger classes would
    /// succeed more often but strand the difference, and a block written at
    /// the target size nearly always lands in the class it came from.
    /// Take a slot freed strictly before `floor`, if the oldest in its class
    /// qualifies. Entries are appended in generation order, so the front is
    /// the oldest and no scan is needed.
    pub fn take_below(&mut self, len: u32, floor: u64) -> Option<(u64, u32)> {
        let c = class_of(len);
        let (off, cap, gen) = *self.classes[c].first()?;
        if cap < len || gen >= floor {
            return None;
        }
        self.classes[c].remove(0);
        self.free_bytes -= cap as u64;
        self.reused += 1;
        self.reused_bytes += cap as u64;
        Some((off, cap))
    }

    #[allow(dead_code)]
    pub fn take(&mut self, len: u32, generation: u64, grace: u64) -> Option<(u64, u32)> {
        let c = class_of(len);
        // oldest first: those are the ones whose quarantine has expired
        // Entries are appended in generation order, so the oldest is at the
        // front: if it is still in quarantine nothing behind it can be out.
        // Scanning the class for an eligible entry was O(n) on the write path
        // and got slower as the store ran -- a third of the throughput decay
        // over three minutes of sustained load.
        let (off, cap, gen) = *self.classes[c].first()?;
        if cap < len || gen.saturating_add(grace) > generation {
            return None;
        }
        self.classes[c].remove(0);
        self.free_bytes -= cap as u64;
        self.reused += 1;
        self.reused_bytes += cap as u64;
        Some((off, cap))
    }
}

impl FreeList {
    /// Every free slot, coalesced, in file order.
    ///
    /// Retaining free space is right for a store that keeps running -- the
    /// space gets reused. It is dead weight for one that has stopped, so at
    /// close the trailing run is truncated away and the interior is offered
    /// back to the filesystem where that is supported.
    pub fn coalesced(&self) -> Vec<(u64, u64)> {
        let mut all: Vec<(u64, u64)> = self
            .classes
            .iter()
            .flat_map(|c| c.iter().map(|(o, l, _)| (*o, *l as u64)))
            .collect();
        all.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(all.len());
        for (off, len) in all {
            match out.last_mut() {
                Some(last) if last.0 + last.1 == off => last.1 += len,
                _ => out.push((off, len)),
            }
        }
        out
    }
}
