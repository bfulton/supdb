//! The compact form's LZ4 block encoder for the groups a form file holds.
//!
//! The block format is LZ4's and nothing else: any LZ4 block decoder reads
//! what this writes, `lz4_flex`'s included, which is the engine's. What it
//! changes is which of the encodings of a group the writer picks. A cold
//! decode's cost is the decoder's branches on tokens it has not learned,
//! so fewer sequences a group is fewer branches a read: liblz4's
//! high-compression mode, which searches longer for its matches, took 16%
//! off the form's cold decode and 4% off its bytes at level 9, where
//! `lz4_flex`'s greedy encoder, the fill's until now, takes the first
//! match its one-entry hash table offers (`bench/examples/zdecode.rs`,
//! `--hc`).
//!
//! Two passes over a group, both bounded by its length. The search walks a
//! hash chain over every earlier position whose four bytes hash alike, up
//! to `attempts` of them and never past LZ4's 64 KiB window, and keeps the
//! longest match at each position a match may start at; a match that runs
//! to the end of what a match may cover ends the walk, and inside a long
//! match the positions are inserted and not searched but for the last few,
//! where any match reaching past it is found again -- which is what keeps
//! a group of one repeated pattern from costing a walk a byte. The parse
//! is a shortest path over the group's positions, priced in the bytes each
//! sequence writes and `seq_cost` more for each sequence: a position is
//! reached after a match or inside a run of literals, a literal adds its
//! byte and the length byte a run gains every 255, and a match at a
//! position ends at its longest or where another match that reaches past
//! that starts, since a cut anywhere else buys literals or a sequence the
//! longest match covers for nothing. The cheapest path that ends in the
//! five literals the format requires is written. The tables are kept between groups and never cleared: a hash
//! entry names a position by the running base of the group it was made in,
//! and an entry below the current base is no position at all.

/// The shortest match LZ4 encodes.
const MINMATCH: usize = 4;
/// The literals a block always ends in.
const LASTLITERALS: usize = 5;
/// The last match starts at least this many bytes before the block's end.
const MFLIMIT: usize = 12;
/// The farthest back a match's offset reaches.
const MAX_DISTANCE: usize = 65_535;
/// The hash table's index bits: 8192 heads, for groups of a few kibibytes.
const HASH_LOG: u32 = 13;
/// How far short of a long match's end the search resumes.
const RESUME: usize = 3;
/// The largest group the encoder takes, the form's largest decoded group.
pub const MAX_INPUT: usize = 64 * 1024;

const INF: u32 = u32::MAX / 4;

/// How hard the encoder looks: the chain positions a match search walks,
/// the price of a sequence beside its bytes, and the match long enough
/// that its inside is not searched. `attempts` of zero writes every group
/// as literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effort {
    pub attempts: u32,
    pub seq_cost: u32,
    /// A match this long has its positions inserted and not searched, but
    /// for the last [`RESUME`] it covers.
    pub skip_min: u32,
    /// Zero: the shortest path over the longest match at every position.
    /// Else the lazy parse: a match taken where none of the next `lazy`
    /// positions starts a longer one, and the positions it covers only
    /// inserted, searched by nothing.
    pub lazy: u32,
}

impl Effort {
    /// The search the form ships with: sixteen chain positions and the
    /// lazy parse one step ahead. Priced on the form's own groups
    /// (`bench/examples/zdecode.rs`, `--rhc`): the groups' bytes at 0.96
    /// of the greedy encoder's and their sequences at 0.84, as liblz4's
    /// high-compression mode writes them, in half the time of the shortest
    /// path over every position (`lazy: 0`), whose bytes and sequences it
    /// matches; sixty-four positions or a second step ahead buy a
    /// fiftieth of a percent, and a price on sequences, which the shortest
    /// path takes, buys fewer of them and a slower decode, the literals
    /// they leave costing more.
    pub const SHIPPED: Effort = Effort {
        attempts: 16,
        seq_cost: 0,
        skip_min: 8,
        lazy: 1,
    };
}

/// The encoder's tables and scratch, kept across groups so that a group's
/// encode allocates nothing once the largest group has been seen.
pub struct Encoder {
    head: Vec<u32>,
    chain: Vec<u16>,
    base: u32,
    /// The longest match at each position: its length and offset.
    best: Vec<(u32, u16)>,
    /// The cheapest price of the bytes before a position with the last
    /// sequence closed by a match ending there.
    m_cost: Vec<u32>,
    /// The match that closes there: where it starts and its offset.
    m_from: Vec<(u32, u16)>,
    /// The length of the run of literals the cheapest price inside a run
    /// at a position has, for the way back.
    p_lit: Vec<u32>,
    /// The sequences found, last first: literal start, literal count,
    /// offset, match length.
    seqs: Vec<(u32, u32, u16, u32)>,
}

impl Default for Encoder {
    fn default() -> Self {
        Encoder::new()
    }
}

impl Encoder {
    pub fn new() -> Encoder {
        Encoder {
            head: vec![0; 1 << HASH_LOG],
            chain: Vec::new(),
            base: 0,
            best: Vec::new(),
            m_cost: Vec::new(),
            m_from: Vec::new(),
            p_lit: Vec::new(),
            seqs: Vec::new(),
        }
    }

    /// `input` as an LZ4 block into `out`, the length written, or `None`
    /// where `out` is shorter than [`max_output`] of the input could need
    /// or the input is over [`MAX_INPUT`].
    pub fn compress_into(&mut self, input: &[u8], out: &mut [u8], e: Effort) -> Option<usize> {
        let n = input.len();
        if n > MAX_INPUT || out.len() < max_output(n) {
            return None;
        }
        self.parse(input, e);
        Some(self.emit(input, out))
    }

    /// The search and the parse, leaving the chosen sequences in `seqs`.
    fn parse(&mut self, input: &[u8], e: Effort) {
        self.seqs.clear();
        if input.len() < MFLIMIT + 1 || e.attempts == 0 {
            return;
        }
        if e.lazy > 0 {
            self.lazy(input, e);
            return;
        }
        self.search(input, e);
        self.shortest(input, e);
    }

    /// The lazy parse, its sequences left in `seqs`: at each position not
    /// inside a match taken, the longest match its chain holds, taken
    /// where none of the next `e.lazy` positions starts a longer one, the
    /// longer one tried in its place otherwise; the positions a match
    /// covers joined to their chains and not searched. Every position is
    /// inserted before any later one is searched, so a search sees the
    /// whole window behind it, as `search` does.
    #[inline(never)]
    fn lazy(&mut self, input: &[u8], e: Effort) {
        let n = input.len();
        if self.base as usize > u32::MAX as usize - 2 * MAX_INPUT - 2 {
            self.head.iter_mut().for_each(|h| *h = 0);
            self.base = 0;
        }
        let base = self.base;
        self.base += n as u32 + 1;
        if self.chain.len() < n {
            self.chain.resize(n, 0);
        }
        let head = &mut self.head[..];
        let chain = &mut self.chain[..n];
        let last_start = n - MFLIMIT;
        let limit = n - LASTLITERALS;
        // Every position before `ins` is in its chain.
        let mut ins = 0usize;
        let at = |p: usize, ins: &mut usize, head: &mut [u32], chain: &mut [u16]| {
            longest_at(input, p, limit, e.attempts, ins, head, chain, base)
        };
        let (mut s, mut anchor) = (0usize, 0usize);
        while s <= last_start {
            let (mut len, mut off) = at(s, &mut ins, head, chain);
            if len < MINMATCH {
                s += 1;
                continue;
            }
            // A match `k` steps on is taken in this one's place where it is
            // longer by more than the `k - 1` literals more it leaves, and
            // the steps after it are asked in turn. The positions are asked
            // in order, so each is searched once and after every position
            // before it is in its chain.
            'ahead: loop {
                for k in 1..=e.lazy as usize {
                    if s + k > last_start {
                        break 'ahead;
                    }
                    let (l2, o2) = at(s + k, &mut ins, head, chain);
                    if l2 > len + k - 1 {
                        s += k;
                        (len, off) = (l2, o2);
                        continue 'ahead;
                    }
                }
                break;
            }
            self.seqs
                .push((anchor as u32, (s - anchor) as u32, off as u16, len as u32));
            s += len;
            anchor = s;
        }
        self.seqs.reverse();
    }

    /// The longest match at every position a match may start at, in
    /// `best`.
    #[inline(never)]
    fn search(&mut self, input: &[u8], e: Effort) {
        let n = input.len();
        // A fresh base for this group: every head below it names nothing.
        // Wrapping it would revive old entries, so the table is cleared
        // the one time in four billion bytes it would.
        if self.base as usize > u32::MAX as usize - 2 * MAX_INPUT - 2 {
            self.head.iter_mut().for_each(|h| *h = 0);
            self.base = 0;
        }
        let base = self.base;
        self.base += n as u32 + 1;
        self.chain.clear();
        self.chain.resize(n, 0);
        self.best.clear();
        self.best.resize(n, (0, 0));
        // The tables as slices of their own, so that nothing the loop
        // stores is taken for a store to the encoder's fields.
        let head = &mut self.head[..];
        let chain = &mut self.chain[..n];
        let best = &mut self.best[..n];
        // Matches start at or before `last_start` and end at or before
        // `limit`.
        let last_start = n - MFLIMIT;
        let limit = n - LASTLITERALS;
        // Inside a match of `skip_min` or more, found at `run.0` with the
        // offset `run.2` and covering up to `run.1`, a position is only
        // inserted: its longest is the match carried on, and the search
        // resumes `RESUME` short of the cover's end, where any match that
        // reaches past it is found again, since a match that does covers
        // that position too.
        let mut run = (0usize, 0usize, 0u16);
        for s in 0..=last_start {
            let v = read_u32(input, s);
            let h = hash(v);
            if s < run.1.saturating_sub(RESUME) && s > run.0 {
                best[s] = ((run.1 - s) as u32, run.2);
                insert(head, chain, s, h, base);
                continue;
            }
            let max_len = limit - s;
            let (mut best_len, mut best_off) = (MINMATCH - 1, 0usize);
            let mut cand = head[h];
            let mut tries = e.attempts;
            while cand > base && tries > 0 {
                let q = (cand - base - 1) as usize;
                let d = s - q;
                if d > MAX_DISTANCE {
                    break;
                }
                tries -= 1;
                if input[q + best_len] == input[s + best_len] && read_u32(input, q) == v {
                    let l = common(input, q, s, max_len);
                    if l > best_len {
                        best_len = l;
                        best_off = d;
                        // A match to the end of what a match may cover ends
                        // the walk: nothing is longer.
                        if l == max_len {
                            break;
                        }
                    }
                }
                let c = chain[q];
                if c == 0 {
                    break;
                }
                cand -= c as u32;
            }
            if best_len >= MINMATCH {
                best[s] = (best_len as u32, best_off as u16);
                if best_len >= e.skip_min as usize {
                    run = (s, s + best_len, best_off as u16);
                }
            }
            // `s` joins the chain after its own search.
            insert(head, chain, s, h, base);
        }
    }

    /// The shortest path over the matches `search` found, its sequences
    /// left in `seqs`: `m_cost[i]` the bytes before `i` with a match ending
    /// at `i`, and the cheapest price with a run of literals ending at `i`
    /// carried from position to position, the run's length kept in
    /// `p_lit[i]` for the way back.
    #[inline(never)]
    fn shortest(&mut self, input: &[u8], e: Effort) {
        let n = input.len();
        let last_start = n - MFLIMIT;
        self.m_cost.clear();
        self.m_cost.resize(n + 1, INF);
        self.m_from.clear();
        self.m_from.resize(n + 1, (0, 0));
        self.p_lit.clear();
        self.p_lit.resize(n + 1, 0);
        let best = &self.best[..n];
        let m_cost = &mut self.m_cost[..n + 1];
        let m_from = &mut self.m_from[..n + 1];
        let p_lit = &mut self.p_lit[..n + 1];
        m_cost[0] = 0;
        // The end: the last run of literals from a match's end, or the
        // whole group as literals. A match never ends past `limit`, so
        // every run from one is the five the format requires.
        let (mut end, mut cost) = (0usize, lit_run(n));
        // The run of literals ending at the position before: its price
        // and its length. Prices stay far under `INF`, so they add plain.
        let (mut pc, mut pl) = (INF, 0u32);
        for i in 0..=n {
            // Inside a run: a match ending here starts a run of none, or
            // the run before goes on by this position's literal.
            let fresh = m_cost[i];
            if i > 0 && fresh < INF {
                let c = fresh + lit_run(n - i);
                if c < cost || (c == cost && i > end) {
                    cost = c;
                    end = i;
                }
            }
            let carry = if i == 0 { INF } else { pc + 1 + lit_step(pl) };
            if fresh <= carry {
                pc = fresh;
                pl = 0;
            } else {
                pc = carry;
                pl += 1;
            }
            p_lit[i] = pl;
            if i > last_start {
                continue;
            }
            let (longest, off) = best[i];
            let longest = longest as usize;
            if longest < MINMATCH {
                continue;
            }
            // The match before carried on, reached by a literal: the match
            // from the position before covers the same and more for a byte
            // less, and every cut of this one is a cut of that one, so
            // nothing from here can be cheaper.
            if pl > 0 && i > 0 && best[i - 1] == ((longest + 1) as u32, off) {
                continue;
            }
            // A sequence: its token and offset, the run's length bytes
            // (counted in the run's price), the match's length bytes.
            // Strictly cheaper replaces, so that the longest of equal
            // prices, tried first, stands: fewer sequences after it.
            let seq = pc + 3 + e.seq_cost;
            let reach = i + longest;
            let c = seq + ext(longest - MINMATCH);
            if c < m_cost[reach] {
                m_cost[reach] = c;
                m_from[reach] = (i as u32, off);
            }
            // A shorter length pays only where it ends on a position whose
            // own match reaches past this one's end: literals after a cut
            // cost a byte each where the match covered them for nothing,
            // and a cut onto a match that ends inside this one buys that
            // match's sequence for no more cover. And only the last
            // `MINMATCH` positions before the end are asked: a match that
            // starts earlier and reaches past it covers the first of them
            // with more than `MINMATCH` to go, so it is found there, the
            // same cover for the same bytes.
            let stop = reach.min(last_start + 1);
            for j in (i + MINMATCH).max(reach.saturating_sub(MINMATCH))..stop {
                let bl = best[j].0 as usize;
                if bl >= MINMATCH && j + bl > reach {
                    let c = seq + ext(j - i - MINMATCH);
                    if c < m_cost[j] {
                        m_cost[j] = c;
                        m_from[j] = (i as u32, off);
                    }
                }
            }
        }
        // Back from the end: each match's start, the run before it from
        // the literal count its start was reached with.
        let mut e2 = end;
        while e2 > 0 {
            let (s, off) = m_from[e2];
            let s = s as usize;
            let lit = p_lit[s] as usize;
            self.seqs
                .push(((s - lit) as u32, lit as u32, off, (e2 - s) as u32));
            e2 = s - lit;
        }
    }

    /// The chosen sequences and the last run, written as a block.
    fn emit(&self, input: &[u8], out: &mut [u8]) -> usize {
        let mut o = 0;
        let mut pos = 0usize;
        for &(lit_at, lit, off, ml) in self.seqs.iter().rev() {
            let (lit_at, lit, ml) = (lit_at as usize, lit as usize, ml as usize);
            debug_assert_eq!(lit_at, pos);
            let token = (lit.min(15) << 4) as u8 | (ml - MINMATCH).min(15) as u8;
            out[o] = token;
            o += 1;
            o = put_len(out, o, lit);
            out[o..o + lit].copy_from_slice(&input[lit_at..lit_at + lit]);
            o += lit;
            out[o..o + 2].copy_from_slice(&off.to_le_bytes());
            o += 2;
            o = put_len(out, o, ml - MINMATCH);
            pos = lit_at + lit + ml;
        }
        let lit = input.len() - pos;
        out[o] = (lit.min(15) << 4) as u8;
        o += 1;
        o = put_len(out, o, lit);
        out[o..o + lit].copy_from_slice(&input[pos..]);
        o + lit
    }
}

/// Position `s`, whose four bytes hash to `h`, at the head of its chain,
/// linked to the one it displaces. A group is at most [`MAX_INPUT`] bytes,
/// so every position of it is in the window of every later one.
#[inline(always)]
fn insert(head: &mut [u32], chain: &mut [u16], s: usize, h: usize, base: u32) {
    let prev = head[h];
    chain[s] = if prev > base {
        let d = s - (prev - base - 1) as usize;
        debug_assert!(d <= MAX_DISTANCE);
        d as u16
    } else {
        0
    };
    head[h] = base + s as u32 + 1;
}

/// The longest match at `p` among the first `attempts` positions of its
/// chain, after every position from `*ins` to `p` has joined its own: its
/// length, `MINMATCH - 1` where there is none, and its offset. `p` itself
/// joins at the next call.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn longest_at(
    input: &[u8],
    p: usize,
    limit: usize,
    attempts: u32,
    ins: &mut usize,
    head: &mut [u32],
    chain: &mut [u16],
    base: u32,
) -> (usize, usize) {
    while *ins < p {
        insert(head, chain, *ins, hash(read_u32(input, *ins)), base);
        *ins += 1;
    }
    let v = read_u32(input, p);
    let max_len = limit - p;
    let (mut best_len, mut best_off) = (MINMATCH - 1, 0usize);
    let mut cand = head[hash(v)];
    let mut tries = attempts;
    while cand > base && tries > 0 {
        let q = (cand - base - 1) as usize;
        tries -= 1;
        if input[q + best_len] == input[p + best_len] && read_u32(input, q) == v {
            let l = common(input, q, p, max_len);
            if l > best_len {
                best_len = l;
                best_off = p - q;
                if l == max_len {
                    break;
                }
            }
        }
        let c = chain[q];
        if c == 0 {
            break;
        }
        cand -= c as u32;
    }
    (best_len, best_off)
}

/// The longest block an input of `n` bytes can encode to: every byte a
/// literal, a length byte every 255 and the token.
pub fn max_output(n: usize) -> usize {
    n + n / 255 + 16
}

/// The length bytes a length field of `k` past its token's nibble writes.
fn put_len(out: &mut [u8], mut o: usize, k: usize) -> usize {
    if k < 15 {
        return o;
    }
    let mut r = k - 15;
    while r >= 255 {
        out[o] = 255;
        o += 1;
        r -= 255;
    }
    out[o] = r as u8;
    o + 1
}

/// The length bytes a field of `k` costs beyond its nibble.
fn ext(k: usize) -> u32 {
    if k < 15 {
        0
    } else {
        1 + ((k - 15) / 255) as u32
    }
}

/// What a run of `k` literals gains in length bytes by one more literal.
fn lit_step(k: u32) -> u32 {
    ext(k as usize + 1) - ext(k as usize)
}

/// A last sequence of `k` literals: its token, length bytes and bytes.
fn lit_run(k: usize) -> u32 {
    1 + ext(k) + k as u32
}

fn hash(v: u32) -> usize {
    (v.wrapping_mul(2_654_435_761) >> (32 - HASH_LOG)) as usize
}

fn read_u32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().expect("four bytes"))
}

fn read_u64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().expect("eight bytes"))
}

/// The length the bytes at `a` and `b` agree for, `a` before `b`, at most
/// `max`, eight bytes a compare.
fn common(b: &[u8], a: usize, s: usize, max: usize) -> usize {
    let mut l = 0;
    while l + 8 <= max {
        let x = read_u64(b, a + l) ^ read_u64(b, s + l);
        if x != 0 {
            return l + (x.trailing_zeros() / 8) as usize;
        }
        l += 8;
    }
    while l < max && b[a + l] == b[s + l] {
        l += 1;
    }
    l
}

/// A block that is not an LZ4 block, or does not fit the output it was
/// given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError;

/// The output past a block's decoded end the fast loop's wide copies may
/// write into: given this much more room than the block decodes to, the
/// decoder runs its fast loop to the block's last few sequences, where an
/// output cut at the decoded length leaves the last 64 bytes to the
/// careful loop. The bytes written there are not the block's.
pub const DECODE_SLACK: usize = 64;

/// The input a fast turn may read from its token: the token, a move of
/// sixteen literals, and an offset after fewer than fifteen of them, which
/// end inside the sixteen moved and a byte past them. A longer run checks
/// its own.
const FAST_IN: usize = 18;
/// The output a fast turn may write past where it starts: sixteen bytes of
/// literals and an eighteen-byte match, with the same room.
const FAST_OUT: usize = 64;
/// The readable bytes past a block's end that let the fast loop run with no
/// input bound of its own: a turn starts at most a byte past the block's
/// end and reads at most forty-nine past it (a run of sixty-four or fewer
/// literals ending at the block's end moves sixty-four from fifteen before
/// it).
const PAD_IN: usize = 64;

/// `src`, an LZ4 block, decoded into `dst` from its start: the decoded
/// length, or [`DecodeError`] for bytes that are not a block or a block
/// that decodes past `dst`. Any input is safe: every read is of `src` and
/// every write of `dst`, the fast loop's wide copies landing inside `dst`
/// past the decoded end where there is room for them (`DECODE_SLACK`).
///
/// The fast loop is liblz4's: while the input and output have a turn's
/// room, a run of fewer than fifteen literals is one sixteen-byte move and a
/// longer one moves of sixteen to sixty-four and of thirty-two past it, and
/// a match moves sixteen bytes at a time from sixteen or more back, eight
/// from eight or more, and from closer byte by byte for its first eight and
/// then eight at a time from the nearest whole number of its periods eight
/// or more back -- no length in any move but the advance, the moves'
/// overrun landing on what the next sequence writes or in the slack. Where
/// a turn lacks the room, the rest of the block goes the careful way, a
/// sequence at a time with every bound checked.
pub fn decompress_into(src: &[u8], dst: &mut [u8]) -> Result<usize, DecodeError> {
    decompress_in(src, src.len(), dst)
}

/// [`decompress_into`] of the block `src[..len]`, the bytes after it in
/// `src` readable but not the block's: the fast loop's moves may read into
/// them, so that a block followed by others in a page runs the fast loop to
/// its last sequence, the end found after each run of literals, and a block
/// with `PAD_IN` bytes after it runs it with no input bound at all, its
/// length bytes read inside the block. One that ends its buffer leaves its
/// last `FAST_IN` bytes to the careful loop.
pub fn decompress_in(src: &[u8], len: usize, dst: &mut [u8]) -> Result<usize, DecodeError> {
    if len > src.len() {
        return Err(DecodeError);
    }
    let (readable, oend) = (src.len(), dst.len());
    let (mut ip, mut op) = (0usize, 0usize);
    if len > 0 && oend >= FAST_OUT {
        let olim = oend - FAST_OUT;
        // SAFETY: every turn starts at `op <= olim` and, unpadded, at `ip <=
        // ilim`, so it may write `FAST_OUT` bytes of `dst` from where it
        // starts and read `FAST_IN` bytes of `src` from its token; padded, a
        // turn starts at most a byte past the block and reads at most
        // `PAD_IN` past it, inside `src`. A turn that would go further
        // checks its own room first. `src` and `dst` are different buffers,
        // and a match reads `dst` behind what it writes.
        let done = if readable - len >= PAD_IN {
            unsafe { fast_loop::<true>(src, len, dst, &mut ip, &mut op, 0, olim)? }
        } else if readable >= FAST_IN {
            let ilim = (readable - FAST_IN).min(len - 1);
            unsafe { fast_loop::<false>(src, len, dst, &mut ip, &mut op, ilim, olim)? }
        } else {
            false
        };
        if done {
            return Ok(op);
        }
    }
    let block = &src[..len];
    while ip < len {
        if careful_sequence(block, dst, &mut ip, &mut op)? {
            return Ok(op);
        }
    }
    Err(DecodeError)
}

/// The fast loop over the block `src[..len]`: turns while `op <= olim` and,
/// unless `PADDED`, `ip <= ilim`; `true` where the block ended in it, else
/// the rest left to the careful loop where a turn lacked its room. Every
/// length byte is read inside the block, so a turn starts at most a byte
/// past it.
///
/// # Safety
/// `olim + FAST_OUT == dst.len()`, `len <= src.len()`, and either `PADDED`
/// and `len + PAD_IN <= src.len()`, or `ilim + FAST_IN <= src.len()` and
/// `ilim < len`.
#[inline(always)]
unsafe fn fast_loop<const PADDED: bool>(
    src: &[u8],
    len: usize,
    dst: &mut [u8],
    ipr: &mut usize,
    opr: &mut usize,
    ilim: usize,
    olim: usize,
) -> Result<bool, DecodeError> {
    let (iend, oend) = (src.len(), dst.len());
    let block = &src[..len];
    let sp = src.as_ptr();
    let dp = dst.as_mut_ptr();
    // A long match's room: thirty-two bytes of moves past its end.
    let mlim = oend - 32;
    let (mut ip, mut op) = (*ipr, *opr);
    // SAFETY, for the whole loop: the caller's room, and each move's source
    // in `dst` ends at or before its destination starts (`offset >= 8` for
    // a move of eight, `>= 16` for sixteen, `step >= 8` in `near_copy`).
    unsafe {
        while op <= olim && (PADDED || ip <= ilim) {
            let token = *sp.add(ip);
            ip += 1;
            // The run. Under fifteen literals, one move of sixteen inside
            // the turn's room; a long one, its length bytes read inside the
            // block and its room checked, moves of sixteen to sixty-four --
            // a form group's long runs are nearly all under that -- and of
            // thirty-two past it.
            let mut lit = (token >> 4) as usize;
            if lit < 15 {
                std::ptr::copy_nonoverlapping(sp.add(ip), dp.add(op), 16);
            } else {
                let at = ip - 1;
                lit += var_len(block, &mut ip)?;
                // Past the block is not a run; past the output's room, or
                // the input's where nothing follows the block, is the
                // careful loop's, from the token.
                if lit > len - ip {
                    return Err(DecodeError);
                }
                // Unpadded, sixty-four moved from the run's start and, past
                // its end, an offset and the thirty-one a move of thirty-two
                // may overrun by.
                if lit > olim - op || (!PADDED && (iend - ip < 64 || iend - ip - lit < 32)) {
                    ip = at;
                    break;
                }
                if lit <= 64 {
                    std::ptr::copy_nonoverlapping(sp.add(ip), dp.add(op), 16);
                    std::ptr::copy_nonoverlapping(sp.add(ip + 16), dp.add(op + 16), 16);
                    std::ptr::copy_nonoverlapping(sp.add(ip + 32), dp.add(op + 32), 16);
                    std::ptr::copy_nonoverlapping(sp.add(ip + 48), dp.add(op + 48), 16);
                } else {
                    wild_copy::<32>(sp.add(ip), dp.add(op), lit);
                }
            }
            ip += lit;
            op += lit;
            // The block's last sequence is literals alone.
            if ip >= len {
                *ipr = ip;
                *opr = op;
                return if ip == len {
                    Ok(true)
                } else {
                    Err(DecodeError)
                };
            }
            let offset = u16::from_le(std::ptr::read_unaligned(sp.add(ip).cast::<u16>())) as usize;
            ip += 2;
            // Zero or past the output's start, in one compare.
            if offset.wrapping_sub(1) >= op {
                return Err(DecodeError);
            }
            let m = op - offset;
            // The match. Under nineteen bytes, eighteen in moves of eight,
            // eight and two, inside the room the run left; a long one, its
            // length read and its room checked, thirty-two in two moves of
            // sixteen -- most of a form group's long matches -- and sixteen
            // at a time past that.
            let ml_n = (token & 15) as usize;
            if ml_n < 15 {
                let ml = ml_n + MINMATCH;
                if offset >= 8 {
                    std::ptr::copy_nonoverlapping(dp.add(m), dp.add(op), 8);
                    std::ptr::copy_nonoverlapping(dp.add(m + 8), dp.add(op + 8), 8);
                    std::ptr::copy_nonoverlapping(dp.add(m + 16), dp.add(op + 16), 2);
                } else {
                    near_copy(dp, m, op, ml, offset);
                }
                op += ml;
                continue;
            }
            let ml = 15 + MINMATCH + var_len(block, &mut ip)?;
            // `op` is under `mlim`: a short run leaves it within thirty-two
            // of where the turn began.
            if ml > mlim - op {
                // A match past the turn's room: copied with its bounds
                // checked, and the rest of the block the careful way. The
                // raw pointers are not used after `dst` is.
                match_checked(dst, op, offset, ml)?;
                op += ml;
                break;
            }
            if offset >= 16 {
                std::ptr::copy_nonoverlapping(dp.add(m), dp.add(op), 16);
                std::ptr::copy_nonoverlapping(dp.add(m + 16), dp.add(op + 16), 16);
                if ml > 32 {
                    let (mut s, mut d) = (dp.add(m + 32), dp.add(op + 32));
                    let end = dp.add(op + ml);
                    loop {
                        std::ptr::copy_nonoverlapping(s, d, 16);
                        s = s.add(16);
                        d = d.add(16);
                        if d >= end {
                            break;
                        }
                    }
                }
            } else if offset >= 8 {
                let (mut s, mut d) = (dp.add(m), dp.add(op));
                let end = dp.add(op + ml);
                loop {
                    std::ptr::copy_nonoverlapping(s, d, 8);
                    std::ptr::copy_nonoverlapping(s.add(8), d.add(8), 8);
                    s = s.add(16);
                    d = d.add(16);
                    if d >= end {
                        break;
                    }
                }
            } else {
                near_copy(dp, m, op, ml, offset);
            }
            op += ml;
        }
    }
    *ipr = ip;
    *opr = op;
    Ok(false)
}

/// `len` bytes from `s` to `d` in moves of `N`, the last move's overrun
/// written past `d + len`: at least one move, at most `len + N - 1` bytes.
/// Out of line: it serves the rare long run, and inline its loop (which the
/// compiler makes a call to `memcpy`) costs the fast loop's every turn the
/// registers the call needs.
///
/// # Safety
/// `s` readable and `d` writable for `len + N - 1` bytes, and every move's
/// source before its destination by at least `N` where the two are one
/// buffer.
#[inline(never)]
unsafe fn wild_copy<const N: usize>(mut s: *const u8, mut d: *mut u8, len: usize) {
    // SAFETY: the caller's.
    unsafe {
        let end = d.add(len);
        loop {
            std::ptr::copy_nonoverlapping(s, d, N);
            s = s.add(N);
            d = d.add(N);
            if d >= end {
                break;
            }
        }
    }
}

/// A match from fewer than eight bytes back: its first eight bytes one at a
/// time, each read after the byte it repeats is written, then moves of
/// eight from `step` back, the least multiple of the period of eight or
/// more, where the bytes repeat as they do one period back.
///
/// Out of line, as the rare case it is.
///
/// # Safety
/// `dp` writable for `op + ml + 7` bytes and `m = op - offset`, `0 < offset
/// < 8`.
#[inline(never)]
unsafe fn near_copy(dp: *mut u8, m: usize, op: usize, ml: usize, offset: usize) {
    // SAFETY: the caller's.
    unsafe {
        for k in 0..8 {
            *dp.add(op + k) = *dp.add(m + k);
        }
        // The least multiple of the period of eight or more, by the
        // period: no divide on the path.
        const STEP: [usize; 8] = [8, 8, 8, 9, 8, 10, 12, 14];
        let step = STEP[offset & 7];
        let mut k = 8;
        while k < ml {
            std::ptr::copy_nonoverlapping(dp.add(op + k - step), dp.add(op + k), 8);
            k += 8;
        }
    }
}

/// A match of `ml` from `offset` back copied with its bounds checked.
fn match_checked(dst: &mut [u8], op: usize, offset: usize, ml: usize) -> Result<(), DecodeError> {
    if ml > dst.len() - op {
        return Err(DecodeError);
    }
    let m = op - offset;
    if offset >= ml {
        let (a, b) = dst.split_at_mut(op);
        b[..ml].copy_from_slice(&a[m..m + ml]);
    } else {
        copy_overlap(dst, m, op, ml);
    }
    Ok(())
}

/// `n` bytes of `dst` from `from` to `to` where the match overlaps its own
/// output, `to - from` smaller than `n`: the bytes behind `to` repeat with
/// that period, so each move copies as much of the repeat as stands
/// already written, doubling, and none reads what it writes.
#[inline(always)]
fn copy_overlap(dst: &mut [u8], from: usize, to: usize, n: usize) {
    let period = to - from;
    let (mut done, mut span) = (0usize, period);
    while done < n {
        let k = span.min(n - done);
        dst.copy_within(to + done - span..to + done - span + k, to + done);
        done += k;
        span = (span * 2).min(done + period);
    }
}

/// One sequence decoded with every bound checked: `true` for the block's
/// last, the literals that end it.
#[inline(never)]
fn careful_sequence(
    src: &[u8],
    dst: &mut [u8],
    ip: &mut usize,
    op: &mut usize,
) -> Result<bool, DecodeError> {
    let (iend, oend) = (src.len(), dst.len());
    let token = *src.get(*ip).ok_or(DecodeError)?;
    *ip += 1;
    let mut lit = (token >> 4) as usize;
    if lit == 15 {
        lit += var_len(src, ip)?;
    }
    if lit > iend - *ip || lit > oend - *op {
        return Err(DecodeError);
    }
    dst[*op..*op + lit].copy_from_slice(&src[*ip..*ip + lit]);
    *ip += lit;
    *op += lit;
    if *ip == iend {
        return Ok(true);
    }
    if iend - *ip < 2 {
        return Err(DecodeError);
    }
    let offset = u16::from_le_bytes([src[*ip], src[*ip + 1]]) as usize;
    *ip += 2;
    if offset == 0 || offset > *op {
        return Err(DecodeError);
    }
    let mut ml = (token & 15) as usize + MINMATCH;
    if ml == 15 + MINMATCH {
        ml += var_len(src, ip)?;
    }
    match_checked(dst, *op, offset, ml)?;
    *op += ml;
    Ok(false)
}

/// A length's continuation bytes: each adds its value, and one under 255
/// ends it.
#[inline(always)]
fn var_len(src: &[u8], ip: &mut usize) -> Result<usize, DecodeError> {
    let mut n = 0usize;
    loop {
        let b = *src.get(*ip).ok_or(DecodeError)?;
        *ip += 1;
        n += b as usize;
        if b != 255 {
            return Ok(n);
        }
    }
}

/// The sequences of an LZ4 block: how many tokens a decoder parses for it,
/// or `None` for bytes that are not a block.
pub fn sequences(block: &[u8]) -> Option<usize> {
    let (mut i, mut n) = (0usize, 0usize);
    loop {
        let token = *block.get(i)?;
        i += 1;
        n += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            loop {
                let b = *block.get(i)?;
                i += 1;
                lit += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        i = i.checked_add(lit)?;
        if i == block.len() {
            return Some(n);
        }
        if i + 2 > block.len() {
            return None;
        }
        i += 2;
        if token & 15 == 15 {
            loop {
                let b = *block.get(i)?;
                i += 1;
                if b != 255 {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small generator, so the cases are the same every run.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn roundtrip(enc: &mut Encoder, input: &[u8], e: Effort) -> usize {
        let mut out = vec![0u8; max_output(input.len())];
        let n = enc.compress_into(input, &mut out, e).expect("room");
        let mut back = vec![0u8; input.len()];
        let got = lz4_flex::block::decompress_into(&out[..n], &mut back)
            .unwrap_or_else(|err| panic!("{} bytes: not a block: {err:?}", input.len()));
        assert_eq!(got, input.len(), "decoded length");
        assert!(back == input, "{} bytes: decoded differently", input.len());
        assert!(n <= max_output(input.len()));
        let seqs = sequences(&out[..n]).expect("a block's sequences");
        assert!(seqs >= 1);
        n
    }

    /// Inputs of every shape a group can take and some it cannot: random
    /// bytes, a small alphabet, periodic patterns of every period to past
    /// the window's cut lengths, long runs, records of a key and a value in
    /// the suite's shape, and every length a block's end rules bite at.
    fn cases() -> Vec<Vec<u8>> {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        let mut v: Vec<Vec<u8>> = Vec::new();
        for n in 0..40 {
            v.push((0..n).map(|i| (i % 3) as u8).collect());
            v.push((0..n).map(|_| r.below(256) as u8).collect());
            v.push(vec![7u8; n]);
        }
        for &n in &[64usize, 255, 256, 270, 271, 1000, 2048, 4096, 9000] {
            v.push((0..n).map(|_| r.below(256) as u8).collect());
            v.push((0..n).map(|_| r.below(4) as u8).collect());
            v.push(vec![0u8; n]);
            for period in [1usize, 2, 3, 5, 8, 17, 40, 300] {
                v.push((0..n).map(|i| ((i % period) * 37) as u8).collect());
            }
        }
        // Records: sixteen-byte ascending keys and values of a hundred
        // bytes, half of each value from a small alphabet.
        let mut rec = Vec::new();
        for k in 0u64..200 {
            rec.extend_from_slice(format!("{k:016}").as_bytes());
            for j in 0..100 {
                rec.push(if j % 2 == 0 {
                    r.below(256) as u8
                } else {
                    b'a' + r.below(4) as u8
                });
            }
        }
        v.push(rec.clone());
        v.push(rec[..2048].to_vec());
        v.push(
            (0..MAX_INPUT)
                .map(|i| (r.below(3) as usize + i / 1000) as u8)
                .collect(),
        );
        v.push(vec![0u8; MAX_INPUT]);
        v
    }

    #[test]
    fn every_case_decodes_to_its_input_under_every_effort() {
        let mut enc = Encoder::new();
        for e in [
            Effort {
                attempts: 0,
                seq_cost: 0,
                skip_min: 8,
                lazy: 0,
            },
            Effort {
                attempts: 1,
                seq_cost: 0,
                skip_min: 8,
                lazy: 0,
            },
            Effort::SHIPPED,
            Effort {
                attempts: 512,
                seq_cost: 0,
                skip_min: 8,
                lazy: 0,
            },
            Effort {
                attempts: 64,
                seq_cost: 6,
                skip_min: 8,
                lazy: 0,
            },
            Effort {
                lazy: 1,
                ..Effort::SHIPPED
            },
            Effort {
                lazy: 2,
                ..Effort::SHIPPED
            },
            Effort {
                attempts: 1,
                lazy: 3,
                ..Effort::SHIPPED
            },
        ] {
            for c in cases() {
                roundtrip(&mut enc, &c, e);
            }
        }
    }

    #[test]
    fn the_tables_kept_across_groups_name_no_position_of_an_earlier_group() {
        // The same encoder over groups that share their bytes with the
        // group before at every offset: a stale head taken for a position
        // would be a match into bytes this group does not have.
        let mut enc = Encoder::new();
        let mut r = Rng(42);
        let pool: Vec<u8> = (0..20_000).map(|_| r.below(8) as u8).collect();
        for g in 0..500 {
            let at = r.below(10_000) as usize;
            let len = 13 + r.below(4000) as usize;
            let input = &pool[at..at + len];
            roundtrip(&mut enc, input, Effort::SHIPPED);
            if g % 97 == 0 {
                // And the base moved to just short of its wrap, so the
                // clear is reached.
                enc.base = u32::MAX - 2 * MAX_INPUT as u32;
            }
        }
    }

    #[test]
    fn a_group_compresses_to_no_more_than_the_greedy_encoder_and_in_fewer_sequences() {
        // Over the compressible cases the parse never writes more bytes
        // than `lz4_flex`'s greedy encoder, by more than a block's
        // rounding, and over all of them in sum fewer bytes and fewer
        // sequences.
        let mut enc = Encoder::new();
        let (mut ours_b, mut flex_b, mut ours_s, mut flex_s) = (0, 0, 0, 0);
        for c in cases() {
            if c.len() < 64 {
                continue;
            }
            let n = roundtrip(&mut enc, &c, Effort::SHIPPED);
            let f = lz4_flex::block::compress(&c);
            let mut out = vec![0u8; max_output(c.len())];
            let n2 = enc.compress_into(&c, &mut out, Effort::SHIPPED).unwrap();
            assert_eq!(n, n2, "the same input, the same bytes");
            assert!(
                n <= f.len() + 2,
                "{} bytes: {n} against {}",
                c.len(),
                f.len()
            );
            ours_b += n;
            flex_b += f.len();
            ours_s += sequences(&out[..n]).unwrap();
            flex_s += sequences(&f).unwrap();
        }
        assert!(ours_b < flex_b, "bytes {ours_b} against {flex_b}");
        assert!(ours_s < flex_s, "sequences {ours_s} against {flex_s}");
    }

    #[test]
    fn a_price_on_sequences_trades_bytes_for_fewer_of_them() {
        let mut enc = Encoder::new();
        let mut r = Rng(7);
        let mut rec = Vec::new();
        for k in 0u64..20 {
            rec.extend_from_slice(format!("{k:016}").as_bytes());
            for j in 0..100 {
                rec.push(if j % 2 == 0 {
                    r.below(256) as u8
                } else {
                    b'a' + r.below(4) as u8
                });
            }
        }
        let mut out = vec![0u8; max_output(rec.len())];
        let plain = enc.compress_into(&rec, &mut out, Effort::SHIPPED).unwrap();
        let s_plain = sequences(&out[..plain]).unwrap();
        let priced = enc
            .compress_into(
                &rec,
                &mut out,
                Effort {
                    attempts: 16,
                    seq_cost: 8,
                    skip_min: 8,
                    lazy: 0,
                },
            )
            .unwrap();
        let s_priced = sequences(&out[..priced]).unwrap();
        assert!(s_priced <= s_plain, "{s_priced} against {s_plain}");
        assert!(priced >= plain, "{priced} against {plain}");
    }

    /// `block` decoded by this module into an output of exactly `n` bytes
    /// and into one with the slack the fast loop runs to the end in, both
    /// held to `want`.
    fn decodes_to(block: &[u8], want: &[u8]) {
        let mut exact = vec![0u8; want.len()];
        assert_eq!(decompress_into(block, &mut exact), Ok(want.len()), "exact");
        assert!(
            exact == want,
            "{} bytes: decoded differently, exact",
            want.len()
        );
        let mut roomy = vec![0xA5u8; want.len() + DECODE_SLACK + 7];
        assert_eq!(decompress_into(block, &mut roomy), Ok(want.len()), "roomy");
        assert!(
            &roomy[..want.len()] == want,
            "{} bytes: decoded differently, roomy",
            want.len()
        );
        // The block followed by bytes that are not its own, as a payload is
        // by the rest of its page.
        for tail in [1usize, 17, 18, 31, 40, 63, 64, 200] {
            let mut src = block.to_vec();
            src.extend((0..tail).map(|i| (i * 131 + 7) as u8));
            for room in [want.len(), want.len() + DECODE_SLACK] {
                let mut out = vec![0x5Au8; room];
                assert_eq!(
                    decompress_in(&src, block.len(), &mut out),
                    Ok(want.len()),
                    "tail {tail}, room {room}"
                );
                assert!(
                    &out[..want.len()] == want,
                    "{} bytes: decoded differently, tail {tail}, room {room}",
                    want.len()
                );
            }
        }
    }

    /// The cases a run under Miri can interpret in minutes: the short ones,
    /// which reach every path of the decoder the long ones do.
    fn decode_cases() -> impl Iterator<Item = Vec<u8>> {
        cases()
            .into_iter()
            .filter(|c| !cfg!(miri) || c.len() <= 2048)
    }

    #[test]
    fn every_block_either_encoder_writes_decodes_to_its_input() {
        let mut enc = Encoder::new();
        for c in decode_cases() {
            for e in [
                Effort::SHIPPED,
                Effort {
                    attempts: 0,
                    seq_cost: 0,
                    skip_min: 8,
                    lazy: 0,
                },
            ] {
                let mut out = vec![0u8; max_output(c.len())];
                let n = enc.compress_into(&c, &mut out, e).unwrap();
                decodes_to(&out[..n], &c);
            }
            decodes_to(&lz4_flex::block::compress(&c), &c);
        }
    }

    #[test]
    fn a_block_that_decodes_past_its_output_is_refused() {
        let input: Vec<u8> = (0..3000).map(|i| (i % 13) as u8).collect();
        let b = lz4_flex::block::compress(&input);
        for short in [0usize, 1, 100, 2999] {
            let mut dst = vec![0u8; short];
            assert_eq!(decompress_into(&b, &mut dst), Err(DecodeError), "{short}");
        }
        assert_eq!(decompress_into(&[], &mut [0u8; 10]), Err(DecodeError));
        assert_eq!(
            decompress_in(&b, b.len() + 1, &mut [0u8; 4000]),
            Err(DecodeError)
        );
        let mut cut = b.clone();
        cut.extend([0u8; 64]);
        assert_eq!(decompress_in(&cut, 0, &mut [0u8; 4000]), Err(DecodeError));
    }

    #[test]
    fn a_damaged_block_never_reads_or_writes_outside_its_buffers_and_agrees_where_both_decode() {
        // Every block damaged a byte at a time, cut short, and random bytes:
        // the decoder returns, never panics, and where it and `lz4_flex`
        // both decode the bytes they agree on what they decode to.
        let mut r = Rng(0xDEC0DE);
        let mut enc = Encoder::new();
        let mut blocks = Vec::new();
        for c in decode_cases() {
            if c.len() < 20 || c.len() > 10_000 {
                continue;
            }
            let mut out = vec![0u8; max_output(c.len())];
            let n = enc.compress_into(&c, &mut out, Effort::SHIPPED).unwrap();
            blocks.push((out[..n].to_vec(), c.len()));
            blocks.push((lz4_flex::block::compress(&c), c.len()));
        }
        let (mut both, mut tried) = (0, 0);
        for (b, ulen) in &blocks {
            for t in 0..if cfg!(miri) { 8 } else { 40 } {
                let mut d = b.clone();
                match t % 4 {
                    0 => {
                        let i = r.below(d.len() as u64) as usize;
                        d[i] ^= 1 << r.below(8);
                    }
                    1 => {
                        let i = r.below(d.len() as u64) as usize;
                        d[i] = r.below(256) as u8;
                    }
                    2 => d.truncate(r.below(d.len() as u64) as usize),
                    _ => {
                        let n = r.below(64) as usize;
                        d = (0..n).map(|_| r.below(256) as u8).collect();
                    }
                }
                for room in [*ulen, ulen + DECODE_SLACK] {
                    tried += 1;
                    let mut ours = vec![0u8; room];
                    let mut theirs = vec![0u8; room];
                    let a = decompress_into(&d, &mut ours);
                    // `lz4_flex`'s decode of a damaged block can offset its
                    // output pointer two past the output's end when the
                    // output is the whole allocation (`duplicate_overlapping`,
                    // `while output_ptr.add(1) < dst_ptr_end`), which Miri
                    // refuses though nothing is read or written there, so a
                    // run under Miri holds this decoder to itself alone.
                    let f =
                        (!cfg!(miri)).then(|| lz4_flex::block::decompress_into(&d, &mut theirs));
                    if let (Ok(n), Some(Ok(m))) = (a, f) {
                        both += 1;
                        assert_eq!(n, m, "the decoded lengths");
                        assert!(ours[..n] == theirs[..m], "the decoded bytes");
                    }
                    // The same bytes with a readable tail decode the same,
                    // unpadded and padded: a tail of 255s is what a length
                    // read past the block would take for a continuation.
                    for (tail, fill) in [
                        (17usize, 0xEEu8),
                        (48, 0xEE),
                        (48, 0xFF),
                        (80, 0xEE),
                        (80, 0xFF),
                    ] {
                        let mut padded = d.clone();
                        padded.extend(std::iter::repeat_n(fill, tail));
                        let mut tailed = vec![0u8; room];
                        let t = decompress_in(&padded, d.len(), &mut tailed);
                        assert_eq!(
                            t.is_ok(),
                            a.is_ok(),
                            "a tail changes no verdict: {tail} of {fill:#x}"
                        );
                        if let (Ok(n), Ok(m)) = (a, t) {
                            assert_eq!(n, m);
                            assert!(ours[..n] == tailed[..m], "a tail changes no byte");
                        }
                    }
                }
            }
        }
        assert!(
            cfg!(miri) || both > tried / 20,
            "the damage left some blocks decodable: {both} of {tried}"
        );
    }

    #[test]
    fn the_sequence_count_reads_a_block_and_refuses_a_cut_one() {
        let input: Vec<u8> = (0..500).map(|i| (i % 7) as u8).collect();
        let b = lz4_flex::block::compress(&input);
        let s = sequences(&b).expect("a block");
        assert!(s >= 2);
        assert_eq!(sequences(&b[..b.len() - 1]), None);
        assert_eq!(sequences(&[0x00]), Some(1));
    }

    #[test]
    fn an_output_short_of_the_worst_case_or_an_input_past_the_largest_group_is_refused() {
        let mut enc = Encoder::new();
        let input = vec![1u8; 100];
        let mut out = vec![0u8; max_output(100) - 1];
        assert_eq!(enc.compress_into(&input, &mut out, Effort::SHIPPED), None);
        let big = vec![0u8; MAX_INPUT + 1];
        let mut out = vec![0u8; max_output(big.len())];
        assert_eq!(enc.compress_into(&big, &mut out, Effort::SHIPPED), None);
    }
}
