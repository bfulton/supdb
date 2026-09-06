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
//! So the size is computed. [`for_lengths`] answers exactly, from the key and
//! run lengths a caller that gathered its input already has. [`from_totals`]
//! answers with an upper bound for a caller that knows only totals. Both
//! return the [`Reserve`] broken into its four pieces, because the last of
//! them is a decision: the directory copy costs four bytes a key and buys a
//! lookup that plans with no second wave, and only the caller knows whether
//! its readers are paying for round trips or for bytes.
//!
//! **None of the layout arithmetic lives here.** `for_lengths` plans the key
//! section with [`crate::flatindex::plan_inline`], the same call the writer
//! makes, over placeholder keys of the caller's lengths; the block table's
//! size comes from [`crate::flatindex::block_table_len`], which
//! `encode_blocks` allocates by. A second copy of that arithmetic would be a
//! second definition of the format, and the two would drift the first time
//! one of them was edited.

use crate::flatindex;
use crate::index::{Ext, Extents};

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
    let mut blocks = 0usize;
    let mut staged = 0usize;
    for n in runs {
        if staged != 0 && staged + n > block_size {
            blocks += 1;
            staged = 0;
        }
        staged += n;
        if staged >= block_size {
            blocks += 1;
            staged = 0;
        }
    }
    if staged != 0 {
        blocks += 1;
    }
    blocks
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

/// The reserve a segment of these keys needs.
///
/// `keys` is one `(key length, run length)` per key, in key order; [`run_len`]
/// turns a key's value lengths into the second. `inline_max` and `block_size`
/// are the writer's, and must be the ones it will be given.
///
/// Exact but for the checksum row, which can be twelve bytes over.
///
/// The row covers the key section in pieces cut on the *object's* pages, so
/// its length depends on where the section lands, which depends on this
/// answer, which is the one circularity in the layout. It is resolved the
/// only way it can be from here: the row is taken at its worst alignment,
/// where the section starts one byte before a page boundary and cuts one
/// piece more than it otherwise would. That is four bytes, and the 8-aligned
/// boundary behind the row can move by eight because of them. Nothing else
/// rounds.
///
/// `None` when the input cannot be a segment at all: a key over 64 KiB, or a
/// key section past the flat index's limits. The writer would refuse it too.
pub fn for_lengths(
    keys: &[(usize, usize)],
    block_size: usize,
    inline_max: usize,
) -> Option<Reserve> {
    let inline = |run: usize| inline_max > 0 && run <= inline_max;

    // Placeholder keys and one extent apiece: the planner reads their lengths
    // and the extent count, never the bytes. A segment gives every key one
    // extent, and its tail is the run when the run is inline.
    let arena = vec![0u8; keys.iter().map(|&(k, _)| k).sum::<usize>()];
    let ext = Extents::One(Ext {
        block: 0,
        off: 0,
        len: 0,
        last: 0,
        count: 0,
    });
    let mut all: Vec<(&[u8], &Extents)> = Vec::with_capacity(keys.len());
    let mut at = 0usize;
    for &(klen, _) in keys {
        all.push((&arena[at..at + klen], &ext));
        at += klen;
    }
    let tail_arena = vec![0u8; keys.iter().map(|&(_, r)| r).sum::<usize>()];
    let mut tails: Vec<&[u8]> = Vec::with_capacity(keys.len());
    let mut at = 0usize;
    for &(_, run) in keys {
        tails.push(if inline(run) {
            &tail_arena[at..at + run]
        } else {
            &[]
        });
        at += run;
    }
    // No insert room and no record slack: a segment is never edited in place,
    // which is exactly how the writer plans it.
    let plan = flatindex::plan_inline(&all, &tails, 0, false)?;

    let table = flatindex::block_table_len(blocks_for(
        keys.iter().filter(|&&(_, r)| !inline(r)).map(|&(_, r)| r),
        block_size,
    ));
    // The section's own length is the planner's total; the row is appended
    // after it and covers everything before itself.
    let row = flatindex::checksum_row_len(
        plan.total,
        flatindex::PIECE_SHIFT,
        // The worst base: a section starting one byte before a page boundary
        // cuts one more piece than one starting on it.
        (1u64 << flatindex::PIECE_SHIFT) - 1,
    );
    // The fence copy is the span the reader will take: from the offset array
    // to the record region, which is what `fence_span` reports.
    let recs_off = plan.total - plan.recs_cap;
    let fence = if plan.fence_n == 0 {
        0
    } else {
        recs_off - plan.fence_offs_off
    };
    Some(Reserve {
        table,
        row,
        fence,
        directory: keys.len() * 4,
    })
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
    let shaped: Vec<(usize, usize)> = (0..keys).map(|_| (max_key_len, per_key_run)).collect();
    let mut need = for_lengths(&shaped, block_size, inline_max)?;

    // `for_lengths` on an even shape cuts the blocks evenly, and an uneven one
    // cuts more. Every block but the last holds more than `block_size -
    // max_run_len`, so that is the worst count; a run at or over a block is
    // its own block, and then one block per key is the worst there is.
    let worst_blocks = if max_run_len >= block_size {
        keys
    } else {
        run_bytes.div_ceil(block_size - max_run_len).min(keys)
    };
    let even_blocks = blocks_for(shaped.iter().map(|&(_, r)| r), block_size);
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
