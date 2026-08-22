//! The key index.
//!
//! Uppend spent 402 bytes per key to record ten 30-bit file offsets, against
//! an information-theoretic floor of 3.75 bytes -- 107x, and the single
//! largest reason it lost the space axis on shallow keys. The cause was
//! structural: every key was handed a minimum block of eight position slots
//! plus a 24-byte header whether it held ten positions or one.
//!
//! So the common case is inlined. A key whose values fit in one sealed extent
//! -- which is every key in a shallow workload -- carries that extent inline
//! and allocates nothing else. Only a key that outgrows one extent gets a
//! spilled vector.

/// One contiguous run of a key's values inside a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ext {
    pub block: u32,
    pub off: u32,
    pub len: u32,
    /// Offset of the last record inside this extent. Without it, reading the
    /// newest value means walking every record in the run -- 267 of them in
    /// the deep shape, which is why read_last measured 105k/s against
    /// Uppend's 363k. Four bytes buys an O(1) answer.
    pub last: u32,
}

/// Extents of a single key. Inline until the key needs more than one.
#[derive(Clone, Debug)]
pub enum Extents {
    None,
    One(Ext),
    Many(Vec<Ext>),
}

impl Extents {
    pub fn push(&mut self, e: Ext) {
        match self {
            Extents::None => *self = Extents::One(e),
            Extents::One(first) => *self = Extents::Many(vec![*first, e]),
            Extents::Many(v) => v.push(e),
        }
    }

    pub fn as_slice(&self) -> &[Ext] {
        match self {
            Extents::None => &[],
            Extents::One(e) => std::slice::from_ref(e),
            Extents::Many(v) => v.as_slice(),
        }
    }

    pub fn first(&self) -> Option<Ext> {
        self.as_slice().first().copied()
    }

    pub fn last(&self) -> Option<Ext> {
        self.as_slice().last().copied()
    }
}

impl Default for Extents {
    fn default() -> Self {
        Extents::None
    }
}

/// Varint helpers -- positions are small numbers most of the time, and the
/// serialized index is the thing we are trying to keep near the floor.
pub fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

pub fn get_uvarint(buf: &[u8], pos: &mut usize) -> u64 {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = buf[*pos];
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b < 0x80 {
            return v;
        }
        shift += 7;
    }
}

// FxHash was tried for the store's internal maps and made the write path ten
// times slower: 411,000 puts per second became 38,000. The keys were decimal
// strings of fixed width, nearly identical except in their last few digits,
// and multiply-rotate hashing clusters badly on exactly that shape -- the map
// degenerated into linear probing. A faster hash is only faster if it still
// spreads the keys it is given, and structured keys are the common case for a
// store rather than the exception.
