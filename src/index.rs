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
///
/// `repr(C)` so a mapped index can hand back `&[Ext]` borrowed straight out of
/// the file rather than decoding one. Four `u32`s in declaration order, four
/// byte alignment; `flatindex` lays its records out to match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Ext {
    pub block: u32,
    pub off: u32,
    pub len: u32,
    /// Offset of the last record inside this extent. Without it, reading the
    /// newest value means walking every record in the run -- 267 of them in
    /// the deep shape, which is why read_last measured 105k/s against
    /// Uppend's 363k. Four bytes buys an O(1) answer.
    pub last: u32,
    /// How many records the run holds, with the top bit reserved as the
    /// tombstone flag (`Ext::TOMBSTONE`). Four more bytes per extent buy an
    /// O(extents) count for variable-width values, which had been measured
    /// at the cost of reading them, and the bit is what a delete
    /// needs to say "nothing older than this extent is live".
    pub count: u32,
}

impl Ext {
    /// Set on an extent that supersedes every older value of its key.
    pub const TOMBSTONE: u32 = 1 << 31;

    /// Bit 30 of `count`: the run's values all share one width and are
    /// stored back to back with no length prefixes. The width is
    /// `len / records()`, so nothing else is stored, and `last` is
    /// `(records - 1) * width` as for any run. Format v6; a reader from
    /// before it refuses the file by its magic rather than parsing a fixed
    /// run as prefixed.
    pub const FIXED: u32 = 1 << 30;

    #[inline]
    pub fn is_fixed(&self) -> bool {
        self.count & Ext::FIXED != 0
    }

    /// The width of a fixed run's values; `None` for a prefixed run or an
    /// empty or inconsistent one.
    #[inline]
    pub fn fixed_width(&self) -> Option<usize> {
        if !self.is_fixed() {
            return None;
        }
        let n = self.records() as usize;
        if n == 0 || !(self.len as usize).is_multiple_of(n) {
            return None;
        }
        Some(self.len as usize / n)
    }

    /// A `block` of this value means the run is INLINE: its bytes sit in the
    /// index record itself, after the extents, at `off` within that tail.
    /// A read of such a run never consults the block table or a block --
    /// two cache misses fewer per lookup at a million keys, which is what
    /// the read lead needed past the arrangement ceiling.
    /// Only the segment writer produces them; `Store` never does, and a
    /// reader from before this extension errors on the block id rather
    /// than answering wrongly.
    pub const INLINE: u32 = u32::MAX;

    #[inline]
    pub fn is_inline(&self) -> bool {
        self.block == Ext::INLINE
    }

    /// Records in the run, without the flag.
    #[inline]
    pub fn records(&self) -> u32 {
        self.count & !(Ext::TOMBSTONE | Ext::FIXED)
    }

    #[inline]
    pub fn is_tombstone(&self) -> bool {
        self.count & Ext::TOMBSTONE != 0
    }
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

/// Read a varint, stopping at the end of the buffer rather than past it.
///
/// This used to index `buf[*pos]` unconditionally and shift without bound. On
/// a damaged or reused index section both are reachable: the corruption
/// experiment drove it straight into "index out of bounds: the len is 13220
/// but the index is 13220", which in an embedded library is the host
/// application aborting.
///
/// Truncated input now yields whatever was decoded so far, and `*pos` stops at
/// the end. That alone is not sufficient -- a caller that then slices by the
/// decoded length can still run off the end -- so every caller validates the
/// length it gets back against the bytes actually remaining.
pub fn get_uvarint(buf: &[u8], pos: &mut usize) -> u64 {
    let mut v = 0u64;
    let mut shift = 0u32;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b < 0x80 {
            return v;
        }
        shift += 7;
        // A 64-bit value needs at most ten 7-bit groups; beyond that the
        // input is not a varint this encoder produced.
        if shift >= 64 {
            break;
        }
    }
    v
}

// FxHash was tried for the store's internal maps and made the write path ten
// times slower: 411,000 puts per second became 38,000. The keys were decimal
// strings of fixed width, nearly identical except in their last few digits,
// and multiply-rotate hashing clusters badly on exactly that shape -- the map
// degenerated into linear probing. A faster hash is only faster if it still
// spreads the keys it is given, and structured keys are the common case for a
// store rather than the exception.

/// Hand each value of a run to `f`, in order, and return how many. A fixed
/// run is `records` slices of one width; a prefixed run is `[varint len]
/// [bytes]` repeated. The one decoder every reader shares, so the two
/// encodings cannot drift apart between them. `Err` names the damage.
pub fn each_value(run: &[u8], e: &Ext, f: &mut dyn FnMut(&[u8])) -> Result<u64, &'static str> {
    if e.is_fixed() {
        let Some(w) = e.fixed_width() else {
            return Err("fixed run's length is not a multiple of its count");
        };
        if run.len() != e.len as usize || w == 0 {
            return Err("fixed run does not match its extent");
        }
        let mut n = 0u64;
        for v in run.chunks_exact(w) {
            f(v);
            n += 1;
        }
        return Ok(n);
    }
    let mut p = 0usize;
    let mut n = 0u64;
    while p < run.len() {
        let len = get_uvarint(run, &mut p) as usize;
        let end = p.checked_add(len).ok_or("record length overflows")?;
        if end > run.len() {
            return Err("record runs past the end of its extent");
        }
        f(&run[p..end]);
        n += 1;
        p = end;
    }
    Ok(n)
}

/// Encode a run from its values: fixed when every value has the same
/// non-zero width, prefixed otherwise. Returns the bytes, the offset of the
/// last value within them, and the count word's flag (`Ext::FIXED` or 0).
pub fn encode_run(values: &[u8], lens: &[u32], out: &mut Vec<u8>) -> (u32, u32) {
    out.clear();
    let n = lens.len();
    let fixed = n > 0 && lens[0] > 0 && lens.iter().all(|&l| l == lens[0]);
    if fixed {
        out.extend_from_slice(values);
        return (((n as u32) - 1) * lens[0], Ext::FIXED);
    }
    let mut at = 0usize;
    let mut last = 0u32;
    for &l in lens {
        last = out.len() as u32;
        put_uvarint(out, l as u64);
        out.extend_from_slice(&values[at..at + l as usize]);
        at += l as usize;
    }
    (last, 0)
}

/// The width a prefixed run's values share, if they all share one and it
/// is not zero; `None` for a mixed or empty run. For a writer deciding how
/// to seal a run it holds already prefixed.
pub fn uniform_width(prefixed: &[u8]) -> Option<usize> {
    let mut p = 0usize;
    let mut w: Option<usize> = None;
    while p < prefixed.len() {
        let len = get_uvarint(prefixed, &mut p) as usize;
        if len == 0 || p.checked_add(len)? > prefixed.len() {
            return None;
        }
        match w {
            None => w = Some(len),
            Some(x) if x != len => return None,
            _ => {}
        }
        p += len;
    }
    w
}

/// Strip the prefixes off a run `uniform_width` accepted, into `out`.
pub fn strip_prefixes(prefixed: &[u8], out: &mut Vec<u8>) -> u32 {
    out.clear();
    let mut p = 0usize;
    let mut n = 0u32;
    while p < prefixed.len() {
        let len = get_uvarint(prefixed, &mut p) as usize;
        out.extend_from_slice(&prefixed[p..p + len]);
        p += len;
        n += 1;
    }
    n
}
