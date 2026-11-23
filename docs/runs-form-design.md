# The runs form: a design for the stride form's second version

> A design registered before anything of it was built, and not built: kept
> for its reasoning, its predictions and its staged plan. The stride form it
> starts from is in the tree behind `Options::zpages_stride`, off as
> shipped, and what the pairings of that form read is in `docs/engine.md`
> ("Out of core: the second form", its last paragraph). They bear on this
> design in one place. The stride form's bytes are the payload's, so it
> fits a narrow band of budgets and loses where it does not fit; the value
> coder this design leaves to a late stage is therefore what decides
> whether the rest is worth building, since a form addressed the same way
> whose slots are smaller than the block form's groups fits wherever the
> block form does. Its stage S0, the forced stride form paired on the C
> mix, is done and its stop rule did not fire. The commits and line
> numbers below are those of `a8069e0`; `gfd/` is the scratchpad of the
> session that wrote this, and its two arithmetic scripts
> (`gfd/calc/che.py`, `gfd/calc/forfit.py`) and the three designs it was
> synthesized from are not kept.

## The design as registered (synthesized)

**Basis.** This document starts from the judges' winner, "step". It grafts in these ideas from the other two designs:

- from "lines": the one-line value block holding both columns, and the presence line with its rank inside the line;
- from "bytes": a probe before any format code, the value coder as a later stage, and the insistence that room be measured;
- from "lines" again: Che's approximation over N populations for the selector.

It fixes every error the judges verified. Section 0 says where each one went. It then takes the completeness critique:

- of its nineteen numbered findings, seventeen are taken whole and two in part (13 and 16);
- of its lists of missing tests, missing workloads and unmeasured constants, every item is taken but one, the DER constant, which is refuted in part.

§0.1 lists each with the evidence and where the fix went.

**Where the facts come from.**

- **Commit.** Every `file:line` is at **a8069e0**, read from the copies in `gfd/a80/`, which are byte-identical to `git show a8069e0:<path>`. I re-checked each one cited below against those copies.
  - HEAD has since moved to `312a9b0`, four commits on (`c840518`, `192db37`, `3baea42`, `312a9b0`). They touch `src/db/zfile.rs` (the cursor, and a door counter on the file), `src/db.rs` (docs, and `Reader::raw_touches`) and `tests/zpages.rs`.
  - To read a citation at HEAD, add the shift for its range (from `git diff a8069e0 HEAD`):

    | file | range at a8069e0 | add |
    |---|---|---|
    | `src/db/zfile.rs` | 1889-2276 | 29 |
    | | 2284-3043 | 30 |
    | | 3048-3352 | 40 |
    | | 3357-5210 | 41 |
    | | 5215-14918 | 49 |
    | `src/db.rs` | 8024-23618 | 9 |
    | | past 23631 | 13 |

    The `src/db.rs` lines before 8024 that this document cites move by 3 to 7.
  - The one change that bears on this design is `3baea42`'s `ZFile::raw_reads`. Under debug assertions it counts every read a cursor over a stride file makes through the form file's door as a raw touch, since such a file holds no value in a block. `Reader::raw_touches` sums it. A runs file holds every value in V too, so the counter is extended to every dense body (§1.8, T18).
- **Nothing was built, tested or run in `/home/user/supdb`.** Read-only `git show` and `git diff` were the only commands run there.
- **Two arithmetic scripts** were run in the scratchpad:
  - `gfd/calc/che.py`: Che's approximation to LRU for the unscrambled Zipfian (θ 0.99) over the value pages of 10M keys × 100 B;
  - `gfd/calc/forfit.py`: frame-of-reference residual widths by block size K for each workload's length law, using the least-range integer slope.

  Both compute on assumed distributions. Neither is a measurement of the engine.
- **Host** (sysfs and `/proc/cpuinfo`): Xeon 2.1 GHz, 4 vCPUs, L1d 48 KB, L2 2 MB, L3 260 MB, 64-byte lines.
- **Labels.** Every figure is one of four things:
  - a citation;
  - arithmetic, with its derivation;
  - an output of the two scripts;
  - an estimate marked *est.*, with the measurement that would replace it given in §10.

---

## 0. Decisions and why

### The design in one paragraph

v2 is a third form-file body, **the runs form**, with magic `SUPDZRN1`. It is kept beside the block form and v1, in the same cell (`Seg::z`), and it uses the raw segment's rank space.

A partition is held as two independent sides, each a short table of segments. This is the user's "contiguous keys into key range to value range entries":

- **The key side** maps numbers to ranks. Each segment is one of:
  - a **run**: every number from `x0` is present, so rank = `r0 + x − x0`;
  - a **bitmap**: one 64-byte presence line per 448 numbers, whose own count gives the rank;
  - (stage v2e) a **listed** segment: keys the codec cannot render, held whole and searched. One stray key then costs one entry, where it would otherwise decline the partition.
- **The value side** maps ranks to a count and a byte range of the value region V. It does this through two monotone columns:
  - `Cum`, the values before a rank;
  - `Off`, the bytes before a rank.

  `Off` is the prefix sum of the length array. Each column is coded per segment in one of three ways:
  - **linear** ("size and count", no bytes at all);
  - **derived** (`Off = w·Cum`, every value of width w);
  - **frame of reference**: one 64-byte line per K ranks holding K+1 positions, so a slot's two ends come from one line. This is "the delta-encoded length array, compressed", and all-one-length is its b = 0 case, which collapses to linear.

**Who decides what.**

- **The writer picks every encoding by exact bytes.** It writes a v1 file wherever v1's reader serves the partition exactly.
- **The engine's controller picks the form per partition** (raw, block, v1 or runs). It does so last, from measured inputs.

**How it is built.** Runs that lie in blocks are read through a blob that checks the blocks. The work is staged:

1. v1 is priced in the bench first, against the shipping store and with its walk's ask-ahead on and off;
2. then each workload is built;
3. then each engine stage is priced on its own workload, on a store whose shape the harness checks before it reads a cell.

### The decisions

Each decision gives the alternative it rejects and the evidence for it.

**D1. Start from "step"; graft the best of "lines" and "bytes".**

- **"Bytes" was rejected as the base for three reasons:**
  1. Its value side stores *lengths* as FOR residuals. A lookup then sums up to 63 fields per read, and the byte-lane shortcut it relies on needs b ≤ 8, while its own GPD figure is b ≈ 10.3. `forfit.py` gives a mean b of 9.8-10.4 for GPD lengths at K = 32-64.
  2. It switches RUNS to BITMAP by a run *count* (R > 8), so clustered sparse keys are declined as `KeyDensity`.
  3. Its coded multi-value read can fall back after handing over a value.
- **"Lines" was rejected as the base for four reasons:**
  1. Its gap-break rule (break where g > 280, at most the 7 largest gaps) turns a partition with one hole into a 49 KB bitmap, with one more line on every read.
  2. It chooses K once per file, so one outlying block halves K for the whole partition.
  3. Its claim that "K = 8 always fits" was computed for one column, while two columns share the payload.
  4. It lands the whole format in one commit, with no workload to price it.

  The evidence for both rejections is the judges' verified errors, re-derived here.

**D2. A third body with a new magic and a features word; v1's format untouched as the arm.**

What stays untouched is v1's *file*. Its read path does change in one place: its page check is factored with the runs body's (§1.8). That commit is priced on its own, by callgrind of `stride_read_all` before and after (S3).

- **The design.**
  - Magic `SUPDZRN1`, version 1. `git grep` finds no `SUPDZRN` anywhere.
  - The trailer head keeps `seal_trailer`/`open_trailer`'s offsets (`zpages.rs:3844`, `:3877`).
  - A `features` word that a reader refuses bit by bit before reading any table, so stages ship without a version bump.
  - `STORE_FORMAT` stays 2 (`db.rs:14957`). The magics pair in `store_format_names_the_files_magics` (`db.rs:15372-15394`) becomes a triple, and the comment there gives the precedent: "a new kind's magic, not a moved one, in a file no manifest names" (`db.rs:15364-15370`).
  - An engine from before this body reads the last page's magic (`zfile.rs:3229`), falls through to `Trailer::decode` (`zfile.rs:3266`) and refuses the file at the magic as damage. That is a refusal, never a misread.
- **Rejected: `SUPDZST1` at version 2.** `open_trailer` accepts one version (`zpages.rs:3877`), so v1 would need a version dispatch, and v1 would stop being an untouched arm.
- **Rejected: a renamed `SUPDZST2`.** A moved magic moves `STORE_FORMAT`; the test's assertion message says so (`db.rs:15389-15392`).

**D3. The writer writes a v1 file wherever v1's reader serves the partition exactly.**

- **The condition:**
  - one key run;
  - every rank one value;
  - one width W in `1..=STRIDE_W_MAX` (4096, `zpages.rs:4258`).
- **Flags and block runs do not disqualify.** A partition v1 itself declines only for a C_TOMB flag or a value in a block still gets a v1 file: v1's reader answers it exactly, as D4 and D11 show.
- **The read-back has to accept those files.** `check_stride` re-parses every rank with `stride_record` and refuses on its error (`zfile.rs:7368-7370`). `stride_record` errs on a C_TOMB flag (`zfile.rs:7052`, `:7064`) and on a run that is not inline (`:7066`). So the runs writer's v1 files are read back by the same `check_stride`, with the record leg made a parameter:
  - the Forced arm passes `stride_record`, byte for byte as today;
  - the runs writer passes its own walk's normalisation: the key from the record, the value from the inline run or from the checking blob's `values_at` for a block run, with the flag ignored.

  It is one check with the record leg as its input ("a check is written once"). T12 runs a C_TOMB fixture and a block-run fixture end to end through it.
- **The codec is v1's wherever v1 has one.** The runs writer first calls v1's `KeyCodec::detect` (`zpages.rs:4448-4491`). Where that succeeds, which means the keys are a contiguous run under it, its codec is the file's. §4.2 gives the rule where it fails, and the proof that the rule never refuses a partition v1 serves.
- **The file** is byte-identical to `write_stride`'s output (`zfile.rs:7155-7343`) wherever both writers would write. A test holds that (T12).
- **Rejected: a degenerate v2 file with a fast path** (the "bytes" §3.5 and "lines" plan). Such a file reads two table lookups and their branches more than v1 does.
- **Why it matters.** Every partition every existing row builds is v1-shaped: keys are 16-digit and contiguous, each holds exactly one 100-byte value, nothing deletes and nothing appends to an existing key (scout 3 §0, scout 2 "What each scenario leaves").
  - So under the v1 arm and the v2 arm every existing row builds **identical files**, and every read is the same read. The count `runs_files = 0` on those rows proves the files (§7.1).
  - **The build is not identical, and it is priced.** The runs writer reaches v1's trailer by its own walk. That walk records nothing beyond v1's own state for as long as every rank so far is v1-shaped (§4.2), so its fresh pages are v1's. The control holds the builds' `cpu_us`, major faults and minor faults equal between `supdb-zruns` and `supdb-zstride` (§7.1). Until that control reads level, "no cell traded" is a hypothesis about the build, not a fact.

**D4. Keep the raw rank space. Keep 0-value ranks. Drop tombstone flags.**

- **Ranks.** Every `ZFile` method answers by the raw segment's ranks (`zfile.rs:4123-4959`). A scan's fallback resumes the raw walk at `seen`. So no rank translation is stored and no caller changes.
- **Flags.**
  - `Seg::tombs` is false for every partition, and the reason is written into the field's doc: "a promoted piece is the only and therefore the oldest source over its own range, and a tombstone with nothing older beneath it masks nothing" (`db.rs:8086-8100`). `Seg::open` sets `tombs: false` for a `par-` name (`db.rs:11694`).
  - The point read's tombstone pass skips any segment whose `tombs` is false (`db.rs:24879-24881`).
  - So dropping C_TOMB from a record that has values is exact. v1's `Tomb` decline goes.
- **0-value ranks.**
  - A piece merge writes a record with no values for a key it tombstoned (`db.rs:16740-16752`), and a promotion links the file as it stands.
  - The raw scan counts such a rank in `seen` and emits nothing (`blob.rs:2339`). Raw `count` answers 0 (`blob.rs:1854-1859`).
  - The form keeps the rank with a count of 0. Dropping it would shift every rank after it.

**D5. The value side is two monotone columns over ranks, and positions are coded, not lengths.**

- **The columns.** `Cum(r)` is the values before rank r, and `Off(r)` the slot bytes before rank r.
  - Rank r's count is `Cum(r+1) − Cum(r)`, and its slot is `V[Off(r), Off(r+1))`.
  - `Off` is the prefix sum of the length array, so coding its positions frame-of-reference **is** the user's "delta encoded length array which is compressed". All one length is `Off = len·r`: "size and length".
- **Rejected: FOR on lengths** (bytes). A read would sum up to 63 residuals of about 10-12 bits, which `forfit.py` puts at a mean of 9.8 at K = 32 for GPD, over a header line and 82 or more bytes of residuals. That is 3-4 lines and on the order of a hundred instructions (*est.*), where two adjacent position extracts from one line answer in O(1).
- **Rejected: general-purpose compression of the length array.** It destroys random access: a point read would decode a block to find one offset.
- **Rejected: an offset per value** (bytes' O over values). It costs one entry per value rather than one per rank, and the values of a rank would need a second decoder, where the raw run already has one: `index::each_value` (`index.rs:210`).

**D6. A rank's count always comes from `Cum`, never from a slot's length.**

- This fixes the judges' error in "step"'s F1 shape. A 0-value rank (count 0, slot of length 0) and a rank holding one empty value (count 1, slot of length 0) are both empty slots, so the slot's length cannot tell them apart. `Cum` can.
- `count` therefore touches no value page in any shape. "Step"'s Fp count parsed a slot, and so touched a page.

**D7. A slot is what the raw record's run is, and the segment says how to split it.**

- **What a slot holds:**
  - count 0: an empty slot;
  - count 1: the value itself;
  - count ≥ 2: the run exactly as `index::encode_run` lays it out (`index.rs:243-262`): FIXED where every value has one width of at least 1, varint-prefixed otherwise.

  The slot is defined by `encode_run`'s rule, whatever the raw record's own coding. The engine's writers make every multi-value run through `encode_run` (`SegmentWriter::end_with`, `db.rs:10286`), so for their inline runs the slot is the raw run's bytes, copied. A reader also accepts a prefixed run whose values happen to share one width (a forged or foreign record). The walk sees that while it reads the varints, which it does anyway to count and validate, and re-encodes such a run FIXED with `encode_run`. So a slot's coding is always a function of its values, and DER and FIX, defined on the values' widths (§1.3), agree with the slots by construction.
- **Why this shape.** Because a slot's bytes do not depend on which segment it lands in, the builder streams V before it has chosen any segment's codes.
- **The FIXED-or-prefixed choice for counts ≥ 2** is given per segment:
  - FIX(w): every value has width w;
  - RUN-F: every run is FIXED, widths vary per rank;
  - RUN-P: every run is prefixed;
  - RUN-M: a mask bit per rank, held in the segment's lines.
- **Rejected: always re-encoding counts ≥ 2 as prefixed.** It costs a varint byte per value, a re-encode of every FIXED run in the walk, and it breaks `Off = w·Cum` for fixed-width multi-value keys.
- **Rejected: "lines"' per-rank mask in every segment.** That spends K bits a line where a segment is uniform.
- **Rejected: FIX(0).** `each_value` refuses a fixed width of zero (`index.rs:212-218`), and `encode_run` writes width-0 runs prefixed (`index.rs:248`). One decoder serves every slot.

**D8. The frame-of-reference line, byte by byte, readable inside its own line.**

- **Layout.** One 64-byte line per K ranks of a segment holds K+1 positions of each coded column, position K included, so the last rank of a block never reads the next line.
  - **One coded column:** an 8-byte header and a 448-bit payload.
  - **Two coded columns:** a 16-byte header and a 384-bit payload.
- **A field read** is one unaligned 8-byte load at byte `min(bit/8, P−8)` of the payload, where P is the payload's length in bytes, then a shift and a mask.
  - This fixes "step"'s load past the line: at K = 32, b = 13 and i = 31, a residual starts at payload bit 403, byte 50 of the payload and byte 58 of the line, so an unclamped 8-byte load reads bytes 58-65 of the line.
  - With the clamp, every load lies in the payload: the clamped shift plus b is at most 64 because the field ends inside the payload.
- **The field-width rule.** A rank whose slot is 2^16 bytes or more, or whose count is 2^16 or more, goes into a U segment of its own (§1.7). Inline runs are at most `inline_bytes` = 256 (`db.rs:2706`, `:6062`), so only block runs reach this rule.
- **Under that rule K = 8 always fits both columns.**
  - Over 8 ranks a column's positions rise by less than 8·2^16 = 2^19, so slope 0 leaves a range under 2^19 and b ≤ 19.
  - With the mask: 9·19 + 9·19 + 8 = 350 ≤ 384.
  - This fixes "lines"' "K = 8 always fits", which was computed for one column.
- **Why two payload sizes, from `forfit.py`, at K = 32, one value a key:**
  - Lengths drawn from the GPD clamped to [1, 4096] need b ≤ 12 in every block. That fits 448 bits (cap 13) and not 384 (cap 11; p99 11, max 12).
  - Uniform 50-150 needs b ≤ 9.
  - Counts of 1+Poisson(1) need b ≤ 5 at K = 64, which fits 448 bits (cap 6).
  - Counts and lengths together for multi-value GPD values fit K = 16 in every block, two columns, and K = 32 in none.

**D9. K is chosen per segment from {64, 32, 16, 8}, by exact bytes, and the segment boundaries that change K are re-decided at block granularity. No Elias-Fano and no WIDE escape in v2.**

- **Rejected: K per file** ("lines"). One outlier halves it for the whole partition.
- **Rejected: a greedy that only lowers K.** A segment opened at an outlier's block takes the K that block allows, and a greedy with no rule to raise it again keeps that K for the rest of the partition. That is "lines"' flaw, one segment at a time. Within each stretch of fixed codes, a DP over 8-rank units chooses where K changes (§4.3). It pays one 32-byte entry per change, and it runs only on a stretch where some block did not fit at K = 64.
- **Rejected: an Elias-Fano code in the line** ("lines").
  - At the measured widths, FOR fits K = 32 for the user's variable-length case.
  - EF's decode is a select across words, *est.* about 40 instructions for two positions against FOR's 15-20.
  - It saves bytes only where FOR forces K below what EF would allow.
  - The count of such blocks is the trigger (§10, Q14).
- **Rejected: WIDE** ("lines"). It makes the read two lines. The field-width rule (D8) removes its use.

**D10. The key side: one `KeyCodec` per partition, runs, (stage v2c) bitmap presence lines chosen per chunk by bytes, and (stage v2e) listed segments for the keys the codec cannot render.**

- **Runs.** A run breaks exactly at a missing number, so the run table is canonical.
- **Bitmap lines** use "lines"' layout: a `before` count, packed sub-counts, then seven words of presence bits. Presence and rank come from one line with at most two popcounts.
- **Popcount and select are chosen per CPU.** The repository builds for the baseline x86-64 target (there is no `.cargo/config.toml` at a8069e0), so `count_ones` there is a software popcount, and `pdep` exists on x86 alone. CI runs two arm64 legs as well (`ubuntu-24.04-arm`, `macos-latest`, `.github/workflows/ci.yml:51`).
  - So the presence line's rank and select are one function each. Each has a portable body, with a broadword in-word select, and an x86-64 body compiled under `#[target_feature(enable = "popcnt,bmi2")]`.
  - The body is picked once a process by `is_x86_feature_detected!`, as the CRC is (`block.rs:226-236`) and the flat decode loop is (`zlz4.rs:771-781`).
  - `pdep` is microcoded on AMD before Zen 3, so the select's two bodies are both priced in S2. The detection may then prefer the portable body there.
- **The choice.** Per aligned chunk of 64 presence lines (28,672 numbers, one 4 KiB page of lines), whichever of runs (16 B each) and bitmap (64 B a line) is smaller. Ties go to the bitmap, the fewer lines. This is Roaring's container rule.
  - The chunk is a granularity, but it is a constant, and S2 prices it. The probe computes the rule's bytes at chunks of 8, 16, 32, 64, 128 and 256 lines over each workload's law, against an exhaustive split. The chunk ships where the bytes stop falling (Q21).
- **Listed segments (v2e).** A key the codec cannot render goes to a listed segment: its bytes held whole in M and searched by bytes (§1.2).
  - Without them, one stray key declines the whole partition. That might be the empty key (which the fuzz draws), a key of another length, or a sentinel.
  - The format family already serves "records the form does not hold" (`ZNOPAGE`, `zpages.rs:26-34`, `:148`, read raw at `db.rs:24143-24146`). A listed segment does better, since the value side is codec-free: its ranks' values stay in V and nothing is read raw.
  - It also replaces "lines"' codec runs. A stretch of keys under another codec is a listed stretch, at the cost of its key bytes.
- **Rejected: "lines"' gap-break rule** (see D1).
- **Rejected: "bytes"' switch at more than 8 runs.** It is a count standing in for bytes.
- **Rejected: several codecs per partition ("lines"' codec runs).** Listed segments cover the same keys with no second codec, at their bytes (above).
  - The trigger for v2e is a workload with stray keys (§7.2, `sparse` with `stray`), and the count of `NotShaped` declines by sub-cause on it (§10, Q15).
  - The sub-causes are two, `KeyShapeNo::Length` and `Field` (`zpages.rs:4376-4387`). A "prefix" sub-cause is not counted: no input reaches it. A key between the first and the last in byte order shares their common prefix, as `detect`'s own doc says (`zpages.rs:4444-4446`).
  - Variable-length hashed keys such as YCSB's `"user"+hash` would be almost all listed: their M would be the keys' own bytes in the heap. They are the block form's, by the controller's comparison of the forms' resident bytes (§6), not by a writer's threshold.

**D11. A runs build reads block runs through a blob whose block checks are on.**

- **The current builder** opens its blob with `verify_checksums: false` and `verify_index: false` (`zfile.rs:6673-6680`), and with checks off `Blob::verify` returns `Ok` without hashing (`blob.rs:1502-1504`).
- **The serving blob** gets the store's `segment.checksums`, true by default (`db.rs:11119`), through `Seg::open` (`db.rs:11585`).
- **The bug in "bytes" and "lines".** Both copy block runs through the builder's blob. A block flipped on disk would therefore become a form page under a good CRC word, and the read-back, comparing against the same unchecked bytes, would pass it. Where the raw form answers an error, the form would answer bytes.
- **The fix.** The runs path opens with `verify_checksums: plan.verify`, the store's own setting, and declines `Unreadable` on a failed block.
- **Cost.** `verify_checksums` is read only on block reads (`blob.rs:1502`, `:1619`), so it costs a partition with no block nothing.
- **Asked ahead and dropped behind.** The current walk asks ahead and drops behind over the record region alone (`zfile.rs:7211-7215`, `:7288-7303`), since v1 reads no block. The runs walk does the same over the blocks its runs lie in. The segment writer numbers a block by its place at the push, `self.blocks.len()` (`db.rs:10325-10333`), and flushes blocks in order, so block ids ascend with rank.
  - The walk asks for the blocks after the last one it has met, a window at a time, through the blob's block table, and drops the blocks behind it. These are the build's own reads of a population it replaces (CLAUDE.md, "A pass that reads what it replaces keeps it"; `zpages_build_ahead`).
  - An id below the last one met stops the asks and counts `blocks_out_of_order`. A record from another writer could do that; the asks are a lever, never a premise.
- **Inline records** keep the raw form's own exposure. The raw read does not hash them per read; the key section's row is verified at an open with `verify_index` (`blob.rs:773`), and not for a landed segment (`db.rs:11549-11561`). So form and raw answer the same bytes there.

**D12. Validate, then emit.**

- **The contract.** "The callback ... is called only after every check that can fail has passed, so a read that falls back has emitted nothing" (`zfile.rs:19-21`). `each_value` emits as it parses (`index.rs:226-236`).
- **For a RUN-P slot,** the varints are walked once to count exactly the rank's values and to end exactly at the slot's length. Only then is anything emitted.
- **Under value coding (v2d),** every value of the rank is decoded before any is emitted. This fixes "bytes"' partial emission.

**D13. Everything but V is resident; M is checked whole at the attach; a check is claimed only where it can fire.**

- **Resident and checked once.**
  - The map M (row, key segments, value segments, lines, listed keys) is read with one `pread` into a 64-byte-aligned heap buffer, then its CRC is checked. It is checked structurally in one O(|M|) pass.
  - It is not read through the mapping. The attach advises the mapping random before its own reads (`zfile.rs:3209-3216`), so a read through it would come in a 4 KiB fault at a time. A `pread` is one syscall.
  - Its pages are then dropped from the page cache with `POSIX_FADV_DONTNEED` over M's span. No mapping holds them, since the read did not go through one.
  - There are no first-touch checks of M: each check is written once (CLAUDE.md, "A check spelled twice").
  - The precedents are the block form's resident fence and v1's row copy.
- **The page cache is charged for V alone.** These accessors work over the whole mapping today, whichever body it holds: `pages_resident` (`zfile.rs:5022-5028`), `prefetch_all` (`:5125-5127`), `file_bytes` (`:5104-5106`, the room's and the hot look's charge, `db.rs:28483`, `:47502`), and the asks and re-asks of `z_join_asks` and `swap_ahead` (`db.rs:48325-48356`).
  - A runs file's M is heap, never in the cache. So the whole-file share could never reach the builder ahead's 90% (`ahead_resident_pct`, `db.rs:6193`, read at `db.rs:38954-38971`, `:39010`) where M is more than a tenth of the file. The builder ahead runs under `ZPages::Always` too (`ahead_out_of_core` is true, `db.rs:6192`; the gate at `db.rs:37172-37176`). The room would charge M twice, once as cache and once as the anonymous memory the budget subtracts. And the asks would pull M back into the cache.
  - So a dense body gives `served_span()`, the byte range of V, and `cache_bytes()`, its length. Every residency, prefetch, drop, swap, ask and room site reads them. `file_bytes()` stays the file's disk size for the disk counts (`db.rs:23887`).
  - A block body's span is the whole file, as today. v1's span is V too, which moves its residency share by its row's share of the file (at most 0.4%, 976,564 B of 1,000,984,576 at 10M). The S5 control re-runs S0's v1 arm to price that.
- **Counted where the process is counted.** The memory census sums the segments' ranks, bounds, bloom and heads (`db.rs:31814-31874`) and no form file's heap. M is 20-320 MB at 10M (§2.9), an owner nobody counted. The census gains a `zf_heap` line, the sum of `resident_bytes()` over the cells and their spares, held in `census_cap`.
- **No claim beyond what the sums can see.** "Bytes" claimed that sums of residuals catch "lengths permuted". Sums cannot see a permutation. Here a consistent forgery of interior positions under good CRCs is refused only where the attach's sample lands. That is v1's and the block form's stated limit (`zfile.rs:3386-3389`), and it is held as a test that asserts the forgery is **served** (T20).

**D14. The selector is the last stage: Che over N populations, heat in one unit, the room read through the mix.**

- **The model.** It generalises `model_raw`'s two-population Che approximation (`zselect.rs:356-414`).
- **Three corrections to the designs' arithmetic, from `che.py`:**
  1. With 12% of the value pages out of cache, the unscrambled Zipfian misses on **1.1%** of reads (LRU by Che). That is not "bytes"' 0.8%, which is the LFU oracle, and not 12%, which is `model_z`'s uniform reading (`zselect.rs:416-422`) that led "step" to say a dense form loses every cell at 1 GiB.
  2. The first partition of 345k keys carries **78.5%** of that Zipfian's reads. A model uniform within each partition overstates the head partition's misses, so the model takes a partition as sub-populations by heat band, or takes the census's measured rate where one exists (§6).
  3. The room is read through the mix, not at one moment. "Bytes"' "0.15 GB spare" at 10M under 1.25 GiB was read at E's start. The budget falls from 1.2 GB at the load's end to 0.87 GB through E (`zselect.rs:448-450`).

**D15. Value coding is a later, probe-gated stage, chosen per partition by heat.**

- **What it is.** Each value is an LZ4 block coded against a per-partition dictionary ("bytes"' `Lz4Dict`).
- **Why it is the lever.** It is the only footprint lever below raw values, *est.* 0.61-0.62 of V for the suite's payload. Each pool chunk is 50 motif bytes then 50 random bytes (`bench/src/workload.rs:278-301`), so a 100-byte window carries 50 random bytes, and the floor is 0.5.
- **Why it is gated.**
  - The record coding lost every cell paired at 10M under 1.25 GiB, the point reads included, once the block coding had gained the partial decode, the memo's admission and the key table (`db.rs:2222-2232`).
  - A probe of the dictionary's ratio and cold decode cost (`zgen`, S2) is therefore the gate, and the stage (S11) comes after the bitmap (S9) and the listed segments (S10).
- **Where it applies.** It is never applied to a scan-hot partition: a whole decode through records cost a scan 2.2x the instructions of a group decode (`db.rs:2215-2219`).

**D16. The bench before the engine.**

- **No bench arm sets `zpages_stride`.** `git grep zpages_stride HEAD -- bench/` is empty, and no stride counter is surfaced.
- **So the first steps change no engine code:**
  - S0 prices v1 in the bench at all. It runs v1's walk with its ask-ahead on and off, and holds it to the shipping store (`supdb`, `ZPages::Auto`) as well as to the forced block form and LMDB.
  - S1 builds the workloads that show v1's cliffs;
  - S2 is the probe that fixes the open constants (CLAUDE.md: "price the primitive before a third policy").
- **Why the ask-ahead arm.** v1's walk asks ahead only behind `Options::zpages_scan_prefetch`, which is off by default (`db.rs:6112`). The ask's own doc leaves the decision to "the forced pairing" (`zfile.rs:5346-5358`). The attach advises the mapping random before any read (`zfile.rs:3209-3216`). So under the bound, a v1 scan with the gate off takes value pages one 4 KiB fault at a time.
  - Priced only that way, v1 would lose E and the scan pass for a reason the next stage would fix. The Policy field already exists: `zpages_prefetch`, used by `supdb-nocachezpagesprefetch` (`bench/src/engines.rs:1365-1370`).
- **Why the shipping bar.** Under `ZPages::Always` the hot-raw policy does not run (`z_hot_runs` needs `ZPages::Auto`, `db.rs:48444-48448`). So `Always` + `Forced` against `Always` + block isolates the body, and `supdb` (Auto) is the bar a v2 arm has to clear in the end.
  - A loss to `supdb` alone, where the forced dense arm beats the forced block arm, is the hot-raw policy's win. It is S12's question, and no reason to stop (§10, Q1).

**D17. Decline reasons are one enum, read where they are counted.**

- **`StrideNo`** (`zfile.rs:6094-6122`) gains `Unreadable` and `TooLarge`. `STRIDE_REASONS` goes from 8 to 10.
- **The `NotShaped` sub-cause** (length, field) is counted apart. A prefix sub-cause has no input that reaches it (D10).
- **`Seg::z_stride_no`** is written (`db.rs:38120`, `:50937`) and read nowhere. The counters and the bench now read it, because those counts are the triggers for D9, D10 and D15.

**D18. One option per change, the old shape kept as an arm, every option off.**

- `ZStride::Runs`: the runs body. `ZStride::Forced` stays v1, and `Off` stays the default (`db.rs:518-532`).
  - **`Runs` is refused beside `ZPages::Auto` until S12.** The controller sizes forms from the block sampler, which declines every partition with a block (`zfile.rs:8027-8030`). Under Auto, a runs build would install forms of sizes the model never predicted, and the partitions it serves best (block runs) would never be bought.
  - `Forced` beside `Auto` is not refused: the hot look reads a stride file deliberately (`db.rs:47483-47494`). Its sizing problem is the same one, and it is S12's prerequisite 11 (§6).
  - `Runs` is refused beside the re-code policies, as `Forced` is (`db.rs:14884-14891`).
- `Options::zpages_runs_for`: frame-of-reference lines. Off means U segments only, which is v2a's shape.
- `Options::zpages_runs_bitmap`: presence lines.
- `Options::zpages_runs_listed`: listed segments (v2e).
- `Options::zpages_runs_coded`: value coding.
- `ZStride::Auto`: the last stage.
- Each is EXPERIMENT and `#[doc(hidden)]`. A default flips only after `sh scripts/check.sh` passes with it in place, and after two sittings lose no cell.

**D19. In-core latencies are L2/L3 here, not DRAM.**

"Step"'s "3 cold lines ≈ 250 ns at DRAM latency" is wrong on this host. Every table of every partition fits L3 at 10M, and most fit L2, so a table lookup is dependent L2/L3 hits, *est.* 30-100 ns. §10 Q3 measures it.

### 0.1 The completeness critique, finding by finding

I checked each finding against the `gfd/a80/` copies before acting on it.

| # | finding | verdict and evidence | where the fix went |
|---|---|---|---|
| 1 | S0 prices v1 with no scan ask and without the shipping arm | **Taken.** The gate is off by default (`db.rs:6112`); the ask's doc leaves it to the forced pairing (`zfile.rs:5346-5358`); the attach advises random (`zfile.rs:3209-3216`); Always runs no hot-raw policy (`db.rs:48444-48448`) | D16, S0, Q1 |
| 2 | no read-back accepts the runs writer's v1 files for flags or block runs | **Taken.** `check_stride` refuses on `stride_record`'s error (`zfile.rs:7368-7370`), which errs on a flag and on a block run (`:7052`, `:7064`, `:7066`) | D3, §4.4, T12 |
| 3 | `multi` never checks the shape it prices | **Taken.** `settle` starts nothing new (`db.rs:35734-35738`); the only wait for form files is `Engine::fold` (`bench/src/engines.rs:11475-11497`), run only under `SUPDB_MIX_FOLD` (`bench/src/run.rs:4327-4337`); `zpages_refusal` checks Loaded, Shuffled and Read alone (`bench/src/run.rs:2283-2321`) | §7.2 (`multi`, the fold and its refusal stage), S8 |
| 4 | forced arms refused on `varlen` and `multi` | **Taken.** A forced arm is refused at Loaded where `form_files + small != partitions` (`bench/src/run.rs:2289-2295`), and the block body declines any partition with a block (`zfile.rs:6692-6694`) | §7.2, a table of the arms each workload runs |
| 5 | `detect_kind` without the count test breaks dominance over v1 | **Taken**, with a case: keys `"05"`…`"10"` under decimal count 6, and the 252-key big-endian run between them is v1-shaped. | §4.2: `detect` first, then decimal, then big-endian on a field failure, with the proof that this reaches v1's codec wherever v1 has one; T12(b) |
| 6 | residency and footprint read over the whole mapping | **Taken.** `zfile.rs:5022-5028`, `:5104-5106`, `:5125-5127`; `db.rs:28483`, `:47502`, `:48325-48356`, `:38954-38971` | D13, `served_span`/`cache_bytes`, T31 |
| 7 | one stray key declines a partition | **Taken** as stage v2e, listed segments. The value side, being codec-free, serves their values from V | D10, §1.2, §2.2, S10 |
| 8 | the greedy never raises K again | **Taken.** §4.3 closed a segment only where K had to fall | D9, §4.3 (the DP within a stretch), T16 |
| 9 | M read under `MADV_RANDOM` | **Taken** (`zfile.rs:3209-3216`) | D13 (one `pread`), §2.7 |
| 10 | block runs neither asked ahead nor dropped behind | **Taken.** The ask and the drop cover the record region alone (`zfile.rs:7213`, `:7288-7303`); block ids ascend with rank (`db.rs:10325-10333`) | D11, §4.2 |
| 11 | "no cell can be traded, by construction" overstated | **Taken.** The files are identical; the build is not | D3, §4.2 (no facts while v1-shaped), §7.1 (the build control) |
| 12 | `pdep` is x86-only, CI runs arm64 | **Taken**, and widened: at the baseline x86-64 target `count_ones` is a software popcount too | D10, §2.5, S2 |
| 13 | `StrideAttach` cannot be reused unchanged; an unknown feature should be Identity | **Taken in part.** The type returns one decision a store (`db.rs:5975-6000`), so it gains a second. But the "caps" do not fire at an attach: Damage and Identity both store `DECLINED_REFUSED` and rebuild (`db.rs:38035-38040`), and the damage cap is a served file's (`db.rs:38305-38317`). What Damage does move is `rejected_damage` (`zfile.rs:5878-5880`). Identity is right by the enum's own words (`zfile.rs:142-147`) | §2.7, §2.8 |
| 14 | the census does not count form-file heap | **Taken** (`db.rs:31814-31874`) | D13 (`zf_heap`), T23 |
| 15 | the factored page check touches v1's hot path | **Taken** | D2, S3 (its own commit, callgrind held equal) |
| 16 | Runs and Forced not refused beside Auto | **Taken in part.** `Runs` is refused until S12. `Forced` beside `Auto` is a supported combination (`db.rs:47483-47494`); refusing it would change v1's arm | D18, S4 |
| 17 | the prefix sub-cause is unreachable | **Taken** (`zpages.rs:4444-4446`) | D10, D17, §4.2 |
| 18 | §5 misstates a run's `first_num` forgery | **Taken.** Runs carry their own numbers, so a forged `first_num` moves its own run alone, and passes the structural pass where it keeps a gap on both sides | §5, T20 |
| 19 | `owner_span_from`'s hint dropped | **Taken.** v1 ignores it (`zfile.rs:4923-4926`); the block form gallops from it (`:4930-4933`) | §3.3 |

**The lists:**

- **Missing tests.** All are taken: promotion, trust, the count half of the field-width rule, `TooLarge`, the refusals, cursor parity at a limit, the flip before the serving blob verifies, and regressions for 5 and 6. The flip one matters because the blob remembers verified chunks (`blob.rs:1455-1462`). They are T9(t), T10, T15, T28-T31.
- **Missing workloads.** All are taken: multi with varied lengths, block runs of 2^16 bytes or more, holes made by deletes and merges, a count pass, and the roster's cost (§7.2, §7.3).
- **Unmeasured constants.**
  - The chunk is priced in S2 (Q21).
  - The sixteen hole numbers become `sample_pages` over the gaps, the existing definition.
  - The "tenth of M" gates become the model's price (Q5, Q14).
  - The `blocks() > 0` move is held to v1's reason (§4.1).
- **The DER constant, refuted in part.** DER on value widths is consistent once the slot is defined by `encode_run`'s rule. The engine's writers make every multi-value run through it (`db.rs:10286`), and a foreign prefixed run of one width is re-encoded FIXED (D7). The sentence that said every inline slot is a plain copy was the error, and it is gone.

---

## 1. The model

### 1.1 Ranks, and the two sides

A partition is its n records, ranks `0..n` in key order. The form answers two kinds of question:

- **For a rank:** its key (`key_at_into`), its values in the raw form's order (`values_at`) and its count.
- **For any key:** its rank or its absence (`read_all`, `count`), and its lower bound (`seek`, `owner_span_from`, `cut_in`).

These are the methods v1 dispatches at `zfile.rs:4123`, `:4161`, `:4207`, `:4688`, `:4738`, `:4791`, `:4923` and `:4959`.

The two sides are independent:

- **The key side** maps ranks to numbers and back.
- **The value side** maps a rank to a count and a byte range of V.

A hole breaks only the key side. A change of width or count breaks only the value side. Each side's table has as many entries as that side has breaks, plus a sentinel.

### 1.2 The key side

**One codec.** The whole partition uses one codec, as in v1 (`KeyCodec`, `zpages.rs:4401-4407`):

- a kind, decimal or big-endian;
- a prefix of P bytes;
- a field of D bytes;
- L = P + D.

The codec's `base` is the first run's number. Runs carry their own numbers.

**A run segment** is `(r0, x0)`. It holds the numbers `x0 + (r − r0)` for ranks `[r0, r_next)`.

- Runs are **maximal**: the next segment's first number is at least `x0 + (r_next − r0) + 1`. This is checked at the attach, so the table is canonical.

**A bitmap segment** (v2c) is `(r0, x0, first line)`. It covers the numbers `[x0, x0 + 448·lines)`.

- Bit q of the line at `(x − x0) / 448` says whether `x` is present. The divide is by a constant, which compiles to a multiply and a shift; the assembly is read before the read path is priced (CLAUDE.md, "A divide an entry").
- Its first line holds `x0` as a present number, and its last line is not empty, so the segment is canonical.

**A listed segment** (v2e) is `(r0, nb, first listed key)`. It holds the keys of ranks `[r0, r_next)` whole, in a key blob in M (§2.2).

- **What goes in one.** Exactly the keys the codec does not render: a key fails `decode` or does not render back to itself. Adjacent such ranks form one segment, so listed segments are maximal.
- **Its `first_num` field** holds `nb = numbers_below(first key)`, saturated to `u64::MAX`. That makes KS sorted by `first_num` as well as by rank: `numbers_below` never falls as keys rise.
- **No present rendering lies between two of a listed segment's keys.** It would be a rank between them, in a number segment that splits the listed one.
- **Ties.** Two listed segments never share an `nb`: a number segment lies between them, and its first number is at least the first one's `nb`. A listed segment may share `first_num` with the number segment after it. Its keys then lie below that segment's first rendering, since that rendering is the least one at or after them, and the keys are not renderings.

**Key to rank.**

1. `x = codec.decode(key)` (`zpages.rs:4546`).
2. **A key that decodes** (and renders back, which a decoded key always does):
   - Find the last segment with `first_num ≤ x`. Where there is a tie, that is the number segment, which starts at `render(x)`.
   - Take the rank:
     - for a run, `r0 + (x − x0)` if that is below the next segment's first rank, otherwise absent;
     - for a bitmap, absent where the bit is clear, otherwise `r0 + before + the ones below the bit within the line`;
     - for a listed segment, the key's place among its keys by byte search, or absent.
3. **A key that does not decode:**
   - Without listed segments (features bit 2 clear), it is absent at once.
   - With them: `y = numbers_below(key)`. Take the last segment with `first_num ≤ y`. Where that is a number segment with `first_num = y`, take the one before it, since that segment starts above the key. Then search the key among a listed segment's keys. In a number segment it is absent.

**Why absence is exact.** The build proves that every key of the partition either decodes and renders back to itself byte for byte, or is listed, and that each was placed in exactly one segment (§4.2). So:

- a key that does not decode is absent where no listed segment holds it;
- a number between segments, or with its bit clear, is absent.

This is a fact of the tables, not a check that failed: v1's argument (`zfile.rs:7553-7557`) extended to many segments.

**Lower bound for any key.**

- **A decoded key:** the arithmetic above, with `Greater` where the number falls in a gap.
- **Any other key:** `y = numbers_below(key)`, then the first rank whose number is at least `y`, with `Greater`.
  - `numbers_below` is v1's private `below` (`zpages.rs:4627-4660`) made `pub(crate)`. It counts the renderings below a key, which is exactly the least number whose rendering is at or after it. It takes any bytes: a key outside the prefix answers 0 or every number (`zpages.rs:4631-4638`).
  - One definition is shared by v1's `lower_bound` (`zpages.rs:4605-4619`) and v2's.
- **With listed segments:** the segment is found as in step 3 above, and inside a listed segment the bound is the byte search's.

**Rank to key.**

1. Find the segment that holds the rank.
2. Take the key:
   - for a run, render `x0 + (r − r0)`;
   - for a bitmap, a select, then render: the line by its `before` word, then the bit within the line's word. The in-word select runs through the per-CPU function of D10: `pdep`/`tzcnt` where `is_x86_feature_detected!("bmi2")` says so, a portable broadword select elsewhere;
   - for a listed segment, copy the key from the blob.
3. A number is rendered with `zpages.rs:4555`.

A walk renders once and then steps in place:

- inside a run, by `increment` (`zpages.rs:4582`);
- inside a bitmap, to the next set bit, adding the gap to the rendered digits;
- inside a listed segment, to the next key of the blob;
- it renders afresh where it crosses into the next segment.

### 1.3 The value side

Every rank r has a count `c_r` and a slot `[Off(r), Off(r+1))` in V. The slots lie back to back in rank order, unpadded, as v1's do (`zpages.rs:81-108`). Within a value segment starting at rank `r0`, each column is coded one of three ways.

**Cum, the values before a rank, relative to the segment:**

- **LIN(cnt):** `Cum(r) = cnt·(r − r0)`, so every rank has `cnt` values.
- **FOR:** a line holds `Cum` at K+1 positions; see §2.4.

**Off, the slot bytes:**

- **LIN(len):** `Off(r) = off0 + len·(r − r0)`. All slots have one length: "size and count".
- **DER(w):** `Off(r) = off0 + w·Cum(r)`. Every value of the segment has width w ≥ 1, so the bytes are the count times the width and only `Cum` is coded. Each slot is then exactly the FIXED run `encode_run` makes of c values of width w, since the slot is defined by `encode_run`'s rule (D7).
- **FOR:** a line holds `Off` at K+1 positions.

**The mode**, which says how a slot splits into values:

| mode | slot of a rank with c values | handed over as |
|---|---|---|
| ONE | c ∈ {0, 1}: empty, or the value | nothing, or `f(slot)` |
| FIX | c·w bytes of values of width w (`Off` is DER) | `each_value` with `Ext` {FIXED, count c, len c·w} |
| RUN-F | c ≤ 1: as ONE; c ≥ 2: a FIXED run of width `len/c` | `each_value` FIXED (`index.rs:212-218`; its `fixed_width` divides as the raw path does) |
| RUN-P | c ≤ 1: as ONE; c ≥ 2: a prefixed run | the walk to validate (D12), then `each_value` prefixed |
| RUN-M | c ≤ 1: as ONE; c ≥ 2: FIXED or prefixed by the rank's mask bit in its line | as RUN-F or RUN-P |

**A U segment** has no FOR column: both columns are arithmetic and touch no line. **An F segment** has one or two FOR columns.

**Every answer in these terms:**

- **count(k):** `Cum(r+1) − Cum(r)`, or 0 for an absent key. No value page is touched.
- **read_all(k):** the slot's values as the mode splits them, every page of the slot checked first.
- **values_at(r)** is the same by rank, and **scan_at(r, l)** is ranks `r..min(r+l, n)`.
  - A 0-value rank counts toward `seen` and emits nothing, as the raw walk does (`blob.rs:2339`).

### 1.4 Several values a key, and how an append works

**The form never changes.** It is a function of one sealed partition, and "a file a store names never changes" (CLAUDE.md).

**An append** goes to the memtable, then to a level-0 piece. A read merges the partition, the oldest source, with the pieces and memtables, exactly as it does over the raw form. The form answers the partition's part only.

**A merge folds the appends in.** Its output's record for the key is the concatenation of its values, oldest first (`db.rs:16506-16513` drops a key with nothing live). The new segment starts with an empty form cell, and a new form file is built from it.

**In the new file the key's slot is its whole run:**

- **v1** declines the whole partition as `NotOneValue`, and once the run passes 256 B, as `BlockBacked` (`zfile.rs:6692-6694`).
- **v2a** makes the key a U segment `(cnt 2, len 200)` between two `(1, 100)` segments: 64 bytes of table, with the next segment's `off0` carrying the shift.
- **v2b** holds a stretch of appended keys as one FIX(100) segment with `Cum` FOR. At K = 64 that is a byte a rank, and each key's count is a difference of two positions in one line.

### 1.5 Tombstones

This is D4. A partition keeps no live tombstone:

- a partition merge drops them, and writes no record for a key with nothing live;
- a promoted piece keeps its flags, but nothing older lies beneath it.

The form therefore drops C_TOMB and answers the values. A delete-only record is a rank with count 0.

A `debug_assert!` at the attach holds the premise that a form file is attached only to a partition (`level > 0`) with `tombs == false`. T9 builds a promoted piece with put and delete records.

### 1.6 Runs that lie in a block

- **Where they come from.** A run longer than `inline_bytes` (256 by default, `db.rs:6062`) goes to a block. Today both the builder and the sampler decline any partition with a block (`zfile.rs:6692-6694`, `:8027-8030`). That is the cliff a variable-length workload meets first: in scout 3's S1, P(length > 256) = (1 + 0.2615·256/73.85)^(−1/0.2615) = 8.5%, so every partition has block runs.
- **What the runs path does.** It reads such a run through the checking blob (D11), with `values_at` (`blob.rs:1305`) over `with_extent` (`blob.rs:1567-1630`). It holds the run in V as one slot: concatenated if every value has one width of at least 1, otherwise prefixed, which is `encode_run`'s own rule.
  - The blocks are asked for ahead of the walk and dropped behind it, by block id, as D11 says.
- **A failed block check** declines the build as `Unreadable`, and the partition keeps its raw form, which would answer that key's read with the same error.
- **Several extents.** A record of several extents (readers accept them; no engine writer makes them, per scout 2) is normalised the same way, to the concatenation of its extents' values in the raw form's order (`blob.rs:1701-1709`).

### 1.7 Where a segment breaks

**Key side:**

- at every missing number (v2a);
- in v2c, at the chunk choice (D10);
- at a key the codec cannot render:
  - through v2c, that declines the partition (`NotShaped`, with its sub-cause, length or field);
  - from v2e, it opens or extends a listed segment.

**Value side:**

- **Field-width ranks.** A rank whose slot is 2^16 bytes or more, or whose count is 2^16 or more, is a U segment of its own (D8).
- **An F segment is closed before 2^39 bytes or 2^31 values**, so its bases fit their fields (§2.4).
- **Code changes.** The segment is closed where the cheaper codes on either side save more bytes than one entry costs (§4.3). The greedy is held to the exhaustive optimum within a bound by T16.
- **K changes.** Within a stretch of fixed codes, a DP over 8-rank units places the closes that change K, lowering it for an outlier block and raising it again after (§4.3). T16 includes a stretch with one outlier block.
- **Uniform stretches.** A stretch of ranks with one `(count, length, fixed bit)` becomes a U segment where it is longer than the open segment's K.
  - The derivation: a U segment inside an F stretch adds two value entries (64 B), while the stretch left in F costs 64/K bytes a rank. So it pays past K ranks.
  - This is a function of the format's field sizes, not a tuned figure.

**Not at the partition's fences**, which are the engine's.

### 1.8 What v2 keeps from v1

**Reused unchanged:**

| item | where |
|---|---|
| `SegIdent`, `bind32`, `same_segment` | `zpages.rs:3677`, `zfile.rs:5482` |
| the trailer head: `seal_trailer`, `open_trailer`, `trailer_magic`, `trailer_crc` | `zpages.rs:3844-3893` |
| a value page's row word and its index mix | `stride_page_crc`, `STRIDE_PAGE_MIX` (`zpages.rs:4265`, `:4273-4275`) |
| `KeyCodec`: `detect` whole, `decode`, `render`, `increment`, `top`, and `below` made `pub(crate)` as `numbers_below` | `zpages.rs:4401-4660` |
| `sample_pages`, the verified and bad bitmaps, `fail`, `damaged` | `zfile.rs:99`, `:3507`, `:4987` |
| `StrideAttach::{Refuse, Check, Trust(seal)}` and `Built::stride_seal`, the type unchanged | `zfile.rs:3101-3115`, `:7323` |
| the write pipeline: 1 MiB pieces, asks ahead, drops behind, a cancel look a window, `name_built` | `zfile.rs:7155-7343`, `:6752` |

**Factored so that both writers share one definition:**

- **The page fill.** The page-fill loop (`zfile.rs:7236-7251`) and `stride_page_out` (`zfile.rs:7083-7096`) become one helper.
- **The page check.** `ZFile::stride_page` (`zfile.rs:5224-5237`) becomes one check over a row slice and a bind, used by both dense bodies. That changes v1's read path, so it is its own commit in S3, held equal by callgrind (finding 15).
- **The read-back.** `check_stride` (`zfile.rs:7345-7387`) takes its record leg as a parameter (D3). The Forced arm passes `stride_record` unchanged.
- **The door counter.** `ZFile::raw_reads` (HEAD `3baea42`) counts a cursor's door reads "while the file is a stride file". It counts them for every dense body, since a runs file also holds every value in V. The door's other use, a run in a block, is the block body's alone.

**Changed, and why.**

- **The attach decision gains a second value.** `Db::z_stride_attach` returns one `StrideAttach` a store (`db.rs:5975-6000`), and the table in §2.8 needs v1 files and runs files decided apart. Under `Forced`, a v1 file is `Check` and a runs file is `Refuse`.
  - So the attach takes a pair, `DenseAttach { v1: StrideAttach, runs: StrideAttach }`, from `z_stride_attach` and a new `z_runs_attach`, one definition each.
  - `z_stride_attach_sealed` (`db.rs:5995-6002`) makes the `Trust` of either from the trailer word the build kept (`Built::stride_seal`, `Shared::z_left_seals`).

**The block path's dispatch.** `if let Some(body) = self.stride()` (`zfile.rs:5190-5195`) becomes a test of the body's tag against `Block`, with an out-of-line dense dispatch. A block file pays the one compare it pays today.

### 1.9 The boundary

These stay with the block form or the raw form through v2c:

- **Keys the one codec cannot render:** mixed lengths, a changing prefix, strings, hashed keys, a stray key. Listed segments take them in v2e (D10). A partition of mostly listed keys is then a runs file whose M is its keys' bytes. Whether that serves better than the block form is the controller's comparison (§6), not the writer's.
- **Value compression.** V holds values raw: 1.0 GB at the suite's shape, against the block form's 0.72 GB. That is v2d (D15).

Nothing in v2a-c precludes either: the `features` word has room, and the value entry has a spare word.

---

## 2. The file format

All integers are little-endian. The file's length is a multiple of 4096. The name is `zpg-{id:08}-{end:016}.zpg`, unchanged; the file is written to `.tmp`, renamed, and never patched.

### 2.1 Regions

| region | offset | length | contents |
|---|---|---|---|
| V | 0 | `4096·VP`, with `VP = ⌈values_len/4096⌉` | the slots in rank order, unpadded; zeros from `values_len` to the page's end; a slot may span any number of pages |
| M | `m_off = 4096·VP` | `m_len`, then zeros to a page | ROW, KS, VS, FL (v2b), BM (v2c), LX and LK (v2e), each 64-byte aligned within M |
| trailer | `len − 4096` | 4096 | §2.6 |

- **The file's length** is `4096·(VP + ⌈m_len/4096⌉ + 1)`.
- **ROW comes first in M**, so the row sits at `4096·VP` as v1's does.
- **The row's words** are v1's: `stride_page_crc(page p, bind32, p)`.
- **`runs_layout(VP, nk, nv, nb, nl, nls, nx, xb)`** is the one definition of every table's offset within M and of the file's length. The writer writes by it, the decode holds the trailer to it, and the precheck and the tests call it, as `stride_layout` is used for v1 (`zpages.rs:4302`).
- **`served_span()` is `[0, 4096·VP)`**, V's pages: the span every residency, prefetch, drop, swap, ask and room site reads (D13). M is read once by `pread` and lives in the heap.

### 2.2 KS: the key segments, `nk + 1` entries of 16 bytes

| off | type | field |
|---|---|---|
| 0 | u32 | `first_rank`: 0 for entry 0, n for the sentinel |
| 4 | u32 | bits 0-7: kind (1 run, 2 bitmap, 3 listed); bits 8-31: `aux`, the first presence line at or after this segment (the lines of earlier bitmaps, counted); the sentinel holds `nl` |
| 8 | u64 | `first_num`: the run's or bitmap's first number; for a listed segment `numbers_below` of its first key, saturated to `u64::MAX`; the sentinel holds 0 |

- A bitmap's lines are `[aux_i, aux_{i+1})`.
- A run and a listed segment have `aux_{i+1} = aux_i`, which is checked.
- **`first_num` never falls from entry to entry.** It is equal across two entries only for a listed segment followed by a number segment (§1.2).
  - Any other tie can arise only where a listed key's count saturates beside a run that reaches the field's top. The build declines that as `TooLarge`.

**LX: the listed segments (v2e), `nls + 1` entries of 8 bytes.**

| off | type | field |
|---|---|---|
| 0 | u32 | `first_rank` of the listed segment; the sentinel holds n |
| 4 | u32 | its first key's index in LK; the sentinel holds `nx` |

- A listed segment's keys are LK's `[key_i, key_{i+1})`, and their count equals its ranks.
- LX is searched by `first_rank` only from a listed segment, which is usually one line. So KS keeps its 16-byte entries, and a partition with no listed key pays nothing for the kind.

**LK: the listed keys (v2e).** `key_off[nx + 1]` as u32, the first 0 and the last `xb`, then the `xb` key bytes, then zeros to 64 bytes. Key i is `bytes[key_off[i]..key_off[i+1])`.

### 2.3 VS: the value segments, `nv + 1` entries of 32 bytes

| off | type | field |
|---|---|---|
| 0 | u32 | `first_rank`; the sentinel holds n |
| 4 | u8 | `code`: bits 0-1 Cum (0 LIN, 1 FOR); bits 2-3 Off (0 LIN, 1 DER, 2 FOR); bits 4-6 mode (0 ONE, 1 FIX, 2 RUN-F, 3 RUN-P, 4 RUN-M); bit 7 zero |
| 5 | u8 | `log2k`: 3..=6 where a column is FOR, 0 otherwise |
| 6 | u16 | zero |
| 8 | u32 | `len`: Off LIN, the slot's length; DER, w; FOR, 0 |
| 12 | u32 | `cnt`: Cum LIN, the values a rank; FOR, 0 |
| 16 | u64 | `off0`: the segment's first slot in V; the sentinel holds `values_len` |
| 24 | u32 | `blk`: the first line in FL that is this segment's or later; the sentinel holds `nb` |
| 28 | u32 | zero (room for v2d) |

**Valid combinations,** checked at the attach:

- **DER ⇔ mode FIX**, and FIX requires w ≥ 1.
- **Mode ONE requires every count ≤ 1.** With Cum LIN that means `cnt ≤ 1`; with Cum FOR it is checked per rank.
- **A count-0 rank has a slot of length 0.** With LIN/LIN and `cnt = 0` that forces `len = 0`; otherwise it is checked per rank.
- **RUN-F needs a valid fixed width at every rank with c ≥ 2:** the slot is longer than 0 and a multiple of c.
- **RUN-P needs the slot at least c bytes long at every rank with c ≥ 2**, since a prefixed value is at least one varint byte.
- **RUN-M** applies the RUN-F test to its masked ranks and the RUN-P test to the rest.
- **RUN-M exists only in F segments**, since the mask lives in the line.
- **F segments are bounded:** slots below 2^16, counts below 2^16, and the segment's bytes and values below 2^39 and 2^31.

### 2.4 FL: the frame-of-reference line (v2b), 64 bytes

**Column header**, 64 bits:

| bits | field |
|---|---|
| 0-39 | `base`, i40: relative to the segment's start (`Off`: to `off0`; `Cum`: to 0 at `r0`) |
| 40-55 | `slope`, u16: bytes or values a rank |
| 56-61 | `b`, u6 |
| 62-63 | zero |

**The line:**

- **One coded column:** bytes 0-7 are the header, bytes 8-63 the payload (P = 56).
- **Two coded columns:** bytes 0-7 are Cum's header, bytes 8-15 Off's, bytes 16-63 the payload (P = 48).

**The payload**, from bit 0:

1. `K'+1` residuals of the first coded column (Cum where both are coded);
2. then the second column's;
3. then, in a RUN-M segment, `K'` mask bits;
4. zeros to the end.

Here `K'` is the block's ranks: K for every line but a segment's last.

**Element i of a column** is `base + slope·i + field(p_col + i·b, b)`, and every residual is at least 0.

**`field(p, b)`** loads 8 bytes little-endian at `j = min(p >> 3, P − 8)`, shifts right by `p − 8j`, and masks to `b` bits. One definition is used by the builder, the reader and the tests.

**Why the load is in bounds.**

- When the load is not clamped, the shift is at most 7 and `b ≤ 49`.
- When it is clamped, `p ≥ 8(P−8)`, so the shift is `p − 8(P−8)`. The field ends inside the payload (`p + b ≤ 8P`), so the shift plus `b` is at most 64.

**Capacity:** `(K'+1)·Σb + mask ≤ 8P`.

**Reading a slot at rank r of an F segment.**

1. The line is `FL[blk + ((r − r0) >> log2k)]`, and `i = (r − r0) & (K − 1)`.
2. Up to four `field` reads come from that one line: Cum at i and i+1, and Off at i and i+1.
3. Then `c = Cum(i+1) − Cum(i)` and the slot is `[Off(i), Off(i+1))`. Under DER the slot is `[off0 + w·Cum(i), off0 + w·Cum(i+1))`.

**Bits a rank, `64·8/K`:** 8 at K = 64, 16 at K = 32, 32 at K = 16, 64 at K = 8.

### 2.5 BM: the presence line (v2c), 64 bytes

| bytes | field |
|---|---|
| 0-3 | `before` u32: present numbers in this segment's lines before this one |
| 4-7 | `sub` u32: bits 0-8 = ones in words 0-1, bits 9-17 = ones in words 0-3, bits 18-26 = ones in words 0-5; bits 27-31 zero |
| 8-63 | `w0..w6`, u64: bit j ↔ number `x0 + 448·(line − first) + 64·word + j`; bits past the segment's last number are zero |

**The ones below word `k`:**

| k | ones below |
|---|---|
| 0 | 0 |
| 1 | `popcnt(w0)` |
| 2 | `sub[0..9)` |
| 3 | `sub[0..9) + popcnt(w2)` |
| 4 | `sub[9..18)` |
| 5 | `sub[9..18) + popcnt(w4)` |
| 6 | `sub[18..27)` |

That is at most two popcounts a rank (this is "lines"' layout). A count is at most 384, which fits in 9 bits.

**Rank and select by CPU (D10).**

- The line's rank, two popcounts and a masked one, is one function with two bodies: a portable one, and on x86-64 one compiled under `#[target_feature(enable = "popcnt,bmi2")]`. At the baseline x86-64 target `count_ones` is a software sequence.
- The line's select is one function with two bodies too: `pdep` then `tzcnt`, or a portable broadword select.
- The bodies are chosen once a process, as `block.rs:226-236` chooses the CRC.
- T6 runs both bodies against one oracle, and S2 prices both.

### 2.6 The trailer, the last 4096 bytes

| off | type | field |
|---|---|---|
| 0 | [8] | magic `SUPDZRN1` (`RUNS_MAGIC`) |
| 8 | u32 | version, 1 |
| 12 | u32 | page size, 4096 |
| 16 | u32 | `features`: bit 0 FL present; bit 1 BM present; bit 2 LX and LK present (v2e); bit 3 coded values (v2d); every other bit 0 |
| 20 | u8 | key kind (`KeyKind::code`: 1 decimal, 2 big-endian) |
| 21 | u8 | D |
| 22 | u8 | L |
| 23 | u8 | P |
| 24 | u64 | VP, the value pages |
| 32 | u64 | `values_len` |
| 40 | u32 | nk |
| 44 | u32 | nv |
| 48 | u32 | nb (0 unless bit 0) |
| 52 | u32 | nl (0 unless bit 1) |
| 56 | u64 | m, the total values |
| 64 | u64 | `m_off` = 4096·VP |
| 72 | u64 | `m_len` |
| 80 | u32 | `m_crc` = `crc32c(M[0..m_len]) ^ bind32` |
| 84 | u32 | `bind32` |
| 88 | [96] | `SegIdent` (`zpages.rs:4148-4212`), whose `nkeys` is n |
| 184 | [64] | the prefix: P bytes, then zeros |
| 248 | u32 | nls, the listed segments (0 unless bit 2) |
| 252 | u32 | nx, the listed keys (0 unless bit 2) |
| 256 | u64 | xb, the listed keys' bytes (0 unless bit 2) |
| 264-4091 | | zeros |
| 4092 | u32 | CRC-32C of `[0, 4092)` (`seal_trailer`) |

- **n is held once:** it is the segment's `nkeys`, as in v1 (`zpages.rs:4715`).
- **Every size is written, then held to `runs_layout`**, never trusted.
- **The trailer's CRC covers `m_crc`, and M begins with ROW.** One word therefore covers every region, so `Trust(seal)` still means "every page" (`zfile.rs:3415-3422`).

### 2.7 Decode and attach checks, in order

**1. The decode (Damage).**

- The length is whole pages, and at least one.
- `open_trailer(RUNS_MAGIC, 1)` checks length, magic, version, page size and CRC.
- The codec is as v1's decode checks it (`zpages.rs:4765-4790`).
- `1 ≤ n ≤ u32::MAX`, `1 ≤ nk ≤ n` and `1 ≤ nv ≤ n`. `nb`, `nl`, `nls`, `nx` and `xb` are 0 unless their bit is set.
- `VP = ⌈values_len/4096⌉ ≤ u32::MAX`.
- `m_off`, `m_len` and the file's length equal `runs_layout`'s.
- The zeros are zeros.
- `bind32` matches its fields.

**1a. Features (Identity).** Every `features` bit set must be one this reader knows. An unknown bit is `Reject::Identity`, not Damage.

- By the enum's own words, Damage is "a file that is not a form file anyone can use", and Identity is "a form file, whole, of something else ... an engine whose layout differs" (`zfile.rs:142-147`).
- A later stage's file read by an earlier stage's engine is whole, so it must not move `rejected_damage` (`zfile.rs:5878-5880`). Both kinds store `DECLINED_REFUSED` and rebuild (`db.rs:38035-38040`), so the rebuild is the same either way.
- The check comes after the trailer's CRC, so a flipped feature bit is Damage, by the CRC, before it is ever read as a feature.

**2. Identity.** `same_segment` (`zfile.rs:5482`).

**3. M (Damage).**

- M's span is read with one `pread` into a 64-byte-aligned heap buffer, never through the mapping, which the attach has already advised random (`zfile.rs:3209-3216`).
- `m_crc` is checked over the buffer, then the zeros after M.
- M's span is dropped from the page cache with `POSIX_FADV_DONTNEED`. No mapping holds its pages, since the read did not fault them in.
- Then the structural pass, O(|M|), over every entry and line:
  - **KS:**
    - ranks start at 0, rise strictly and end at n;
    - kinds are allowed by the features;
    - `first_num` never falls, and is equal across two entries only for a listed segment then a number segment;
    - runs are maximal, and a bitmap's next `first_num` is at least `x0 + 448·lines`;
    - the last number is at most `codec.top()` (`zpages.rs:4514`);
    - `aux` is consistent.
  - **LX and LK (v2e):**
    - LX's ranks are exactly the listed KS entries' first ranks, and its key indices rise by each segment's ranks to `nx`;
    - `key_off` rises from 0 to `xb`;
    - each listed segment's keys rise strictly by `key_cmp`;
    - no listed key is a rendering: it fails `decode`, or its decoded number does not render back to it;
    - a listed segment's `first_num` equals `numbers_below` of its first key, saturated;
    - across every boundary between segments, the last key of one is below the first key of the next. The two keys are rendered or taken from LK, so the pass costs O(nk) renders.
  - **BM:**
    - each line's popcounts equal its `before` and `sub` words, and the segment's total equals its ranks;
    - the first and last lines are non-empty, bit 0 of the first line is set, and the bits past the end are zero.
  - **VS:**
    - ranks rise strictly from 0 to n;
    - `off0[0] = 0`, and the sentinel holds `values_len`;
    - each code combination is valid (§2.3);
    - `blk` is consistent;
    - a U segment's `off0[j+1] = off0[j] + Off_rel(end)`, by checked arithmetic.
  - **FL**, decoding every line:
    - each coded column is non-decreasing;
    - line j's position 0 equals line j−1's position K;
    - the first position is 0;
    - the last line's `Off` position K' equals `off0[next] − off0`;
    - the sum of the segments' `Cum` totals equals m;
    - each rank's count and slot obey the mode (§2.3);
    - slots and counts are below 2^16;
    - unused bits are zero.

**4. Identity: the cross-check against raw**, skipped under `Trust(seal)`.

- **The ranks checked:**
  - the slot starting on each page of `sample_pages(VP)`;
  - the first rank of each segment of `sample_pages(nk)` and `sample_pages(nv)`, and the rank before each, which is a hole's left edge. A listed segment's first rank is among them by the same rule;
  - the first and last ranks.
- **The cost.** At most 16 + 32 + 32 + 2 ranks, which collapses to v1's 18 where `nk = nv = 1`.
- **The M read's cost.** One `pread` of M (0.7 MB a partition with FOR, about 11 MB U-only for varlen, §2.9) and its CRC. §10 Q8 times both, with the structural pass, at a reopen.
- **For each rank r:**
  - every page of its slot is checked against its row word;
  - raw `key_at(r)` equals the form's key at r (rendered, or from LK);
  - raw `values_at(r)` equals the form's values, as a sequence;
  - `rank_of(raw key)` is `Some(r)`.
- **The gaps (v2a on).** Where the key side has gaps, the numbers at `sample_pages` over the gaps are checked: raw `count(render(x))` is 0. That is the gap's first missing number for each gap `sample_pages(gaps)` picks, the existing definition of a sample and no new constant.

**5. The heap's derived indexes**, built after the checks and never stored.

- An implicit B-tree, one 64-byte line a node (8 u64 or 16 u32 keys), over:
  - KS `first_num`, KS `first_rank` and VS `first_rank`, where a table exceeds one line;
  - each bitmap segment's `before` words, for select;
  - VS `off0`, for the attach's sample and the scan's page spans.
- One helper, `Ladder`, serves all of them.
- A table that fits in one line is scanned whole: a fact of the line size, not a tuned threshold.

### 2.8 Which engine does what with which file

| engine and option | `SUPDZPG2` (block) | `SUPDZST1` (v1) | `SUPDZRN1` (v2) |
|---|---|---|---|
| before the stride form | attach | Damage at the magic, rebuild | Damage at the magic, rebuild |
| a8069e0 or later, `ZStride::Off` | attach | Identity, rebuild (`zfile.rs:3229-3234`) | Damage at the magic (`zfile.rs:3266`), rebuild |
| a8069e0 or later, `ZStride::Forced` | attach | attach | Damage at the magic, rebuild |
| v2 engine, `Off` | attach | Identity, rebuild (`DenseAttach.v1` is `Refuse`) | Identity ("a runs form file, which the store does not take"), rebuild (`DenseAttach.runs` is `Refuse`) |
| v2 engine, `Forced` | attach | attach (`Check` or `Trust`) | Identity, rebuild (`DenseAttach.runs` is `Refuse`) |
| v2 engine, `Runs` | attach | attach (`Check` or `Trust`) | attach (`Check` or `Trust`) |
| v2 engine of an earlier stage, `Runs`, a file with a later stage's feature bit | attach | attach | Identity at the features check, rebuild |

- **Every refusal** is `DECLINED_REFUSED`, then a rebuild renamed over the name. A checkpoint's hard link keeps the old inode.
- **Within v2,** a stage sets a `features` bit only where the file holds that element, so a v2a reader refuses a v2b file with FL and reads one without. The refusal is Identity (§2.7, step 1a).
- **The version stays 1** until a field's meaning moves.

### 2.9 Sizes, by arithmetic and from `forfit.py`

**The suite (10M keys, W = 100) gets a v1 file:**

- `stride_layout(10^7, 100)`: 244,141 value pages, plus a row of 976,564 B (239 pages), plus the trailer;
- 244,381 pages, **1,000,984,576 B**, the same under either arm.

**Per partition:** 29 partitions at 10M means about 345k keys and 34.5 MB of values each.

| shape | v2 file | M beyond ROW, per partition | M at 10M |
|---|---|---|---|
| sparse, isolated holes at 10^-4 | runs + U | ~36 KS entries + 2 VS = 0.6 KB | 17 KB |
| odd widths at 10^-4 (one rank of 50 B) | runs + U | ~71 VS entries = 2.3 KB | 66 KB |
| holes at 10%, ranges of 1000 numbers | runs + U | ~39 KS = 0.7 KB | 20 KB |
| holes at 10%, isolated, v2a | runs | ~34.5k KS = 552 KB + Ladder | 16 MB |
| the same, v2c | bitmap | ⌈383k/448⌉ = 856 lines = 55 KB | 1.59 MB |
| varlen GPD [1, 4096], v2a (U only) | U at almost every rank | ≈ 32 B a rank = 11 MB | 320 MB |
| the same, v2b | one F segment, Off FOR one column, K = 32 (b ≤ 12 ≤ 13) | 345k/32 × 64 = 690 KB | **20 MB** |
| uniform 50-150, v2b | Off FOR one column, K = 32 (b ≤ 9) | 690 KB | 20 MB |
| multi, 1+Poisson(1) × 100 B (5M keys) | FIX(100), Cum FOR one column, K = 64 (b ≤ 5 ≤ 6) | 345 KB | **5 MB** |
| multi, varlen values (5M keys) | RUN-M, two columns, K = 16 (all blocks fit; K = 32 none) | 1.4 MB | 20 MB |
| a stray key every 100,000 numbers (`sparse` with `stray`), v2e | runs + a listed segment for each, about 3.5 a partition | each: two KS entries more (32 B; the stray splits its run), one LX entry (8 B), the key and its offset (21 B), so about 210 B | ~6 KB |

**V** is the values' own bytes. For the multi shape that is the runs' bytes: prefixed runs carry their varints, about 1-2 B a value.

**Where M's bytes sit.** M is heap, and the page cache holds V alone (D13). The 320 MB of the U-only varlen row is anonymous memory the budget subtracts. That is why the `zruns`-against-`zrunsfor` pairing reads `lagmem` and the census's `zf_heap` beside its rates (§10, Q9).

---

## 3. Read paths

**Estimates and how they are checked.**

- Instruction counts below are *est.* from reading v1's code paths.
- "lines" here means cache lines beyond the path every form shares: the epoch pin, the state load, the partition routing, `part_read_z` (`db.rs:24113-24153`) and `z_on` (`db.rs:24044`).
- Each estimate is held by callgrind on `bench/examples/read_only.rs` with `--toggle-collect` on the engine's `get`, with the probe's source identical across arms, and the price is quoted from the function that holds the inlined read (CLAUDE.md, "A probe's total over a whole read is an inlining result").

### 3.1 Point read (`read_all`)

1. `damaged()` gives `Out`, before anything else, as every v1 arm does (`zfile.rs:5249-5251`).
2. `codec.decode(key)`. A key that does not decode answers `Served{0}` where features bit 2 is clear. Where it is set, the key goes to the listed search of §1.2: `numbers_below`, then the `Ladder`, then one or two LX/LK lines.
3. **The key side:**
   - KS within one line (`nk ≤ 4`): scan it.
   - Otherwise: the `Ladder` over `first_num`.
   - A run: arithmetic. A bitmap: one presence line, at most two popcounts, through the per-CPU rank (§2.5). A listed segment: a byte search of its keys.
   - Absence answers `Served{0}` and touches no value page.
4. **The value side:**
   - VS within one line (`nv ≤ 2`): direct.
   - Otherwise: the `Ladder` over `first_rank`.
   - Then the columns (§2.4).
5. Every page of `[Off(r), Off(r+1))` is checked by its verified bit or its row word, through the one page check (§1.8). A failure calls `fail(p)` and answers `Fallback` with nothing emitted.
6. **The mode:**
   - ONE and a count of 1: `f(slot)`.
   - FIX and RUN-F: `each_value` FIXED.
   - RUN-P: the validating walk, then `each_value`.
   - RUN-M: by the mask bit.

   The answer is `Served{c}`.

| shape | key side | value side | value lines (cold) | instructions over v1's (~110-130, *est.*) | dependent misses |
|---|---|---|---|---|---|
| v1 file | — | — | 2.5 mean at W = 100 (o = 100r mod 64 takes 16 values; o ≥ 32 touches 3 lines) | 0 | 1 |
| few breaks (all U, tables in one line) | the body's line, hot | the same line | as v1 | +5-10 | 1 |
| varlen, one-column F | 0 | 1 F line (20 MB of FL at 10M: L3) | ⌈(o+len)/64⌉ | +15-20 | 2 |
| multi, FIX with Cum F | 0 | 1 | ⌈(o+c·w)/64⌉ | +15-20 | 2 |
| multi varlen, two-column F, RUN-M | 0 | 1 | ⌈(o+len)/64⌉ | +25-30, plus ~5 a value for the walk | 2 |
| holes at 10% (bitmap) | 1 presence line (55 KB a partition: L2) | 0 | as v1 | +15-20 | 2 |
| holes at 0.1-1% (runs, `Ladder` over 0.3-3.4k runs) | 1-2 lower levels (L2) | 0 | as v1 | +20-40 | 2-3 |
| a listed key (v2e) | `numbers_below`, the KS line, one LX line, one or two LK lines | 0 | as v1 | +60-120, for a stray key's reads alone | 3 |
| a miss (hole or gap) | the same | none | 0 | the key side alone | 1 |

**Page straddles.** 100r mod 4096 takes 1024 values, and 24 of them (those ≥ 3997) straddle a page, so 2.34% of reads check two pages. In general a slot of length `len` straddles with probability about `len/4096`.

**Under a bound.** M is heap, counted in the anonymous memory the budget subtracts. A read faults only on its slot's pages, as v1's does. The raw form's read touches two populations, the hash slot and the record (`model_raw`, `zselect.rs:356-366`).

### 3.2 Count

Steps 1-4 of §3.1, answering `Cum(r+1) − Cum(r)`, or 0 for an absent key. It touches no value page in any shape. v1 touches none either (`zfile.rs:5271-5280`).

### 3.3 Seek, `owner_span_from`, `cut_in`

- `decode(key)`, or `numbers_below(key)` as a u128.
- Then the key side gives `(r, Equal)` or `(r, Greater)`. Past every segment the answer is `(n, Greater)`.
- These touch heap only: no value page, and no raw ordered index, whose heads `drop_raw_pages` dropped for a served partition (`db.rs:11403-11413`).
- `owner_span_from` and `cut_in` are written over `seek`, as v1's are (`zfile.rs:5435-5471`).
- **`owner_span_from` uses its hint.** v1 ignores it (`zfile.rs:4923-4926`). The block form keeps the fence entry it found in the hint and gallops from it (`zfile.rs:4930-4933`), because the settle files a sorted batch of keys over one partition (`Reader::lazy_block`).
  - The runs body keeps the key segment it found in the hint. The next key first tries that segment and the one after it, and only then the `Ladder` from the top.
  - Without the hint, a partition of 34.5k runs would pay a full descent a key where one or two compares serve a sorted batch.
  - T15 holds a sorted batch to the same answers with the hint and without it.

### 3.4 Scan from a rank, and the cursor

**At the start rank:**

- one key-side placement and one value-side placement;
- the key rendered once;
- the current F line decoded into the walk's window of K+1 positions.

**Per rank:**

- **The key:**
  - `increment` inside a run;
  - inside a bitmap, the next set bit, by `trailing_zeros` and clearing the lowest bit over the line's words and then the next line, with the gap added to the rendered digits. These compile to `tzcnt`/`blsr` where the CPU has them, and to a portable sequence elsewhere;
  - inside a listed segment, the next key of LK;
  - a fresh render at a segment crossing.
- **The positions:** the next window entry, or for U segments a running add.
- **The pages:** a check where the walk first enters a page. This is v1's check through `values_at` per value (`zfile.rs:5328-5333`), kept as the default for parity. The once-a-page check is a lever (§10 Q16).

**What a scan touches:** V's bytes over 64 lines, plus one FL line per K ranks, plus one presence line per ~400 ranks.

**Under `Options::zpages_scan_prefetch`** (off, `db.rs:6112`), the walk's exact page span from `Off` is asked once, behind the same gate as v1's `stride_ask_ahead` (`zfile.rs:5359`). Whether a dense walk asks with the gate off is S0's measurement for v1 (Q1, Q17), and the runs body follows v1's answer.

**The cursor, `Group::Runs`,** sits beside `Group::Stride`. It holds the rank, the key segment and value segment indexes, the bitmap line and bit or the listed key's index, the number, the rendered key in the inline buffer v1 added at `192db37`, and the line window. Its door reads count in `ZFile::raw_reads` as a stride cursor's do (§1.8).

- Every placement (`at`, `at_wanting`, `unplaced`, `seek`, `skip_to`) is the arithmetic above.
- `walk_until` takes `until`'s rank once, as `stride_walk` does.
- The block cache's builds and the merge paths read a covered partition only through the cursor and through `values_at`/`key_at_into`, as for v1.

---

## 4. The builder

### 4.1 The plan

- `z_stride_ask` (`db.rs:5964-5969`) returns `(OrBlock, Runs)` under `ZStride::Runs`; `Forced` is unchanged.
- `BuildPlan` gains `dense: Dense {V1, Runs}` and `verify: bool`, which is the store's `opts.segment.checksums`.
- **Under Runs, and only under Runs,** `build_file_seen` (`zfile.rs:6661`):
  - opens the blob with `verify_checksums: plan.verify` (D11);
  - takes the `blocks() > 0` decline (`zfile.rs:6692-6694`) past the runs walk, into the block body's path. A runs build that declines falls through to the block body in the same call, which declines `BlockBacked` there as it does now.
- **Under `Off` and `Forced` the order is unchanged.** The `blocks() > 0` check stays where it is, ahead of v1's walk. So a block-backed partition under `Forced` is still recorded `BlockBacked`, and never `NotOneValue` from v1's walk.
  - The stride oracle relies on that order: its comment says a run in a block "is the block form's decline and counted there alone" (`tests/zpages.rs:14650-14655`).
  - T14 asserts the Forced arm's reasons unchanged on a store with block runs.
- **Refusals at open** (`check_options`, `db.rs:14884-14891`):
  - `ZStride::Runs` beside the three re-code policies, as `Forced` is;
  - `ZStride::Runs` beside `ZPages::Auto`, until S12 (D18).

  T28 holds both.

### 4.2 One walk

**The precheck, O(1), from ranks 0 and n−1:**

- n ≥ 1, and the floor (`Small`).
- **The codec, by a fixed order that reaches v1's codec wherever v1 has one:**
  1. `KeyCodec::detect(first, last, n)` (`zpages.rs:4448-4491`), whole. Where it succeeds, the keys between are a contiguous run under it, and its codec is the file's: v1's own.
  2. Otherwise, the key side starts under decimal. That holds where both ends' fields are ASCII digits and `D ≤ STRIDE_DECIMAL_MAX`, as `detect`'s first candidate is tested.
  3. Otherwise, or where the walk under decimal meets a field byte that is not a digit, under big-endian where `D ≤ 8`. A key side abandoned midway is begun again from rank 0 over the keys alone, which are warm: the value side does not depend on the codec.
  4. Otherwise `NotShaped`, with the sub-cause `detect` returned: `Length` or `Field`.
- **Why the order never refuses a partition v1 serves.**
  - If v1 serves it, `detect` succeeds, and step 1 takes v1's codec.
  - When `detect` takes big-endian, decimal failed its count. Then either some key's field has a byte that is not a digit, and step 3's restart reaches the same big-endian codec; or every key is digits. The second case cannot happen: n consecutive big-endian numbers that are all digit strings are consecutive decimal numbers too, since a big-endian step from a digit to a digit never carries. Then decimal would have counted n.
  - So T12's byte-identity holds on every v1-shaped input, the 252-key big-endian run from `"05"` to `"10"` included (T12(b)).
- **Why decimal first where it holds.** For keys that are all digits, decimal's segments are never more than big-endian's. Two present keys adjacent as big-endian numbers are adjacent as decimal numbers, by the same no-carry argument, so every big-endian run lies inside a decimal run. The order is a bytes rule, not a preference.
- **v2e.** Where the codec from ranks 0 and n−1 fails, ranks 1 and n−2 are tried: one stray key at either end, such as the empty key or a sentinel.
  - Where the two pairs give two codecs, the walk carries both key sides and keeps the one with fewer M bytes, the writer's own rule. The value side is shared.
  - The cost is a second decode and render a key, paid only where the ends disagree.

**The walk.** Every rank, in rank order through the directory, never by stride, because direct-run segments keep commit markers in the record region (`zfile.rs:7131-7133`). The record region is asked for ahead a window at a time and dropped behind the read-back, as v1's walk does (`zfile.rs:7213`, `:7288-7303`). The blocks the runs lie in are asked for ahead and dropped behind by block id (D11). For each rank:

1. **The key.**
   - `x = decode(key)`, and `render(x) == key` byte for byte. These are the two quantities of `stride_key_is` (`zfile.rs:7107-7121`), generalised to no expected x.
   - A key of another length is `NotShaped`/length. A field byte that is not the kind's is `NotShaped`/field, after the big-endian restart of step 3 above where it applies. There is no prefix case: a key between the first and the last shares their prefix (`zpages.rs:4444-4446`).
   - From v2e, such a key opens or extends a listed segment instead, and the walk goes on.
   - Then push to the key side: a run is extended where `x = x_prev + 1`, and a new run starts otherwise. `x > x_prev` holds because keys ascend and renderings are monotone; a `debug_assert!` checks it.
2. **The record.**
   - The fast path is `compact_fixed_tomb_at` (`flatindex.rs:597-630`), and anything else goes through `parse_record` (`flatindex.rs:632`). The flag is ignored.
   - **One inline extent:** c is the extent's count.
     - c = 1: the slot is the value (a FIXED run's bytes, or a one-value prefixed run's bytes after its varint, which may be empty).
     - c ≥ 2, FIXED: the slot is the run's bytes as they lie.
     - c ≥ 2, prefixed: the varints are walked to count exactly c values ending at the run's length. Where the values are not all of one width of at least 1, the slot is the run's bytes as they lie. Where they are (a foreign run `encode_run` would have written FIXED), the slot is `encode_run`'s FIXED run of the values (D7).
   - **A block run, or several extents:** the values through the checking blob's `values_at` into scratch, then the same normalisation, with `encode_run` for c ≥ 2. A block that fails is `Unreadable`.
3. **The facts, run-length coded.** The walk keeps v1's running state alone (one key run, one value a rank, one width) for as long as every rank so far is v1-shaped. That is the same state `write_stride` keeps, so a v1-shaped partition's build has v1's fresh pages.
   - At the first rank that breaks it, the ranks before it become one fact run, `(count r, c 1, len W, fixed)`, and from there the facts are run-length coded: `(ranks, c, len, fixed, w)` per run of equal facts, 17 bytes a run.
   - Per-rank facts arise only where the facts change at every rank (varlen: about 6 MB for a partition of 345k, freed at the end).
4. **The bytes.** The slot's bytes go through the shared page fill (§1.8), which takes each page's row word as the page completes.

**At the end:**

- **A v1 file** where `nk = 1`, every c is 1, and every length is one W in `1..=4096`: the row, its padding and `StrideTrailer::encode` (`zpages.rs:4737`). `coding = Stride`, and T12 holds it byte-identical to `write_stride`'s.
  - It is read back by `check_stride` with the runs walk's record leg (D3): the key from the record, the value from the inline run or the checking blob, the flag ignored.
- **Otherwise:** the segmenter (§4.3), then M and the `RunsTrailer`. `coding = Runs`.

### 4.3 Every choice the writer makes, and the quantity behind it

**The objective is the fewest bytes of M**, and V is fixed by the slots. Ties go to the fewer lines a read touches. There are two reasons:

- under a bound, M is resident anonymous memory and a byte of it is a byte the page cache does not have;
- in core, smaller tables are the more cache-resident ones.

Where a choice trades lines for bytes beyond a tie, the trade is a measured open question (§10), not a writer's threshold.

| choice | rule | constant | what the constant stands for |
|---|---|---|---|
| v1 file or runs file | v1 wherever it applies (§4.2) | none | v1 has the same V, no tables, fewer instructions |
| runs (v2a) | a new run exactly at a missing number | none | the canonical table |
| run or bitmap (v2c) | per aligned chunk of 64 presence lines (28,672 numbers from the first number), runs at 16 B each against ⌈span/448⌉ lines at 64 B, the smaller; ties to the bitmap; adjacent bitmap chunks are one segment | 64 lines | a 4 KiB heap page of lines. T17 holds the bytes against an exhaustive split on small inputs, within one entry per change of kind. The chunk itself is priced in S2 at 8 to 256 lines over each workload's law against the exhaustive split, and ships where the bytes stop falling (Q21) |
| number or listed (v2e) | a key that decodes and renders back to itself is a number; any other key is listed; adjacent listed ranks are one segment | none | exactness: a listed key is exactly a key no number renders |
| codec (v2e, where the ends disagree) | of the two candidates the walk carried, the one with fewer M bytes | none | the objective |
| field-width ranks | slot ≥ 2^16 or c ≥ 2^16: a U segment of its own | 2^16 | the slope's u16; makes K = 8 always fit (D8) |
| F segment end | before 2^39 bytes or 2^31 values | the base's i40 and the Cum base | field widths |
| codes of an open segment | the cheapest codes holding every rank so far: Cum LIN if one count, else FOR. Off DER(w) if every value has width w ≥ 1, else LIN if one slot length, else FOR. Mode: FIX if DER; else ONE if every c ≤ 1; else RUN-F, RUN-P or RUN-M by the fixed bits of the ranks with c ≥ 2 | none | |
| widen the codes in place or close | where the next rank would widen the codes, compare by exact bytes (a) the segment so far re-coded to hold it, against (b) the segment closed there, plus one entry (32 B), plus a new segment from there; take the smaller, and on a tie (a), which keeps the lines a read touches. This pass fixes the stretches of fixed codes and the U stretches; K is the next pass's | 32 B, the entry | the format's entry size |
| U stretch | a stretch of one `(c, len, fixed)` longer than the K the stretch around it would take is cut out as a U segment | K | two entries (64 B) against 64/K a rank |
| K within a stretch of fixed codes | where every block of the stretch fits at K = 64, one segment at 64 and nothing more. Otherwise a DP over the stretch's 8-rank units decides it, with state (unit, K of the open segment). Its moves: extend by one block of K ranks where that block fits the line's capacity (§2.4) at a cost of 64 B; close with a last, partial block (64 B), then open at any K for one entry (32 B). The DP's least total is the stretch's segments and their K. So K falls for an outlier block and rises again after it wherever one entry pays, which over 64 ranks it always does: from K to 2K saves 64·64/(2K) B against the entry's 32 | the 448 or 384 payload bits; 32 B | format facts. Bytes a rank are 64/K, and a larger K only raises b |
| the fit test inside the DP | a block fits at K when the least-range slope's b (below) fits the capacity. The endpoint slope is tried first: its range bounds the least range from above, so where it fits, the block fits. Tests are cached per (unit, K) | none | the DP's cost: S2 times it with the build (Q5), and the builds' `cpu_us` reads it in the engine |
| slope and base of a column in a line | the integer slope minimising `max_i(X_i − w·i) − min_i(X_i − w·i)`. The range is convex in w (max of affines minus min of affines), so a ternary search over `[min step, max step]` finds it; `base` is the minimum; `b = bit_length(range)` | none | the least b any linear model of the block admits |

**The code pass is greedy and not proved optimal; the K pass is exact given the code pass's stretches and 8-rank boundaries.**

- T16 holds the whole segmenter's bytes within one entry (32 B) per close of the exhaustive optimum, on inputs up to 200 ranks. That includes a stretch with one outlier block, where a greedy that only lowers K would hold the low K to the end.
- §10 Q5 measures the gap on the workloads' distributions against a DP over codes as well.

**There is no cost-based decline.** The writer builds what it can represent exactly and reports exact sizes. Whether a runs file serves a partition better than raw, block or nothing is the controller's question (§6).

### 4.4 The read-back before the name (`check_runs`)

The file is read back from the tmp file's bytes alone (`RunsBody::from_bytes`), then `same_segment`. Then, at every rank:

- (a) the values equal raw's, as a sequence;
- (b) `key_at_into(r)` equals raw's;
- (c) `rank_of(raw key)` is `Some(r)`;
- (d) `count` equals raw's.

Then:

- for every gap between segments, its first and last missing numbers decode to absent, and so does each end's neighbour outside the partition;
- under v2c, every clear bit of every presence line answers absent;
- under v2e, for every listed key, the key with one zero byte appended answers `count` and `seek` as raw's `count` and lower bound do. That key sorts directly after the listed key, so it probes the edge of every listed segment against the oracle;
- `seek` below the first number gives 0, and above the last gives n;
- `scan_at(0, n)` equals raw's walk, `seen` included;
- `owner_span_from` over the keys in rank order, with the hint carried, gives what it gives with no hint (§3.3).

**A v1 file the runs writer wrote** is read back by `check_stride` with the runs walk's record leg (D3), not by this check.

**Leg (c) is a leg of its own here.** v1 argues its composition is no third leg (`zfile.rs:7337-7344`). Here key-to-rank goes through the `Ladder` over numbers and rank-to-key through the `Ladder` over ranks, two searches that can disagree.

**The records are warm** from the walk, and the drop behind the walk is the check's, as in v1 (`zfile.rs:7137-7142`).

**A failure is `SelfCheck`**, with nothing renamed. A `#[cfg(test)]` seam (`plan.damage`, `zfile.rs:7274-7276`) damages the output between the write and the check.

**`Built`** carries `coding`, the counts `(nk, nv, nb, nl, nls, nx)` and the M bytes, and `stride_seal = trailer_crc` (`zfile.rs:7323`). The seal is kept by name until the attach, as v1's is (`Shared::z_left_seals`), so files built ahead or at a merge's finish attach as `Trust` (T29).

### 4.5 Declines (`StrideNo`, extended)

| reason | when |
|---|---|
| `NotShaped` | through v2c, a key the one codec cannot render; its sub-cause (length or field) is counted apart. From v2e, only where neither pair of ends gives a codec |
| `Small` | under the floor (`ZMIN_RECS`) |
| `SelfCheck` | the read-back failed |
| `Unreadable` (new) | a block run failed its checksum |
| `TooLarge` (new) | n > u32::MAX, VP > u32::MAX, nl ≥ 2^24, a table past M's u32 fields, or a `first_num` tie other than a listed segment then a number segment (§2.2). A `#[cfg(test)]` seam lowers the limits so the reason is reached (T9(u)) |

`Holes`, `NotOneValue`, `Width`, `Tomb` and `BlockBacked` stay v1's reasons. Under Runs they are never given, since each is a shape v2 holds. The oracle's destructuring of the reasons (`tests/zpages.rs:14656`) changes with the enum.

---

## 5. Exactness and checks

**Proven at build, by the walk and the read-back:**

- every key is its number's rendering, or is listed and is no rendering;
- the segments list exactly the present numbers and the listed keys;
- every slot is exactly raw's values, and every count raw's count;
- key-to-rank and rank-to-key agree at every rank;
- every gap's edges are absent;
- the bytes are bound to this segment's content by `bind32`, whose input includes the raw key section's row CRC (`zpages.rs:3677-3683`).

**Checked at the attach** (§2.7): the trailer and layout, identity, `m_crc`, the whole structure of M, and up to 82 sampled ranks against raw in both directions, values included. Under `Trust(seal)` the sample is skipped.

**Checked at first touch:** each value page against its row word, through the one page check.

**Checked before a value is emitted** (D12):

- a RUN-P slot's varints are walked to exactly c values ending at `len`;
- a RUN-F slot's `len mod c = 0` and `len > 0` were checked at the attach.

**What a forged file with every checksum recomputed does:**

| forgery | caught by |
|---|---|
| a VS entry's `off0` | the structural pass, always. `off0[0] = 0`, the sentinel is `values_len`, and every segment's end is checked against the next segment's `off0`, so a single `off0` cannot move without breaking one of them |
| a U segment's `len` and the next `off0` forged together, keeping continuity (every slot of the segment and every slot after it shifted) | continuity at the sentinel unless a later entry compensates; then the sample, wherever a sampled rank lies in the shifted span; otherwise **served** |
| a run's `first_num` (it moves that run's numbers alone, since runs carry their own) | the structural pass where maximality or monotonicity breaks; a forgery that keeps a gap on both sides passes it, and is caught by the sample only where a sampled rank lies in that run. With 34.5k runs a partition (§2.9), that is rarely: otherwise **served** |
| a KS `first_rank` moved by one with `first_num` moved to match (one key moved across a gap) | the same: maximality where the gap was one number; otherwise the sample where it lands, else **served** |
| a listed key changed to another non-rendering key that keeps the order | the sample where it lands; otherwise **served** |
| a listed key changed to a rendering | the structural pass: a listed key that renders is refused |
| a presence bit added or removed | the popcount against `before`/`sub` |
| a presence bit moved within a line, counts kept | the sample, where it lands; otherwise **served** |
| an interior FL position moved, monotonicity and continuity kept (two slot boundaries shifted) | a RUN-P slot's validating walk, often; the sample, where it lands; otherwise **served** |
| two counts in one block exchanged, totals kept | the sample, where it lands; otherwise **served** |
| a value page forged with its row word | the sample, where it lands; otherwise **served** |

The rows marked **served** are the limit v1 and the block form state (`zfile.rs:3386-3389`). T20 asserts each case either refused or served, so the limit is a test and not a claim.

The key side's limit is wider than v1's. v1 has one run and checks both its ends against raw. A runs file with many segments has many interior boundaries, and the sample covers `sample_pages(nk)` of them. That is the price of holding a holey partition at all, and T20 states it as served.

**The threat these checks cover** is corruption, and a stale file under a reused segment id. The CRCs and `bind32` cover both: every page of a file built from other content fails.

**Fallback.**

- Any failed page check calls `fail(p)` and takes the whole file out of service (`zfile.rs:3507`).
- The read that met it answers `Fallback` with nothing emitted, and every later read answers `Out`. Both go raw at the dispatch (`db.rs:24140-24150`).
- The raw pages were dropped at the install, so a fallback read faults.
- Rebuilds follow v1's path: up to the rebuild count, then `DECLINED_DAMAGE`.

---

## 6. Which form serves a partition

**Before Auto, forced arms decide.** Every stage is priced first under forced arms:

- `supdb-zstride` and `supdb-nocachezstride`: `ZPages::Always` + `ZStride::Forced` (S0);
- `supdb-zruns` and `supdb-nocachezruns`: `Always` + `Runs`.

Under `Always`, every partition the writer can hold gets the dense file, and the hot-raw policy does not run. These arms say where each encoding wins, and by how much. None is a default.

**Auto is the last stage.** The choice is made per partition inside the hot look, where the hot-raw policy's knapsack already runs: one move a look, the best saving per byte first. Its inputs are all measured.

| input | for each partition and each candidate form (raw, block, dense) | source |
|---|---|---|
| page-cache bytes a read needs resident | raw: hash region + records (`Seg::foot`, `db.rs:11481`); block: file − resident; dense: `cache_bytes()`, V alone (D13) | the files; for unbuilt forms, the sampler's estimate held within a tenth of the build ("an estimate from the first call ..." rule). The block sampler declines any partition with a block (`zfile.rs:8027-8030`), so the dense estimate is the runs walk's own exact V and M sizes, from a dry walk that writes nothing |
| resident (anonymous) bytes | raw: owned heads where held (`heads_owned`, `db.rs:5556-5570`); block: fence + filter (`zfile.rs:5034`); dense: M + `Ladder` | the attach |
| populations a read touches | raw 2 (`model_raw`); block 1; dense 1 plus the straddle share | the format |
| CPU a resident read costs | `t_hit`, `t_z`, `t_dense` | a census word per form, priced on the form's own path over at least 256 distinct pages on the thread's CPU clock (CLAUDE.md, "A decoder timed on one input"), with `ZInject` seams for tests; the v1 critique's #7 says a window seldom serves one form |
| a fault's price | `sample_t_f_ns`, `PRIOR_T_F` = 41.5 µs until measured (`zselect.rs:342`) | the sampler |
| heat | reads a second, decayed, one unit across forms; heat bands within a partition | the look's heat (`hot_read`, `db.rs:11421`); v1 critique #8 |
| a move's cost | device bytes over the measured device rate, plus the build's CPU (`timed_build`, `zfile.rs:7655`) | the builds' own `wall_us`, `cpu_us` and bytes |
| room | the group's measured room (`zpages_hot_raw_slack`, `db.rs:5380`), read at every look and never carried from a reading at a mix's start | `memory.stat` |

**The model.** Che's approximation over N populations, the generalisation of `model_raw`:

- `T` solves `Σ_j n_j (1 − e^(−λ_j p_j T / n_j)) = C − Σ_i h_i`.
- Population j misses at `m_j = e^(−λ_j p_j T / n_j)`.
- The cost a second is `Σ_j λ_j (t_hit,j + p_j m_j t_f)`.

**The population is a heat band of a partition, not a partition.** The unscrambled Zipfian puts 78.5% of reads in the first 345k keys (`che.py`), and a uniform model of that partition overstates its misses. Where the census has measured a partition's miss rate under its present form, the measurement stands in for the model, which predicts only the forms not serving.

**The decision.**

- One move a look. A move is taken when its saving rate over the partition's measured dwell between rewrites pays its cost (`ski_rental_price`, `zselect.rs:491-497`).
- A partition moved back once is latched.
- The function is pure: numbers only, held by `the_selector_names_no_clock_thread_file_or_engine_type` (`zselect.rs:3692`).
- Its replay feeds back the room and heat its own moves change (v1 critique #10; CLAUDE.md, "A controller tested on constants its own answer moves").

**Prerequisites from the v1 critique, all before Auto:**

1. a conversion slot known to every lifecycle site (#1);
2. the fill and the dense moves in one look (#2);
3. C's race against its passes (#3);
4. gated on a quiet store (#4);
5. room charged at `cache_bytes()` for a dense file, which is V alone. This is D13's accessor, in place from S5, and it is what #5 asked for once M is heap;
6. `damaged` first in every arm (#6, which v2 does);
7. the census by form (#7);
8. heat in one unit (#8);
9. E1-E3 counted (#9);
10. `z_footprint` (`db.rs:48561-48603`): it divides a covered partition's keys by its file's pages, which for a dense file are value pages. It must count a dense file's partition by its values' pages, or by its raw footprint;
11. the forms sized by their own estimate, not the block sampler's. Today a store under `Forced` beside `Auto` builds v1 files the model priced as block files (`db.rs:47483-47494` reads them in the hot look). `ZStride::Auto` replaces that pairing, and S12's model takes each dense candidate's V and M from the dry walk.

**What the model predicts** (*est.*, to be read off the forced pairings first):

- **Where every candidate fits:** dense, which decodes nothing.
- **Where none fits whole:** dense for the hot head, block for the cold tail.
- **At 10M under 1 GiB, the room is about 0.88 GB at E's start** (limit less anonymous memory less the other tenants, by "bytes" §1.2's arithmetic on `memory.stat`), and less as the mixes grow the process. The dense form is 1.0 GB:
  - **uniform reads** miss on about 12% (`model_z`), roughly 5 µs a read at `PRIOR_T_F`: block wins.
  - **The Zipfian** misses on about 1.1% (Che), roughly 0.46 µs a read on top of a dense hit, against the block form's partial decode.
  - Which wins the Zipfian is §10 Q10.

---

## 7. Workloads and predictions

### 7.1 Existing cells: none may be traded, and none can be

- **The default (`ZStride::Off`) reaches no new path.**
  - The only new code on its path is one more magic compare at an attach, and the read dispatch's tag compare, which replaces today's `self.stride()` compare one for one.
  - This is held by callgrind under `supdb-nocachezpages` (`ZPages::Always`) at 100k, where reads go through block files. In FITS no form is read, so a count there is trivially equal (v1 critique #13).
- **`supdb-zruns` against `supdb-zstride` builds identical files on every existing row**, since every partition is v1-shaped and the runs writer then writes v1 (§4.2).
  - The count that holds the premise is `runs_files = 0` on every existing pass, asserted by a refusal stage in `zpages_refusal` (`bench/src/run.rs:2262`). A non-zero count refutes the premise, and the pairing is not read.
- **The builds are not identical, so they are priced.** The runs writer reaches v1's trailer by its own walk: v1's codec rule, the record leg through the checking blob, and the facts kept as v1's running state while every rank is v1-shaped (§4.2).
  - The control reads, by arm, the builds' `cpu_us`, `wall_us`, major and minor faults (`timed_build`, `zfile.rs:7655`), and the load's cells. Only once those read level is "no cell traded" a fact; before that it is a hypothesis.
  - A difference there is the writer's to remove, for instance by calling `write_stride` itself where v1's precheck passes, before any v2 cell is read.

| cell family | default | `zruns` against `zstride` | why |
|---|---|---|---|
| load, load-shuffled | no change | no change, held by the build control above | the same files; the runs walk pushes about 5 instructions a rank (*est.*) into a build that ends in v1's trailer, with no facts recorded while every rank is v1-shaped |
| read and scan (1t and 3t), scan-mixed | no change | no change | the same files |
| ycsb A-F, ycsbs A-F | no change | no change | merges of puts drop flags, and inserts are contiguous appends by ordered ingest, so partitions stay v1-shaped |
| lag sweep, recover | no change | no change | the same files, and v1's attach |
| quick row, smallest rung | no change | no change | below the floor no form is built |

**The question for today's cells is v1's, not v2's.** S0 measures it (§10 Q1), with the walk's ask on and off:

- whether `supdb-nocachezstride` beats `supdb-nocachezpages` on C and E at 10M under 1.25 GiB;
- whether either beats `supdb`, the shipping Auto.

### 7.2 New workloads, each on a store of its own after `recover`

**Conditions** (scout 3 §4):

- **Each runs under a `Passes::shapes` flag**, off in `Passes::all()` and in `paired()`, and named with `--passes` while its stage is being priced.
  - `all()` is the row's pass set and `paired()` is `all()` less `recover` (`bench/src/run.rs:170-193`). A flag on in `all()` would put every new workload in every row from its first commit, which is the roster cost §7.3 prices.
  - The flag joins `all()` only when the stage it prices ships, with the time and bytes it adds stated in that pull request.
- no existing seed, constant, payload or comparator open moves;
- each new quantity is registered in `gate::higher_is_better`;
- each workload's first row is banked on its controls.

**Which arms each workload runs.** `zpages_refusal` refuses a forced arm at `Loaded` where `form_files + small != partitions` (`bench/src/run.rs:2289-2295`). The block body declines every partition with a block (`zfile.rs:6692-6694`). So a forced block arm, or v1 falling back to it, is refused wherever every partition has a block run. The arms are named per workload:

| workload | default (`supdb`, Auto) | `nocachezpages`, `zstride` | `zruns`, `zrunsfor` | LMDB, RocksDB |
|---|---|---|---|---|
| `sparse` (all points) | yes | yes: v1 declines `Holes` and the block body serves | yes | yes |
| `sparse` with `stray` (v2e) | yes | yes | yes, and `zrunslisted` | yes |
| `varlen` GPD, and `gpd131072` | yes (reads raw: its sampler declines `BlockBacked`) | **no**: every partition has a block run, so the pass is refused at `Loaded` | yes | yes |
| `varlen` `uniform:50:150` | yes | yes: all inline; v1 declines `Width` and the block body serves | yes | yes |
| `multi`, `multi:gpd` | yes | **no**: a quarter of the runs pass 256 B, so every partition has a block run | yes | yes |

- **The default arm's declines are read from its counters**, not from a refusal: `zpage_declined_block_backed` (`bench/src/engines.rs:11246-11247`) and the stride decline counts S0 surfaces.
- **The forced arms' refusal is the measurement of the cliff.** Its text names the partitions declined. Q2's price on `varlen` is the default arm against `zruns`, since no block arm can run there.

**Length laws are pure functions of the key's number**, so the readers' byte checks stay exact.

**`sparse` (S1, for v2a and v2c).**

- **Keys:** 16 digits, loaded in order. Number x is absent where `fnv64(x ^ salt_h) mod 10^6 < 10^6·p_hole`. A clustered variant makes x absent in aligned ranges of m numbers.
- **Widths:** 100 B, except 50 B where `fnv64(x ^ salt_w) mod 10^6 < 10^6·p_width`.
- **Points:**
  - `p_hole ∈ {10^-5, 10^-4, 10^-2, 10^-1}` with m = 1;
  - `10^-1` with m = 1000;
  - `p_width ∈ {10^-4, 10^-2}`.

  These lie on both sides of v1's cliff (one event a partition at 10M is p ≈ 2.6·10^-6) and of the runs-or-bitmap crossover, which by bytes is `16·p·(1−p)·448 = 64`, p ≈ 0.9%.
- **Phases:** `load-sparse`, `read-sparse` (uniform over the number range, misses checked as 0 bytes), `scan-sparse`, `ycsb-C-sparse`.
- **Comparators:** native. They never write the absent keys. There is no merge asymmetry, since the holes are made at load time.
- **The variant `deletes`: holes made the engine's way.** Every number is loaded, then the numbers the law marks absent are deleted in batches of 1000. Then the fold described under `multi` below, then the same phases.
  - This makes holes the way the engine actually makes them: a merge drops a key with nothing live (`db.rs:16506-16513`), and a promoted piece's piece merge can leave a 0-value record (`db.rs:16740-16752`).
  - The comparators delete natively. RocksDB's tombstones are folded by its compaction in the fold step.
  - The variant prices whether the engine's merges leave the runs writer the same tables as the load-time holes. Counts: `runs_key_segs` against the load-time variant's, and the 0-value ranks counted by the walk (`runs_zero_ranks`).
- **The variant `stray` (v2e).** Beside every 100,000th number x, one key of another length: x's 16 digits with `"!"` appended. That is about 3.5 strays a partition of 345k at 10M, and 100 in all.
  - The stray sorts directly after x's own key and before x + 1's, so it splits a run. Both comparators write it.
  - Counts: `stride_declines[NotShaped]` under `zruns` without `zpages_runs_listed`, and `runs_listed_keys` with it.

**`varlen` (S1, for v2b).**

- Scout 3's S1: GPD (k = 0.2615, σ = 73.85, mean 100), clamped to [1, 4096], compressibility 0.5.
- The `ab`-only variant `uniform:50:150` separates offset coding from the 256 B inline threshold.
- **The `ab`-only variant `gpd131072`:** the same law clamped to [1, 131072].
  - It is the only shape that reaches the field-width rule (slots of 2^16 B or more, a U segment each), and Q13's churn of segments on K. The law clamped at 4096 never reaches either.
  - The share of values past 2^16 is `(1 + 0.2615·65536/73.85)^(−1/0.2615)` = 8.8·10^-10, so at 10M an expected 0.009: none at all. (Past 4096 it is 2.8·10^-5, about 280 values at 10M, which the clamp at 4096 folds to 4096.) So the variant also plants one value of 2^16 + 1 bytes at every 100,000th number, by the law's own function of the number.
  - LMDB puts those values in overflow pages, RocksDB in blob-sized blocks.
- **Phases:** `load-varlen`, `read-varlen`, `scan-varlen`, `ycsb-C-varlen`.
- **Comparators:** unchanged. LMDB moves values over about 2 KB to overflow pages (scout 3).

**`multi` (S8, for v2b FIX/RUN).**

- Scout 3's S3: contiguous keys `[0, size/2)` with one value each.
- Then `append-multi`: `size/2` uniform appends in batches of 1000, through a new `Engine::append_batch`, whose trait default is an error and never `write_batch`.
- Counts follow 1+Poisson(1). Each value carries an 8-byte big-endian sequence stamp, so values are distinct within a key and LMDB's sorted order is insertion order.
- **The variant `multi:gpd`:** the appended values' lengths follow the GPD of `varlen`, so the runs are RUN-M with two FOR columns. That is §2.9's last row, Q4's two-column path, and the only store that reaches it. LMDB takes it as `DUPSORT` without `DUPFIXED`.
- **Then the passes, in two shapes.** `sync` and `settle` alone leave pieces standing below the trigger: `settle` waits for what is in flight and starts nothing new (`db.rs:35734-35738`). So:
  1. `read-multi-unfolded`, `scan-multi-unfolded` and `count-multi-unfolded`: the store as the appends leave it, pieces and all. This is the realistic shape of an append workload.
  2. **The fold**, on every arm and timed on its own line, never in a cell:
     - supdb: a new `#[doc(hidden)]` `Db::merge_pieces`, for an experiment as `settle` is (`db.rs:35734-35737`), which plans a merge over every range a piece stands over, whatever the trigger, and joins it. Then `Engine::fold`'s existing wait for form files (`bench/src/engines.rs:11475-11497`), run here for every arm and not only under `SUPDB_MIX_FOLD`;
     - RocksDB: a full `compact_range`, which folds the merge operands;
     - LMDB: nothing.
  3. **A refusal stage `Folded` in `zpages_refusal`.** For a forced or runs arm: pieces standing = 0 (`Db::pieces_ranked`, `db.rs:23492-23502`), `form_files + small = partitions`, `runs_files > 0`, and under `zrunsfor` `runs_value_segs` with a FOR code > 0. The stage then prints the share of the read pass's values served by a form. A store that fails it is not the store the pass is for, and the pass is refused.
  4. `read-multi`, `scan-multi` and `count-multi`: the folded store.
- **`count-multi`** times a key's count of values. It is the pass that holds D6 and §3.2's claim that a count touches no value page, by `runs_page_checks` staying 0 across it.
  - LMDB answers with `mdb_cursor_count`. RocksDB reads the merged value and counts its stamps. This needs `Engine::count`, whose trait default is an error, as `append_batch`'s is.
- **Comparators:** LMDB `DUPSORT|DUPFIXED` (values ≤ 511 B) and RocksDB with a merge operator.
- **This is the workload that prices the user's "append to the same key"**: the appends land in pieces, a merge folds them, and the partition's new file holds each key's run. The unfolded passes price the time before the merge, the folded ones the form.

**Predictions** (each a hypothesis, with the count that refutes it):

1. **sparse, under `zstride`, p_hole ≥ 10^-5:** nearly every partition declines v1 as `Holes` at the precheck, and takes the block form.
   - The chance a partition of 383k numbers has no hole is `(1−p)^383k`: 2% at 10^-5 and 0 above.
   - Count: `stride_declines[Holes]` ≈ partitions.
2. **sparse, under `zruns`, 10^-4:** every partition holds a runs file of sub-kilobyte tables and the clean store's value pages. C at 10M under 1.25 GiB reads within the pairing's resolution of `zstride` on the clean store, with equal faults a read.
   - Counts: `runs_files` = partitions; `runs_key_segs` ≈ holes + partitions.
   - Refuted if C trails by more than the pairing resolves with equal faults. Then callgrind names the extra instructions.
3. **sparse 10%, isolated holes:**
   - v2a's KS is 552 KB a partition and its lookup about 2-3 dependent L2/L3 lines (D19): *est.* +50-100 ns in core.
   - Under the bound the faults are equal, but a dense hit read is near a microsecond, so that is 5-20% of a read, not "a few percent of a fault".
   - v2c takes it to one presence line.
   - Counts: `runs_key_segs` by kind, and callgrind over the read.
4. **varlen GPD:** every partition has block runs.
   - The block form and v1 decline every partition (`BlockBacked`), so the default reads raw: about 1.4 GB of records, hash and blocks at 10M (*est.*, "lines" §7.2).
   - `zruns` with `zpages_runs_for` off builds U-only tables of about 32 B a rank (320 MB at 10M), which the budget subtracts.
   - With it on: one F segment a partition, K = 32, 20 MB.
   - Counts: `runs_lines[K]`, M bytes, `lagmem`.
5. **varlen uniform 50-150:** everything is inline, so the block form serves at about 0.72 of the raw bytes, and v2b's form is 1.0 GB + 20 MB.
   - Under 1.25 GiB at the load's end both fit, and v2b reads with no decode.
   - Under 1 GiB v2b does not fit and the block form does: Auto's regime (§6).
6. **multi:** 26.4% of runs pass 256 B (P(c ≥ 3) = 1 − 2/e), so the block form declines every partition.
   - v2a builds U segments at every change of count: nv ≈ 0.69n, since two neighbours' counts agree with probability Σp_k² = e^-2·I₀(2) ≈ 0.31.
   - v2b builds FIX(100) with Cum FOR at K = 64: 5 MB at 5M keys.
   - Counts: `runs_value_segs` by code, `zstride_declines`, read only after the `Folded` stage has passed.
7. **multi:gpd:** RUN-M with two FOR columns at K = 16 in every block (`forfit.py`), 1.4 MB a partition. Count: `runs_lines[16]` against the lines of the other K.
8. **count-multi:** `runs_page_checks` is 0 across the pass under `zruns`. A count that moved it would refute D6.
9. **sparse:deletes:** the same key segments as the load-time variant at equal `p_hole`, within the keys a merge has not yet dropped. Any 0-value ranks are counted (`runs_zero_ranks`) and served as count 0.

### 7.3 What the new workloads cost the roster

**The cost is measured, not assumed.** Before any 10M run, each workload runs once at 1M under every arm of the table above. Its pass times, its stores' bytes (`du` of the run's own `TMPDIR`) and `df` of the temp volume are recorded. The 10M figures are those times ten, as an estimate, and the first 10M run replaces them.

**What is known now (arithmetic).** At 10M a store is about:

- supdb: 2.5 GB, raw 1.45 GB plus a form of 0.7-1.0 GB;
- LMDB: 1.3 GB;
- RocksDB: 1.1 GB.

`bench ab` keeps each arm's store under `$TMPDIR/supdb-ab-<pid>` and removes it only at its exit (CLAUDE.md, "Running the checks").

**So the rules are:**

- one workload at a time;
- a `TMPDIR` of the run's own, removed by name after a kill;
- the test stores and the checked examples deleted first.

**Into the row.** A workload joins `Passes::all()` only when its stage ships (§7.2), and its first row's added time is stated in that pull request.

---

## 8. Tests

**Two rules from CLAUDE.md govern every test here:**

- **Each check is held to firing** by an input that passes the checksum ("A check spelled twice", "A guard no input can reach").
- **Each mutant named below is a deletion or a flip** that the named test must fail.

The mechanical operator-flip pass runs over every new module beside the list ("A test that reads what it tests").

### Format units (`src/db/zpages.rs`)

**T1. `runs_layout`'s sizes,** against the writer's files in T12.

**T2. The trailer.**
- It round-trips.
- A byte flipped at every offset gives the named error.
- Each decoder refuses the other two magics: the old engine's view, held as a test.
- An unknown feature bit, with the CRC recomputed, is refused as `Reject::Identity` and not as Damage, and `rejected_damage` does not move. Mutant: the features check moved under Damage.
- Forged with its CRC recomputed, each of these must be refused:
  - version 2;
  - `P + D ≠ L`;
  - nk, nv or n of 0;
  - `n > u32::MAX`;
  - `VP ≠ ⌈values_len/4096⌉`;
  - `m_off` or `m_len` off the layout;
  - `bind32` changed;
  - a non-zero reserved byte.
- Mutant: delete any one check.

**T3. KS, VS, LX and LK invariants,** each held by an M forged with `m_crc` recomputed. The invariants are those listed in §2.7 step 3. They include:

- a `first_num` tie in the wrong order (a number segment then a listed one);
- a listed key that renders;
- a listed segment's `first_num` off its key's `numbers_below`;
- two segments whose boundary keys are out of order.

Mutant: delete any one check.

**T4. `field(p, b)`.**
- At every `p` and `b` that fits, both payload sizes, against a bit-by-bit oracle.
- Explicitly the case at K = 32, b = 13, i = 31 (bit 403 of 448), and the last field of a full 384-bit payload.
- Mutant: the clamp removed, which runs under Miri or a guard page past a line-aligned buffer and must fault.

**T5. The FL line codec.**
- Round trip over random non-decreasing sequences with steps below 2^16, at every K, one column and two, with and without a mask.
- The slope equals a brute-force least-range search, and b is the least.
- The capacity holds, and K = 8 fits the worst case of D8 (steps of `2^16 − 1`, both columns, mask).
- Mutants:
  - the slope rounded the other way;
  - `K+1 → K` positions;
  - the mask placed before the residuals;
  - the second column's offset off by a field.

**T6. The presence line.**
- Rank and select against a naive popcount, at every position of segments at d ∈ {1, 0.9, 0.5, 0.01}, and bits 0, 63, 64, 383, 384 and 447.
- Both bodies of each, the portable one and the x86-64 `popcnt,bmi2` one where the CPU has it, against the same oracle. The portable body also runs on the arm64 CI legs (`.github/workflows/ci.yml:51`).
- Mutants:
  - `sub` read for the wrong pair;
  - a mask of `(2 << b) − 1`;
  - the divisor 512 for 448.

**T7. `numbers_below` and the runs' lower bound** against a binary search under `key_cmp` over the rendered keys.
- Random codecs and tables, with listed segments among them.
- Shaped and unshaped keys; keys in holes, below, above and at the prefix alone; keys one byte shorter and one longer; listed keys and the keys either side of each.
- The tie: a listed segment whose `first_num` equals the next number segment's, searched with that number's rendering and with each listed key.
- Mutants:
  - v1's u128 saturation (`zpages.rs:4613-4616`);
  - `≤` for `<` in the segment search;
  - the step back past a number segment at `first_num = y` removed.

**T8. The `Ladder`** against `partition_point`, for tables from 1 to 70,000 entries. Mutant: a level count off by one.

### The builder (units over `SegImages` fixtures, as the stride tests are)

**T9. Every state builds, serves every key, value and count, answers every number in every gap as absent, and agrees with raw.** The states:

- (a) one hole mid-partition;
- (b) holes at ranks 1 and n−2;
- (c) gaps of 1000;
- (d) 10% random holes;
- (e) a two-value FIXED run;
- (f) a three-value prefixed run;
- (g) an empty value (count 1, length 0) **in the same F segment as** (h);
- (h) a delete-only record (count 0, length 0). This fixes and holds the F1 error. Mutant: the count taken from the slot length instead of `Cum`;
- (i) a C_TOMB record with values;
- (j) in `tests/zpages.rs`: a store with `inline_bytes` 64 and values of 100 and 300 B, so runs lie in blocks;
- (k) a forged record of two extents, normalised to the concatenation;
- (l) a key of another length, giving `NotShaped`/length, then the block body;
- (m) a non-digit byte mid-partition under decimal, giving `NotShaped`/field;
- (n) n = 1;
- (p) slots of 5,000 and 70,000 B, the second its own U segment (field-width rule), with damage on a middle page caught;
- (q) a direct run's segment with markers in its records;
- (r) RUN-M with FIXED and prefixed ranks interleaved;
- (s) all-empty multi-value ranks, which are prefixed runs (not FIX(0));
- (t) a rank with 2^16 values of one byte: the count half of the field-width rule, its own U segment, and K = 8 still fitting its neighbours. Mutant: the rule's count test removed, which overflows the slope's u16 and fails the read-back;
- (u) `TooLarge`, through a `#[cfg(test)]` seam that lowers the limits (n, VP, nl, the M fields) to small numbers, each reached by a fixture over it. Mutant: each limit's test deleted;
- (v) a foreign prefixed run whose values share one width, re-encoded FIXED: its slot equals `encode_run`'s, and DER(w) holds across it. Mutant: the slot copied as it lies, which the read-back refuses;
- (w) v2e listed keys, under `zpages_runs_listed`:
  - the empty key at rank 0;
  - a sentinel of `0xff` bytes at rank n−1;
  - an interior key of another length;
  - two adjacent interior strays, which make one listed segment;
  - every answer against raw, including the keys either side of each listed key;
  - without the option, the same fixtures decline `NotShaped` with the right sub-cause;
- (x) the 252-key big-endian run `"05"`…`"10"` (D = 2), and a big-endian run whose ends are digit strings while an interior key is not. Both build v1 files under v1's codec (T12).

**T10. The checking blob.**
- A store with block runs has one block byte flipped on disk after its landing. The runs build must decline `Unreadable`, leave no tmp file, and the raw read of the key must answer the checksum error.
- **The flip comes before any read through the serving blob, or the store is reopened after the flip.** The blob remembers the chunks it has verified (`is_verified`, `blob.rs:1455-1462`). A flip after the serving blob had read the block would leave the raw read answering bytes, and the test would hold nothing.
- Mutant: `verify_checksums: false` on the runs path. The build then succeeds and serves the flipped bytes, and the test fails.
- This holds D11.

**T11. The read-back fires.** The seam damages, after the write:
- a value byte, with the row word recomputed;
- a run's `first_num`, with `m_crc` recomputed;
- an FL residual;
- a `before` word;
- the trailer's `values_len`;
- a seam that perturbs the number `Ladder`.

Each build must refuse with nothing renamed. Mutant: deleting each leg passes its case.

**T12. v1 dominance.**
- (a) For every v1-shaped fixture, the runs writer's output is **byte-identical** to `write_stride`'s.
- (b) The codec cases of T9(x) are byte-identical too. Mutant: `detect` skipped in favour of the decimal-first walk, which declines the first case `NotShaped`/field.
- (c) For a C_TOMB fixture and a block-run fixture, which v1 declines only for the flag or the block, the runs writer writes a v1 file. The file passes `check_stride` with the runs walk's record leg, and v1's reader attaches it (Check) and serves raw's answers. Mutant: `stride_record` passed as the record leg, which reads every such file back as `SelfCheck`.
- `Built::coding` is `Stride` there and `Runs` for T9's other states.

**T13. The pipeline.** The shared page fill and page check produce v1's pages and row for v1's inputs. This guards against one definition drifting.

### Integration (`tests/zpages.rs`, `tests/fuzz.rs`, `tests/leak.rs`, `tests/mapped.rs`)

**T14. The stride oracle under `Runs` + `Always`** (`tests/zpages.rs:14441`).
- Cache off and on, through the writer and through handles.
- The partitions holding second values, deletes and puts are **built**.
- Asserted:
  - `runs_files > 0`;
  - `stride_files > 0` for the untouched partitions;
  - the reasons `Holes`, `NotOneValue`, `Width`, `Tomb` and `BlockBacked` are 0;
  - every answer equals raw's;
  - every segment kind, code and mode is reached by count;
  - fallbacks and damaged files count as failures.
- The same store with block runs under `Forced`: the stride declines are those of today, `BlockBacked` by the block path. The order of §4.1 is held, and the oracle's comment (`tests/zpages.rs:14650-14655`) stays true.

**T15. Cursor parity:**
- `at`, `at_wanting`, `unplaced` plus `skip_to` both ways, `seek`, and `walk_until` (shaped, unshaped, in a hole, past the end, into and out of a listed segment);
- unsealed keys dense in the window ("A walk interrupted by another source");
- bitmap steps across lines;
- 0-value and multi-value ranks inside every scan window, against limits that end on, before and after each. The raw walk counts a 0-value rank in `seen` and emits nothing (`blob.rs:2339`), and the oracle's own comment says "a deleted key counts against a limit" (`tests/zpages.rs:14406`);
- `owner_span_from` over a sorted batch of keys across many runs, with the hint carried and with `usize::MAX`, giving the same spans. A counter of the hint's hits shows the hint taken. Mutant: the hint ignored, which this counter catches and no answer does.

**T16. The segmenter's bytes** are within one entry per close of an exhaustive search over U/F splits, codes and K, for inputs up to 200 ranks.

- It includes a stretch with one outlier block in its middle, where the exhaustive answer lowers K for the outlier and raises it after.
- Mutant: the DP's "open at any K" move limited to K no larger than the closing segment's, the greedy's flaw. It fails the bound on that input.

**T17. The chunk rule's bytes** are within one entry per change of kind of an exhaustive split.

**T18. No raw touches** over points, counts, scans, seeks, cuts and block-cache builds. The sum is `Reader::raw_touches`, which at HEAD includes the door reads a cursor makes over a stride file (`ZFile::raw_reads`, `3baea42`), extended to runs files (§1.8). The test confirms that the counter fires on a forced raw read, and on a cursor read forced through the door of a runs file.

**T19. Damage.**
- A flipped byte at every byte of V (every page of a multi-page slot), of M and of the trailer.
- Each read answers raw's value, or the attach refuses.
- After the first failure a second page answers raw (`damaged` first).
- Rebuilds stop at the cap.

**T20. Forged under good CRCs,** every CRC recomputed, each case asserted refused or served per §5's table:
- a sampled entry's `first_num`: refused (Identity);
- an unsampled run's `first_num`, moved inside its gaps: **asserted served**. The run's keys answer absent and the forged numbers answer its values;
- an unsampled KS `first_rank` shifted by one with `first_num` to match, across a gap of more than one number: **asserted served**;
- a single VS `off0`: refused by the structural pass;
- a listed key changed to a rendering: refused;
- a bit moved across lines: refused by popcount;
- a bit moved within a line under a sampled rank: refused;
- the same bit unsampled: **asserted served**;
- an FL position moved, monotonicity and continuity kept, unsampled: **asserted served**;
- an FL position moved past the next line's first: refused by continuity;
- a `Cum` total off by one: refused;
- another segment's file: refused;
- a reused id with other content: refused.

**T21. Crash windows:**
- a tmp file alone;
- a torn trailer;
- a torn M;
- a zeroed page;
- a runs file under a name where a v1 file was.

Each is reopened under `Off` (refused, rebuilt as block), `Forced` (refused, rebuilt as v1 or block) and `Runs`.

**T22. Checkpoint.** A runs file is linked into a checkpoint and the live store is rebuilt as block. The checkpoint attaches its own runs file, and neither file's bytes changed.

**T23. `tests/leak.rs` and `tests/mapped.rs`.** The heap copy of M and its `Ladder` are freed by the segment's drop, and by `RetiredZ` when a page check takes the file out of its cell, which the test drives with no sweep. Each mechanism is held by a mutant of itself ("A second mechanism that masks the first"). The census's `zf_heap` equals the sum of the files' `resident_bytes()` while they stand and 0 after, and is in `census_cap`.

**T24. The fuzz with `ZFUZZ_RUNS=1`,** a flag, so no seed's draws move.
- Keys of a fixed width over a numbered pool; deletes, appends, empty values, values of 300-3000 B.
- A run in which no partition reaches each kind, code and mode fails.

**T25. The counters** `runs_key_segs`, `runs_value_segs`, `runs_lines` and `runs_m_bytes` equal the sums over the served files, so the bench reads the mechanism and not a proxy.

**T26. The format table.** The magics triple in `store_format_names_the_files_magics`.

**T28. The refusals at open.** `ZStride::Runs` is refused beside each of `zpages_recode`, `zpages_recode_auto` and `zpages_reparse`, and beside `ZPages::Auto`. `ZStride::Forced` beside `ZPages::Auto` still opens. Mutant: each refusal deleted.

**T29. Promotion and trust, as v1 has them.**
- A promotion keeps the runs file of the partition it closes. That is v1's `a_promotion_keeps_the_stride_file_of_the_partition_it_closes` (`tests/zpages.rs:14891`, commit `dcccd4a`), on a holey store.
- A runs file built here is trusted, and one found on disk is checked page by page (v1's test at `tests/zpages.rs:14834`).
- A runs file built ahead or at a merge's finish is installed trusted at its late attach, through `Shared::z_left_seals` (v1's test at `tests/zpages.rs:15149`).

**T30. The attach's per-body decision.** Under `Forced`, a v1 file attaches and a runs file under the same name is refused as Identity and rebuilt as v1 or block. Under `Runs` both attach. The `Trust` of each comes from its own sealed word.

**T31. The page-cache accessors on a file with a large M** (a U-only varlen fixture, M about a quarter of the file):
- `served_span()` is V's.
- With V resident, `pages_resident()` reads 1.0 and the builder ahead does not count the partition cold (`ahead_cold` unchanged).
- `settle_room_bytes` charges `cache_bytes()`.
- `z_join_asks`, `prefetch_all` and `swap_ahead` ask for V's span alone. After the attach and the asks, `mincore` over M's span shows none of it resident.
- The attach reads M with no fault through the mapping: the test counts the mapping's resident pages over M's span after the attach, and they are 0.

Mutants: each site back on `map.len()`.

### The selector, at Auto (S12)

**T27. `zselect`'s dense move.**
- Pure, and passing `the_selector_names_no_clock_thread_file_or_engine_type`.
- Grids over room and heat bands.
- One move a look, and the latch.
- A replay with the room and heat fed back.
- Every constant moved both ways ("A tally that missed the case it was kept for").
- Through `ZInject`:
  - scan-hot partitions stay off value coding;
  - a store in FITS builds nothing;
  - the measured room, not the budget's share, decides.

---

## 9. Implementation steps

Each step is the size of a commit. Every option is EXPERIMENT, `#[doc(hidden)]` and off. A default flips only after `sh scripts/check.sh` passes with it in place, in a worktree with its own target directory and `TMPDIR`, with `--list` read first, and after two sittings lose no cell.

**S0. Bench: v1 in the bench. No engine change.**
- **Arms:**
  - `supdb-zstride` and `supdb-nocachezstride` (`ZPages::Always` + `ZStride::Forced`);
  - `supdb-zstrideprefetch` and `supdb-nocachezstrideprefetch`, the same with `Options::zpages_scan_prefetch` on, through the Policy field `zpages_prefetch` that `supdb-nocachezpagesprefetch` already uses (`bench/src/engines.rs:1365-1370`, `:2511`).
- `Supdb::counters` reports the engine's stride counters: `stride_reads`, `stride_page_checks`, `stride_cursor_reads`, `stride_builds`, `stride_declines` and `stride_files`.
- A dense stage in `zpages_refusal`.
- **The pairing**, on C and E at 10M under 1.25 GiB, in the row's whole order, plus the quick row:
  - the four v1 arms;
  - `supdb-nocachezpages` and `supdb-nocachezpagesprefetch`, the forced block form with its ask off and on;
  - `supdb`, the shipping Auto with the hot-raw policy;
  - LMDB.

  Faults a scan and reads by form are read before any rate.
- This answers Q1 and Q17.

**S1. Bench: the `sparse` and `varlen` workloads. No change to any engine path; one experiment entry point.**
- Load-time laws, misses checked, their own stores after `recover`, and `Passes::shapes`, off in `all()` (§7.2).
- The `deletes` variant of `sparse`, and the `gpd131072` variant of `varlen`, with the fold step of §7.2: `Db::merge_pieces` and `Engine::fold` for every arm. `Db::merge_pieces` is the one addition to the engine here: a test-and-experiment entry point beside `settle`, `#[doc(hidden)]`, that no engine path calls.
- Stub-engine tests for the byte checks (the `bench/tests/recover.rs` precedent), and a test holding each generator.
- The 1M run of §7.3 first: its times, its stores' bytes and `df`, recorded before any 10M run.
- First rows banked on their controls.
- **The pairing**, by §7.2's table of arms per workload:
  - `sparse`: the default, `zstride`, `nocachezpages`, LMDB and RocksDB;
  - `varlen` GPD: the default, LMDB and RocksDB, with the forced arms' refusal at `Loaded` read as the cliff, by its text and by `zpage_declined_block_backed`.
- This answers Q2.

**S2. The probe `bench/examples/zgen.rs`. No engine change.**
- Exact bytes per encoding over the workloads' laws.
- Cold ns per lookup, cycling at least 256 distinct structures and read where the figure stops climbing:
  - KS runs at nk from 1 to 64k;
  - the presence line, with both bodies of its rank and select (D10): the portable one, and the `popcnt,bmi2` one;
  - a listed segment's search, at 1, 16 and 1,024 keys;
  - one-column and two-column FL;
  - U arithmetic;
  - the divide by 448.
- The segmenter's bytes against a DP over codes as well as K, and the K pass's time per partition.
- The chunk rule's bytes at chunks of 8 to 256 lines against an exhaustive split (Q21).
- Under the value-coding question: the dictionary's ratio and cold decode cost at dictionaries of 1, 4, 16 and 60 KiB, over the suite's payload, a drifting payload and a random one.
- This answers Q3, Q4, Q5, Q12, Q14 and Q21. Gate: §2.9's arithmetic within 5%.

**S3. Engine: the format alone, and the factoring of v1's page check.**
- In `zpages.rs`: `RUNS_MAGIC`, `RunsTrailer`, `runs_layout`, the KS, VS, FL, BM, LX and LK codecs and `field`, `Ladder`, the per-CPU rank and select of the presence line, and `numbers_below` made `pub(crate)`.
- The magics triple.
- T1-T8, T26.
- No option, since nothing writes it yet (as `ef093bc` was for v1).
- **Its own commit:** the page check and the page fill factored for both dense bodies (§1.8), and `check_stride`'s record leg made a parameter.
  - That commit changes v1's hot path, so callgrind on `stride_read_all` (`zfile.rs:5242`) runs before and after it. The probe's source is identical, and the price is quoted from the function that holds the inlined read.
  - The instruction counts must match. A difference is fixed before S4.

**S4. Engine: the builder (runs plus U).**
- `ZStride::Runs`, with its refusals: beside the re-code policies, and beside `ZPages::Auto` until S12 (T28).
- `BuildPlan::{dense, verify}` and the checking blob, with the blocks asked ahead and dropped behind by block id (D11).
- `write_runs`, with:
  - the codec rule of §4.2;
  - v1's running state while the ranks are v1-shaped, then run-length facts;
  - the shared page fill and the v1 choice;
  - `check_runs`, and `check_stride` with the runs record leg for the v1 files it writes.
- The `blocks() > 0` check moved past the runs walk under `Runs` alone (§4.1).
- `StrideNo` gains `Unreadable` and `TooLarge`; the sub-cause counts (length, field).
- Counters: `runs_builds`, `runs_v1_written`, `runs_zero_ranks`, `blocks_out_of_order`.
- T9 (U states, (t), (u), (v), (x)), T10-T13.

**S5. Engine: serving.**
- `ZBody::Runs`, and the attach dispatch on `RUNS_MAGIC`, by `DenseAttach { v1, runs }` (§1.8). An unknown feature bit is Identity.
- `RunsBody`, with M read by one `pread` onto the heap, and the `Ladder`.
- The seven `ZFile` arms, each beginning with `damaged()`, and `owner_span_from` with its hint.
- `served_span()` and `cache_bytes()`, read by every residency, prefetch, drop, swap, ask and room site (D13). `file_bytes()` kept for disk counts.
- `zf_heap` in the memory census.
- `ZFile::raw_reads` counting for every dense body.
- `Group::Runs` in the cursor; `FileCoding::Runs`.
- The block-only `debug_assert!`s become `!is_dense()`.
- Counters: `runs_reads`, `runs_page_checks`, `runs_cursor_reads`, `runs_files`, `runs_key_segs`, `runs_value_segs`, `runs_m_bytes`, `runs_hint_hits`.
- `Seg::z_stride_no` read into the counts.
- T14, T15, T18-T25, T29-T31.
- Then **one gate run** with `Runs` and `Always` flipped on together over the form-agnostic set: the oracle, the fuzz with `ZFUZZ_ZPAGES` and `ZFUZZ_RUNS`, and `tests/db.rs` under `Always` (v1 critique #14). Then off again.
- The S0 v1 arm is re-run once at S5's head, as the control for v1's residency share moving with `served_span` (D13).

**S6. Bench: price v2a.**
- Arms `supdb-zruns` and `supdb-nocachezruns`, with their counters, and the refusal stage asserting `runs_files = 0` on the existing passes.
- **The build control** (§7.1): `cpu_us`, `wall_us`, and major and minor faults of the builds by arm, `zruns` against `zstride`, on the existing passes. A difference is the writer's to remove before any v2 cell is read.
- Pair on `sparse` (all points, and the `deletes` variant) against `zstride`, `nocachezpages`, the default, LMDB and RocksDB, at 10M under 1.25 GiB and on the quick ladder.
- Callgrind on `read_only` for the clean store against `sparse` at 10^-4 (Q6), and the block path's dispatch (Q7).
- The existing rows once, as the control.

**S7. Engine: v2b, behind `Options::zpages_runs_for`.**
- FL, feature bit 0, the segmenter of §4.3 (the code pass and the K pass's DP), Cum and Off FOR, modes FIX, RUN-F, RUN-P and RUN-M, and validate-then-emit.
- T5, T9 (F states), T16.
- Off means U only, which is v2a's shape and the arm.

**S8. Bench: price v2b.**
- The `multi` and `multi:gpd` workloads, with:
  - `Engine::append_batch` and `Engine::count`, whose trait defaults are errors;
  - the LMDB and RocksDB opens of scout 3 §3;
  - the unfolded passes, the fold, and the `Folded` refusal stage (§7.2).
- Arm `supdb-zrunsfor` against `supdb-zruns` on `varlen`, `varlen:gpd131072`, `multi` and `multi:gpd`, with `lagmem` and the census's `zf_heap` beside the rates. After a flip to on, `supdb-zrunsnofor` keeps the U-only shape.

**S9. Engine and bench: v2c, behind `Options::zpages_runs_bitmap`.**
- BM, feature bit 1, the chunk rule at the chunk S2 chose.
- T6, T17.
- Price on `sparse` at 1%, 10% and 50%.

**S10. Engine and bench: v2e, behind `Options::zpages_runs_listed`.**
- LX and LK, feature bit 2, the listed segment's lookup and walk (§1.2), and the codec from ranks 1 and n−2 where the ends fail (§4.2).
- T3 (LX/LK), T7 (ties), T9(w).
- Price on `sparse` with `stray`, against `zruns` without the option (which declines `NotShaped` there) and the default.
- A listed key's own read costs more (§3.1). The pairing reads the other keys' reads as unchanged by callgrind.

**S11. Engine and bench: v2d, behind `Options::zpages_runs_coded`, only if S2's probe says the dictionary's ratio pays its cold decode.**
- A dictionary entry point in `zlz4`: `decompress_into` and `decompress_in` take one source (`zlz4.rs:742-755`). `decompress_more` (`zlz4.rs:883`) continues a decode whose output holds a prefix, so a dictionary decode built on it would copy the dictionary before every value, which is too slow. The real entry point reads matches that reach before the output from the dictionary.
- The encoder indexes the dictionary once a partition, into a frozen table.
- Feature bit 3: the dictionary region in M; `Off` over coded bytes; every value of a rank decoded before any is emitted.
- Price at 10M under 1 GiB and 1.25 GiB in the row's whole order, the scrambled mixes included. Decodes and faults by mix are read before any rate.

**S12. Auto.**
- The prerequisites of §6, 1-11, then `ZStride::Auto`, with the Che-N kernel in `zselect`, its replay, and T27. The refusal of `Runs` beside `ZPages::Auto` is lifted for `ZStride::Auto` alone.
- Arms `supdb-zauto` and, after a flip, `supdb-nozruns`.
- Priced on the whole roster in the row's order, the scrambled mixes after the mixes ("A policy priced on the mixes it was built for").

---

## 10. Open questions only a measurement can answer

Each question gives the measurement that answers it. The risks of the design are among them, phrased as the measurement that would refute it.

| # | question | the measurement that answers it |
|---|---|---|
| Q1 | **The premise.** Does v1 beat the block form on C and E at 10M under 1.25 GiB through the engine's whole read path (memtable and piece consults, the dispatch, the harness's sink), as the dense probe's 4.4x of LMDB suggested? And does it beat the shipping `supdb` (Auto) and LMDB? **Stop rule:** stop before S3 only if the better of `nocachezstride` and `nocachezstrideprefetch` loses to the better of `nocachezpages` and `nocachezpagesprefetch` on C at equal or fewer faults a read. A loss on E or the scan pass is not read until the ask-ahead arm has been. A loss to `supdb` alone, where the forced dense arm beats the forced block arm, is the hot-raw policy's win and S12's question, not a stop. | S0's pairing in the row's whole order: faults a read and a scan, and reads by form, first. Then callgrind on `stride_read_all` (`zfile.rs:5242`) |
| Q2 | **The cliff's price.** How much does `zstride` lose on `sparse` at 10^-4 (nearly every partition block-served) against `zstride` on the clean store? If it is within noise, v1's gain over block is small and so is v2a's. On `varlen`, where no block arm runs, the cliff is the default (raw) against `zruns`. | S1's pairing, with `stride_declines[Holes]` and `zpage_declined_block_backed` read first |
| Q3 | **Runs or bitmap between 0.1% and 1% holes.** The bytes are near equal there; does the runs `Ladder`'s depth cost more lines than the bitmap's one? L2/L3 latencies here (D19) may make the depth cheap. | S2's cold ns per lookup by nk, cycling at least 256 partitions' tables; then callgrind on `sparse` at 0.5% |
| Q4 | **One-column or two-column lines, and K against lines.** Is the two-column extract (+10 instructions *est.*) and the larger K worth more than the bytes? | S2, then callgrind on `multi` varlen; if instructions dominate in core, an arm that halves K for a single column |
| Q5 | **The segmenter against the optimum** on the workloads' real distributions: the code pass's greedy against a DP over codes, and the K pass's time. | S2's DP comparison. A DP over codes is built as an arm only where the bytes it saves, put through the Che model at the measured budget as faults a read times `t_f`, outweigh the build's measured extra CPU, on a measured rung. That is the model's price, not a share of M |
| Q6 | **Few-breaks v2 against v1.** Does a runs file at 10^-4 holes cost more than §3's +5-10 instructions over v1's file on the clean store? Refutes the staged approach if it costs more than the estimate or reads worse at equal faults. | callgrind on `read_only` (S6) |
| Q7 | **Does the block path's dispatch pay zero more instructions?** | callgrind under `supdb-nocachezpages` at 100k against the commit before |
| Q8 | **The attach's cost:** M's one `pread`, its CRC, the O(\|M\|) structural pass and the sample of up to 82 ranks; their time and faults at a reopen under a bound, at 29 partitions. | the recover workload's open time by form, and `ZEvents` attach timings. If it costs, sample segments only where nk, nv > 1 |
| Q9 | **M's heap against the budget.** Does a varlen store's 20 MB (or v2a's 320 MB) of M displace V's pages as `model_z(B, f = M, W = V)` predicts? | `lagmem` and `mixcensus` beside faults a read |
| Q10 | **The dense form against the block form at 10M under 1 GiB on the Zipfian.** Che predicts 1.1% misses (0.46 µs a read at `PRIOR_T_F`) for the dense form, against the block form's partial decode with no misses. | forced pairings `zruns` against `nocachezpages` and LMDB, faults and decodes a read by mix before rates |
| Q11 | **Device bytes of dense builds.** Forced dense arms write about 40% more form bytes per rewrite than the block form (1.0 against 0.72 GB); does that show in the lag sweep and in the commits' fdatasyncs ("A write that asked for no flush")? | `lagdev`, `/sys/block/vda/stat` around the builds, commits by kind (`SUPDB_MIX_KINDS=1`) |
| Q12 | **Does the value coder pay?** Its ratio is *est.* 0.61-0.62 on the suite's payload; does it beat raw values where the room forces a choice, given about 4 sequences a value cold? | S2's probe first, then S11's pairing: faults saved × `t_f` against decodes × `t_dec` by mix |
| Q13 | **Do segments churn on K?** Is a per-64-rank K (a superblock array, one more load a read) ever worth building, once the K pass can raise K again? | `runs_value_segs` and `runs_lines[K]` on `varlen` and on `varlen:gpd131072`, whose planted values of 2^16 + 1 bytes reach the field-width rule |
| Q14 | **Does Elias-Fano earn its decode?** | the count of blocks FOR forced below the K that EF's capacity would allow, from S2 over each workload. EF is built only where that count is non-zero and its bytes saved, priced as Q5's are, through the model as faults a read against EF's extra instructions a read (callgrind), come out ahead |
| Q15 | **Do listed segments (v2e) pay, and do mixed keys need more?** | `NotShaped` by sub-cause (length, field) under `zruns` on `sparse` with `stray`, then S10's pairing. Then `runs_listed_keys` against `runs_key_segs` on any workload with prefix- or length-mixed keys: where listed keys are most of a partition, the controller's comparison of the forms' resident bytes decides, not a writer's rule |
| Q16 | **The scan's page check once a page, against v1's once a value.** | callgrind on the scan pass over a runs store; an arm if it moves |
| Q17 | **Should a dense walk ask ahead under a bound by default?** (v1 critique #11) | S0's prefetch arms: faults a scan by form under 1.25 GiB, with `zpages_scan_prefetch` on and off. The runs body follows v1's answer |
| Q18 | **Heat bands in the Che model.** Does a uniform-within-partition model mispredict the head partition's misses, and do bands fix it? | the census's measured miss rate by partition against the model's prediction, printed on the trace before any verdict is read ("A population taken from the section that holds the other population") |
| Q19 | **The RUN-P validating walk's cost.** Is about 5 instructions a value (*est.*) right, and does it matter against raw's own prefixed parse? | callgrind on `read-multi` |
| Q20 | **Does the room move enough through the mixes** that a dense form chosen at the load's end must move back? | the trace's room at every look through the roster, against the moves made |
| Q21 | **The bitmap chunk.** At what chunk do the chunk rule's bytes stop falling against an exhaustive split, over each workload's law? | S2, at chunks of 8, 16, 32, 64, 128 and 256 lines; the chunk ships where the bytes stop falling |
| Q22 | **The runs writer's build against v1's,** on v1-shaped partitions: CPU, wall, and major and minor faults. | S6's build control (§7.1); a difference is removed before any v2 cell is read |
| Q23 | **Popcount and select by CPU.** What do the bitmap read and the rank-to-key select cost under each body, on this host and on an arm64 host? | S2, both bodies cycling at least 256 lines; the CI legs run the portable body's tests |
