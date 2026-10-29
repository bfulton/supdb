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
}

impl Effort {
    /// The search the form ships with: sixteen chain positions, a
    /// sequence priced at its bytes alone, and the inside of a match of
    /// eight or more not searched. Priced on the form's own groups
    /// (`bench/examples/zdecode.rs`, `--rhc`): the groups' bytes at 0.96
    /// of the greedy encoder's and their sequences at 0.84, as liblz4's
    /// high-compression mode writes them; sixty-four positions buy a
    /// fiftieth of a percent, and a price on sequences buys fewer of them
    /// and a slower decode, the literals they leave costing more.
    pub const SHIPPED: Effort = Effort {
        attempts: 16,
        seq_cost: 0,
        skip_min: 8,
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
        self.search(input, e);
        self.shortest(input, e);
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
/// linked to the one it displaces where that is in the window.
fn insert(head: &mut [u32], chain: &mut [u16], s: usize, h: usize, base: u32) {
    let prev = head[h];
    chain[s] = if prev > base && s - (prev - base - 1) as usize <= MAX_DISTANCE {
        (s - (prev - base - 1) as usize) as u16
    } else {
        0
    };
    head[h] = base + s as u32 + 1;
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
            },
            Effort {
                attempts: 1,
                seq_cost: 0,
                skip_min: 8,
            },
            Effort::SHIPPED,
            Effort {
                attempts: 512,
                seq_cost: 0,
                skip_min: 8,
            },
            Effort {
                attempts: 64,
                seq_cost: 6,
                skip_min: 8,
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
                },
            )
            .unwrap();
        let s_priced = sequences(&out[..priced]).unwrap();
        assert!(s_priced <= s_plain, "{s_priced} against {s_plain}");
        assert!(priced >= plain, "{priced} against {plain}");
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
