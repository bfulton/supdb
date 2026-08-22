//! The writer's key index: open addressing, inline fingerprints, keys in an
//! arena.
//!
//! The write path used two `HashMap<Vec<u8>, _>` -- one for sealed extents and
//! one for values still buffered -- so every put hashed twice, probed twice,
//! and chased a heap pointer on each probe to compare the key. The cost grew
//! with the key count accordingly: 1.18 microseconds per put at six thousand
//! keys, 2.97 at two and a half million.
//!
//! The reader already demonstrates the alternative on the same keys and the
//! same machine, resolving a key and reading a whole value in less time than
//! the writer took to store one. This is that structure: one table rather than
//! two, a byte of hash inline in the slot so a mismatch costs no memory
//! traffic, and key bytes packed contiguously instead of one allocation each.

use crate::index::Extents;

const EMPTY: u32 = u32::MAX;
/// Grow at seven eighths full. Open addressing degrades sharply past that, and
/// the fingerprint keeps the probe cheap until then.
const LOAD_NUM: usize = 7;
const LOAD_DEN: usize = 8;

/// Key bytes carried in the slot itself. Twenty covers the sixteen-byte keys
/// db_bench uses and the ten-byte ones the shape benchmarks use; anything
/// longer keeps its bytes in the arena and its offset here instead.
const INLINE: usize = 20;

/// A slot is thirty-two bytes, two to a cache line.
///
/// The previous layout was eight bytes holding a byte of hash and an entry
/// index, which meant a probe touched three separate lines: the slot, the
/// entry to learn where the key lived, and the arena to compare it. At six
/// million keys none of those are cached and all three are misses. Carrying
/// the key in the slot collapses that to one: a mismatch is settled without
/// leaving the line, and only a confirmed hit reaches for the entry.
#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    entry: u32,
    len: u32,
    /// The key when it fits, otherwise its offset in the arena in the first
    /// four bytes.
    key: [u8; INLINE],
    fp: u32,
}

impl Slot {
    const fn empty() -> Slot {
        Slot {
            entry: EMPTY,
            len: 0,
            key: [0; INLINE],
            fp: 0,
        }
    }

    #[inline]
    fn arena_off(&self) -> usize {
        u32::from_le_bytes([self.key[0], self.key[1], self.key[2], self.key[3]]) as usize
    }
}

pub struct Entry<P> {
    key_off: u32,
    key_len: u32,
    pub extents: Extents,
    pub pending: Option<P>,
}

pub struct KeyTable<P> {
    slots: Vec<Slot>,
    mask: usize,
    entries: Vec<Entry<P>>,
    arena: Vec<u8>,
}

/// The same mixer the reader uses. Deliberately not a multiply-rotate hash:
/// those cluster on fixed-width decimal keys, which cost a factor of ten here
/// when tried.
#[inline]
pub fn hash(key: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h ^ (h >> 29)
}

/// Ask for the slot's cache line while the hash is still being computed.
#[inline(always)]
fn prefetch(p: *const Slot) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_prefetch(p as *const i8, core::arch::x86_64::_MM_HINT_T0);
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = p;
}

impl<P> KeyTable<P> {
    pub fn new() -> Self {
        KeyTable {
            slots: vec![Slot::empty(); 64],
            mask: 63,
            entries: Vec::new(),
            arena: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    fn slot_key<'a>(&'a self, s: &'a Slot) -> &'a [u8] {
        if s.len as usize <= INLINE {
            &s.key[..s.len as usize]
        } else {
            let off = s.arena_off();
            &self.arena[off..off + s.len as usize]
        }
    }

    #[inline]
    fn find(&self, key: &[u8], h: u64) -> Option<u32> {
        let fp = (h >> 32) as u32 | 1;
        let mut i = (h as usize) & self.mask;
        prefetch(&self.slots[i]);
        loop {
            let s = &self.slots[i];
            if s.entry == EMPTY {
                return None;
            }
            // settled inside one cache line unless the key is long
            if s.fp == fp && s.len as usize == key.len() && self.slot_key(s) == key {
                return Some(s.entry);
            }
            i = (i + 1) & self.mask;
        }
    }

    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut Entry<P>> {
        let idx = self.find(key, hash(key))?;
        Some(&mut self.entries[idx as usize])
    }

    /// Find the key, inserting an empty entry if it is not present.
    pub fn get_or_insert(&mut self, key: &[u8]) -> &mut Entry<P> {
        let h = hash(key);
        if let Some(idx) = self.find(key, h) {
            return &mut self.entries[idx as usize];
        }
        if (self.entries.len() + 1) * LOAD_DEN > self.slots.len() * LOAD_NUM {
            self.grow();
        }
        let key_off = self.arena.len() as u32;
        self.arena.extend_from_slice(key);
        let idx = self.entries.len() as u32;
        self.entries.push(Entry {
            key_off,
            key_len: key.len() as u32,
            extents: Extents::None,
            pending: None,
        });
        self.place(h, idx, key, key_off);
        &mut self.entries[idx as usize]
    }

    fn place(&mut self, h: u64, idx: u32, key: &[u8], key_off: u32) {
        let fp = (h >> 32) as u32 | 1;
        let mut i = (h as usize) & self.mask;
        while self.slots[i].entry != EMPTY {
            i = (i + 1) & self.mask;
        }
        let mut slot = Slot {
            entry: idx,
            len: key.len() as u32,
            key: [0; INLINE],
            fp,
        };
        if key.len() <= INLINE {
            slot.key[..key.len()].copy_from_slice(key);
        } else {
            slot.key[..4].copy_from_slice(&key_off.to_le_bytes());
        }
        self.slots[i] = slot;
    }

    fn grow(&mut self) {
        let cap = self.slots.len() * 2;
        let old = std::mem::replace(&mut self.slots, vec![Slot::empty(); cap]);
        self.mask = cap - 1;
        for s in old.into_iter().filter(|s| s.entry != EMPTY) {
            let h = {
                let k: &[u8] = if s.len as usize <= INLINE {
                    &s.key[..s.len as usize]
                } else {
                    let off = s.arena_off();
                    &self.arena[off..off + s.len as usize]
                };
                hash(k)
            };
            let mut i = (h as usize) & self.mask;
            while self.slots[i].entry != EMPTY {
                i = (i + 1) & self.mask;
            }
            self.slots[i] = s;
        }
    }

    /// Every entry, with its key. Entries are never removed -- a deleted key
    /// keeps a tombstone -- so indices stay valid for the table's lifetime.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &Entry<P>)> {
        self.entries.iter().map(move |e| {
            (
                &self.arena[e.key_off as usize..(e.key_off + e.key_len) as usize],
                e,
            )
        })
    }

    /// Take every buffered value, leaving the sealed extents in place.
    ///
    /// Entries are handed back by index, not by key. Sealing used to copy each
    /// key out here and then look it up twice more -- once to find what the
    /// value superseded and once to record where it landed -- when the entry
    /// was already located. An index is a stable handle for the table's
    /// lifetime, since entries are never removed.
    pub fn take_pending(&mut self) -> Vec<(u32, P)> {
        let mut out = Vec::new();
        for (i, e) in self.entries.iter_mut().enumerate() {
            if let Some(p) = e.pending.take() {
                out.push((i as u32, p));
            }
        }
        out
    }

    #[inline]
    pub fn entry_at(&mut self, idx: u32) -> &mut Entry<P> {
        &mut self.entries[idx as usize]
    }

    /// The key an index refers to.
    pub fn key_at(&self, idx: u32) -> &[u8] {
        let e = &self.entries[idx as usize];
        &self.arena[e.key_off as usize..(e.key_off + e.key_len) as usize]
    }

    pub fn extents_of(&mut self, key: &[u8]) -> Option<&mut Extents> {
        self.get_mut(key).map(|e| &mut e.extents)
    }

    /// Sort indices by the keys they refer to, without copying any of them.
    pub fn sort_by_key(&self, idx: &mut [(u32, impl Sized)]) {
        idx.sort_unstable_by(|a, b| self.key_at(a.0).cmp(self.key_at(b.0)));
    }
}
