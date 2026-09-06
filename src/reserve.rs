//! What a segment's head reserve needs, computed rather than guessed.
//!
//! `SegmentWriter::set_head_reserve` leaves room after the superblock page for
//! the block table, the key section's checksum row, and copies of the fence
//! and the directory, so a reader whose first probe covers the reserve opens
//! and plans without a second round trip. The size has to be chosen before the
//! first key is written, which is why it used to be a guess with a floor under
//! it -- and a floor is wrong in both directions. Too small and the pieces
//! that do not fit go after the data, which costs the sparse reader a round
//! trip; too large and every segment carries zeroes it will never use. Neither
//! is an error, and that is the point: a wrong reserve is visible as size or
//! as latency, never as a fault, which is the shape of defect this repository
//! keeps a list of.
//!
//! So the size is computed. [`Planner`] accumulates the answer one key at a
//! time, holding aggregates rather than the input, for a caller that streams
//! its records and will not hold them. [`for_lengths`] is that planner over a
//! slice, for a caller that has one. [`from_totals`] answers with an upper
//! bound for a caller that knows only totals. All three return the
//! [`Reserve`] broken into its four pieces, because the last of them is a
//! decision: the directory copy costs four bytes a key and buys a lookup that
//! plans with no second wave, and only the caller knows whether its readers
//! are paying for round trips or for bytes.
//!
//! **None of the layout arithmetic lives here.** Where the key section's
//! regions land comes from [`crate::flatindex::section_layout`], which
//! `plan_inline` calls after counting and the planner calls after
//! accumulating; the block table's size comes from
//! [`crate::flatindex::block_table_len`], which `encode_blocks` allocates by;
//! a record's bytes come from `flatindex::record_len_tail` and the fence's
//! stride from `flatindex::fence_stride`. What is left here is the cut into
//! blocks, which is four lines of the writer's own rule. A second copy of any
//! of that would be a second definition of the format, and the two would
//! drift the first time one of them was edited.

use crate::flatindex;

/// The bytes a key's values encode to, which is what a block holds and what
/// an inline run puts in the record.
///
/// The rule is `index::encode_run`'s: values that all share one non-zero
/// width are stored with no per-value prefix, and anything else takes a
/// varint length before each value.
pub fn run_len(value_lens: &[u32]) -> usize {
    let n = value_lens.len();
    let fixed = n > 0 && value_lens[0] > 0 && value_lens.iter().all(|&l| l == value_lens[0]);
    if fixed {
        return n * value_lens[0] as usize;
    }
    value_lens
        .iter()
        .map(|&l| uvarint_len(l as u64) + l as usize)
        .sum()
}

fn uvarint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// How many blocks a sequence of runs is cut into.
///
/// The rule is the writer's, and it is the whole of it: a run that does not
/// fit beside what is staged starts a new block, and a builder at or over the
/// block size is flushed after the push. So a run larger than a block is a
/// block by itself and a key's values stay contiguous. Inline runs are not
/// passed here -- they never reach a block.
fn blocks_for(runs: impl Iterator<Item = usize>, block_size: usize) -> usize {
    let mut staged = 0usize;
    let mut blocks: usize = runs.map(|n| cut(&mut staged, n, block_size)).sum();
    if staged != 0 {
        blocks += 1;
    }
    blocks
}

/// One run through the cut, as the writer does it: a run that does not fit
/// beside what is staged closes a block first, and a builder at or over the
/// block size is flushed after the push. Returns how many blocks closed, and
/// leaves what is still staged in `staged` -- which the last block takes.
fn cut(staged: &mut usize, n: usize, block_size: usize) -> usize {
    let mut closed = 0;
    // `staged + n > block_size`, without the sum: what is staged is always
    // below the block size, and the sum is what would overflow first where a
    // usize is 32 bits.
    if *staged != 0 && n > block_size - *staged {
        closed += 1;
        *staged = 0;
    }
    *staged = staged.saturating_add(n);
    if *staged >= block_size {
        closed += 1;
        *staged = 0;
    }
    closed
}

/// The longest run this planner will size, which is the longest one the
/// writer will store: an extent addresses its run with a `u32`, and a run
/// past that is refused rather than written. The headroom below that limit is
/// for the record framing's own 4-byte rounding, and it matters where a usize
/// is 32 bits and the limit is `usize::MAX` -- there the rounding is what
/// overflows, not the length.
const MAX_RUN: usize = (u32::MAX as usize) - 8;

/// The reserve, accumulated one key at a time.
///
/// This is the shape a caller wants who will not hold their records: the
/// answer depends on the key count, the record bytes, how the runs cut into
/// blocks, and the lengths of the keys the fence samples -- all of which are
/// aggregates. So a first pass over lengths alone, with no values retained,
/// is enough, and the second pass streams the records through
/// [`crate::SegmentWriter::create_with`] with the reserve already known.
///
/// What it holds is one `u32` per sixteenth key, and nothing else that grows.
/// The fence samples every `stride`-th key and `stride` is not known until the
/// count is, but every stride the format can choose is a power of two at or
/// above the smallest one, so every sampled key is a multiple of that
/// smallest stride and keeping those is enough. At ten million keys that is
/// about 2.5 MB, against the 160 MB a slice of every key's lengths takes on a
/// 64-bit target and the gigabyte the records themselves would.
pub struct Planner {
    block_size: usize,
    inline_max: usize,
    keys: usize,
    rec_bytes: usize,
    staged: usize,
    blocks: usize,
    /// Key lengths at every `sample_stride`-th key, for the fence.
    sampled: Vec<u32>,
    sample_stride: usize,
    /// Cleared when the input cannot be a segment, so `finish` says so.
    viable: bool,
}

impl Planner {
    /// `block_size` and `inline_max` are the writer's, and must be the ones
    /// it will be given.
    pub fn new(block_size: usize, inline_max: usize) -> Planner {
        Planner {
            block_size,
            inline_max,
            keys: 0,
            rec_bytes: 0,
            staged: 0,
            blocks: 0,
            sampled: Vec::new(),
            // The smallest stride the fence can choose. Asking the format
            // rather than restating it: every larger stride is a power of two
            // multiple of this one, so a key the fence samples is always one
            // of these.
            sample_stride: flatindex::fence_stride(0),
            viable: true,
        }
    }

    /// One key, by its key length and the bytes its values encode to.
    /// [`run_len`] turns a key's value lengths into the second. Keys must
    /// arrive in the order they will be written, which is key order.
    pub fn push(&mut self, key_len: usize, run_len: usize) {
        // A key the writer cannot frame with a u16 length, or a run it cannot
        // address with a u32 extent, is refused here too: a planner that
        // returned a number for a segment that cannot exist would be sizing a
        // reserve for a file nobody can write.
        if key_len > u16::MAX as usize || run_len > MAX_RUN {
            self.viable = false;
            return;
        }
        if self.keys.is_multiple_of(self.sample_stride) {
            self.sampled.push(key_len as u32);
        }
        let inline = self.inline_max > 0 && run_len <= self.inline_max;
        let tail = if inline { run_len } else { 0 };
        // One extent per key: that is what a segment writes. Checked, as
        // `plan_inline` checks the same sum -- unchecked it would wrap where
        // a usize is 32 bits and hand back a reserve for the wrapped total,
        // which is a wrong number rather than a refusal.
        match self
            .rec_bytes
            .checked_add(flatindex::record_len_tail(key_len, 1, tail))
        {
            Some(n) => self.rec_bytes = n,
            None => {
                self.viable = false;
                return;
            }
        }
        if !inline {
            self.blocks += cut(&mut self.staged, run_len, self.block_size);
        }
        self.keys += 1;
    }

    /// How many keys have been pushed.
    pub fn keys(&self) -> usize {
        self.keys
    }

    /// The bytes this planner holds that grow with the input: the sampled key
    /// lengths, and nothing else. One `u32` per sixteenth key, so a caller
    /// sizing a first pass can check the claim rather than trust it.
    pub fn retained_bytes(&self) -> usize {
        self.sampled.len() * std::mem::size_of::<u32>()
    }

    /// The reserve for what has been pushed so far.
    ///
    /// Exact but for the checksum row, which can be twelve bytes over: the
    /// row covers the key section in pieces cut on the *object's* pages, so
    /// its length depends on where the section lands, which depends on this
    /// answer. It is taken at its worst alignment, where the section starts
    /// one byte before a page boundary and cuts one piece more than it
    /// otherwise would. That is four bytes, and the 8-aligned boundary behind
    /// the row can move by eight because of them. Nothing else rounds.
    ///
    /// `None` when what was pushed cannot be a segment: a key over 64 KiB, or
    /// a key section past the flat index's limits. The writer would refuse it
    /// too.
    pub fn finish(&self) -> Option<Reserve> {
        if !self.viable {
            return None;
        }
        let stride = flatindex::fence_stride(self.keys);
        let fence_n = self.keys.div_ceil(stride);
        let mut fence_blob_len = 0usize;
        for i in 0..fence_n {
            // Every sampled key is a multiple of `sample_stride`, so it is in
            // hand; if it ever is not, the format changed under this.
            let at = (i * stride) / self.sample_stride;
            fence_blob_len = fence_blob_len.checked_add(*self.sampled.get(at)? as usize)?;
        }
        let layout = flatindex::section_layout(
            self.keys,
            self.rec_bytes,
            fence_n,
            fence_blob_len,
            // No insert room and no record slack: a segment is never edited
            // in place, which is exactly how the writer plans it.
            0,
            false,
        )?;

        let mut blocks = self.blocks;
        if self.staged != 0 {
            blocks += 1;
        }
        let table = flatindex::block_table_len(blocks);
        let row = flatindex::checksum_row_len(
            layout.total,
            flatindex::PIECE_SHIFT,
            (1u64 << flatindex::PIECE_SHIFT) - 1,
        );
        // The fence copy is the span the reader takes: from the offset array
        // to the record region, which is what `fence_span` reports.
        let fence = if fence_n == 0 {
            0
        } else {
            layout.recs_off - layout.fence_offs_off
        };
        Some(Reserve {
            table,
            row,
            fence,
            directory: self.keys * 4,
        })
    }
}

/// What a segment's reserve holds, in bytes, piece by piece.
///
/// The order is the writer's: the table first, then the checksum row, the
/// fence and the directory, each 8-aligned and each placed only if what is
/// left holds it. So dropping a piece is only possible from the end, which is
/// why [`Reserve::without_directory`] exists and there is no method for
/// dropping the fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reserve {
    /// The block table. Nothing else can go in the reserve without it, since
    /// the writer places it first or not at all.
    pub table: usize,
    /// The key section's checksum row, without which a sparse reader cannot
    /// verify what it fetched.
    pub row: usize,
    /// A copy of the fence, which is what lets a seek narrow before reading
    /// records.
    pub fence: usize,
    /// A copy of the hash directory: four bytes a key, and the difference
    /// between a lookup that plans its records straight away and one that
    /// fetches the directory first.
    pub directory: usize,
}

impl Reserve {
    /// Every piece, so a lookup needs no dependent read.
    pub fn bytes(&self) -> usize {
        fits(self.table, self.row, self.fence, self.directory)
    }

    /// Every piece but the directory copy. The open still takes one wave and
    /// the fence still narrows a seek; a lookup then plans the directory as a
    /// second read. Four bytes a key cheaper.
    pub fn without_directory(&self) -> usize {
        fits(self.table, self.row, self.fence, 0)
    }
}

/// The reserve a segment of these keys needs, for a caller holding a slice.
///
/// `keys` is one `(key length, run length)` per key, in key order; [`run_len`]
/// turns a key's value lengths into the second. `inline_max` and `block_size`
/// are the writer's, and must be the ones it will be given.
///
/// This is [`Planner`] over a slice, and the exactness and the failure cases
/// are its. A caller who will not hold its records should use the planner
/// directly: a slice of lengths is `size_of::<(usize, usize)>()` a key, which
/// is sixteen bytes where a pointer is eight.
pub fn for_lengths(
    keys: &[(usize, usize)],
    block_size: usize,
    inline_max: usize,
) -> Option<Reserve> {
    let mut p = Planner::new(block_size, inline_max);
    for &(key_len, run) in keys {
        p.push(key_len, run);
    }
    p.finish()
}

/// An upper bound on the reserve, for a caller that knows only totals.
///
/// `max_key_len` and `max_run_len` are not padding on the interface, they are
/// what a bound requires. The fence copies whole keys, so without the longest
/// one the answer is an average and averages are not bounds. Blocks are cut by
/// what fits, so a run that does not fit closes a block early: every block but
/// the last holds more than `block_size - max_run_len` bytes, and without the
/// longest run there is no bound on how many blocks there are at all.
///
/// The bound is loose in proportion to how far the longest key and run are
/// from the typical ones. A caller holding its input should use
/// [`for_lengths`], which is not a bound.
pub fn from_totals(
    keys: usize,
    max_key_len: usize,
    max_run_len: usize,
    run_bytes: usize,
    block_size: usize,
    inline_max: usize,
) -> Option<Reserve> {
    if keys == 0 {
        return for_lengths(&[], block_size, inline_max);
    }
    // The worst case for every part of the reserve is the same one: keys as
    // long as the longest, runs as long as the longest, as many of both as
    // the totals allow.
    let per_key_run = run_bytes.div_ceil(keys).max(1).min(max_run_len);
    let mut p = Planner::new(block_size, inline_max);
    for _ in 0..keys {
        p.push(max_key_len, per_key_run);
    }
    let mut need = p.finish()?;

    // `for_lengths` on an even shape cuts the blocks evenly, and an uneven one
    // cuts more. Every block but the last holds more than `block_size -
    // max_run_len`, so that is the worst count; a run at or over a block is
    // its own block, and then one block per key is the worst there is.
    let worst_blocks = if max_run_len >= block_size {
        keys
    } else {
        run_bytes.div_ceil(block_size - max_run_len).min(keys)
    };
    let even_blocks = blocks_for(std::iter::repeat_n(per_key_run, keys), block_size);
    if worst_blocks > even_blocks {
        need.table = flatindex::block_table_len(worst_blocks);
    }
    Some(need)
}

/// The reserve that holds all four pieces, laid out as `finish` lays them:
/// the table first, then the row, the fence and the directory, each 8-aligned
/// and each placed only if it fits. The table needs eight bytes to spare
/// before the writer will put it here at all, so a reserve smaller than that
/// holds nothing and is wasted whole.
fn fits(table: usize, row: usize, fence: usize, dir: usize) -> usize {
    let align8 = |n: usize| n.div_ceil(8) * 8;
    let mut at = align8(table);
    at = align8(at + row);
    at = align8(at + fence);
    (at + dir).max(table + 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_of_one_width_carries_no_prefixes() {
        assert_eq!(run_len(&[8, 8, 8]), 24);
        // A mixed run pays a varint each, and a zero-length value is not a
        // fixed run however uniform it looks.
        assert_eq!(run_len(&[8, 9]), 1 + 8 + 1 + 9);
        assert_eq!(run_len(&[0, 0]), 2);
        assert_eq!(run_len(&[]), 0);
        // Past 127 the varint takes a second byte.
        assert_eq!(run_len(&[200, 1]), 2 + 200 + 1 + 1);
    }

    #[test]
    fn blocks_are_cut_where_the_writer_cuts_them() {
        // Exactly full closes a block; the next run opens a new one.
        assert_eq!(blocks_for([64, 64].into_iter(), 64), 2);
        assert_eq!(blocks_for([32, 32, 1].into_iter(), 64), 2);
        // A run bigger than a block is a block by itself, and does not drag
        // what was staged beside it.
        assert_eq!(blocks_for([1, 100].into_iter(), 64), 2);
        assert_eq!(blocks_for([].into_iter(), 64), 0);
    }

    #[test]
    fn a_small_segment_reserves_kilobytes_not_the_old_floor() {
        // A hundred keys of sixteen bytes with hundred-byte runs: about
        // 100 KB of segment, which used to take a 32 KiB floor.
        let keys: Vec<(usize, usize)> = (0..100).map(|_| (16, 100)).collect();
        let need = for_lengths(&keys, 64 << 10, 0).expect("plannable").bytes();
        assert!(
            need < 8 << 10,
            "a 100-key segment wants {need} bytes of reserve"
        );
    }

    #[test]
    fn the_bound_is_never_below_the_exact_answer() {
        for &(n, klen, run) in &[(1usize, 4usize, 4usize), (10, 16, 100), (5000, 32, 7)] {
            let keys: Vec<(usize, usize)> = (0..n).map(|_| (klen, run)).collect();
            let exact = for_lengths(&keys, 64 << 10, 0).expect("plannable").bytes();
            let bound = from_totals(n, klen, run, n * run, 64 << 10, 0)
                .expect("boundable")
                .bytes();
            assert!(bound >= exact, "bound {bound} below exact {exact}");
        }
    }

    #[test]
    fn the_planner_holds_aggregates_rather_than_the_input() {
        // A million keys of sixteen bytes: the planner keeps one u32 per
        // sixteenth key and nothing else that grows.
        let mut p = Planner::new(64 << 10, 0);
        for _ in 0..1_000_000 {
            p.push(16, 100);
        }
        assert_eq!(p.keys(), 1_000_000);
        assert_eq!(p.retained_bytes(), 1_000_000usize.div_ceil(16) * 4);
        // Against a slice of the same lengths, which is a pair of usizes a
        // key -- asked of the target rather than assumed to be sixteen, since
        // this crate also builds for wasm32, where it is eight.
        let sliced = 1_000_000 * std::mem::size_of::<(usize, usize)>();
        assert!(p.retained_bytes() * 30 < sliced);
        assert!(p.finish().is_some());
    }

    #[test]
    fn the_planner_and_the_slice_agree() {
        // Uneven keys and runs, so the fence blob and the block cut both
        // depend on the order they arrive in.
        for n in [0usize, 1, 15, 16, 17, 4096, 40_000] {
            let keys: Vec<(usize, usize)> = (0..n)
                .map(|i| (8 + (i * 7) % 40, 1 + (i * 13) % 900))
                .collect();
            let sliced = for_lengths(&keys, 4096, 256);
            let mut p = Planner::new(4096, 256);
            for &(k, r) in &keys {
                p.push(k, r);
            }
            assert_eq!(sliced, p.finish(), "{n} keys");
        }
    }

    #[test]
    fn a_key_too_long_to_frame_is_refused_rather_than_sized() {
        let mut p = Planner::new(4096, 0);
        p.push(16, 100);
        p.push(u16::MAX as usize + 1, 100);
        assert!(p.finish().is_none());
    }

    #[test]
    fn a_run_past_what_an_extent_addresses_is_refused_rather_than_sized() {
        let mut p = Planner::new(4096, 0);
        p.push(16, 100);
        p.push(16, MAX_RUN + 1);
        assert!(p.finish().is_none());
    }

    #[test]
    fn record_bytes_that_would_wrap_refuse_rather_than_return_the_wrapped_sum() {
        // Runs at the largest a segment can hold, inline so every byte lands
        // in the record region. The sum passes usize on a 32-bit target long
        // before this many keys, and stays honest on a 64-bit one.
        let mut p = Planner::new(4096, MAX_RUN);
        for _ in 0..64 {
            p.push(16, MAX_RUN);
        }
        // Either the sum overflowed and it refused, or it did not and the
        // section is past what the index can address. Never a number.
        assert!(p.finish().is_none());
    }

    #[test]
    fn a_huge_run_does_not_wrap_the_block_cut() {
        // `cut` used to add the run to what was staged and compare the sum,
        // which is what overflows -- at `MAX_RUN` only where a usize is 32
        // bits, so the value here is one that overflows at any width and
        // takes the same path. `from_totals` reaches this with whatever
        // `max_run_len` its caller passes, so it is not a hypothetical.
        //
        // Wrapped, the sum comes out small: no block closes and the run is
        // left staged. The counts below are what says which happened.
        let huge = usize::MAX - 10;
        let mut staged = 0usize;
        assert_eq!(cut(&mut staged, huge, 4096), 1);
        assert_eq!(staged, 0);
        let mut staged = 100usize;
        assert_eq!(cut(&mut staged, huge, 4096), 2, "the sum wrapped");
        assert_eq!(staged, 0);
    }

    #[test]
    fn no_keys_still_reserves_room_for_the_table() {
        let need = for_lengths(&[], 64 << 10, 0).expect("plannable");
        assert!(need.bytes() >= flatindex::block_table_len(0) + 8);
    }

    #[test]
    fn the_directory_copy_is_four_bytes_a_key_and_the_only_optional_piece() {
        let keys: Vec<(usize, usize)> = (0..500).map(|_| (16, 100)).collect();
        let r = for_lengths(&keys, 64 << 10, 0).expect("plannable");
        assert_eq!(r.directory, 500 * 4);
        // Dropping it saves its bytes, give or take the 8-alignment it no
        // longer has to sit after.
        let saved = r.bytes() - r.without_directory();
        assert!(
            saved >= r.directory && saved <= r.directory + 8,
            "dropping a {}-byte directory saved {saved}",
            r.directory
        );
    }
}
