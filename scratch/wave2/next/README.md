# Drafts for the next lever

Nothing is drafted here at the moment. The memo-size drafts that were here
(`Options::zpages_memo_bytes` with the arms `supdb-cache64tofitautozpagesmemo16`
and `...memo32`) landed with the commit that added them to the record
(`stage4.md`, "The memo's size, counted"); the lever C drafts were
superseded by `Options::zpages_block_cursor` (`stage4.md`, "A block read
through one cursor").

The next lever is the per-record form, probed and not yet built:
`bench/examples/zrec.rs` and `stage4.md`, "The per-record form, probed
against the comparators". Under 640 MiB at five million keys it reads
point reads at 0.31-0.42x of the group form's time and scans at 0.58x, in
the same process, both forms fitting the bound and faulting nothing, so
the gap is the group decode. Building it means the form file's page as a
container of per-record-decodable groups (offset table, first key raw,
keys front-coded, the group's first record stored raw as the dictionary for
the rest -- the choice "The dictionary and the value coding, probed" made,
with a segment dictionary appended to it as the arm), the cursor's walk and
the point read decoding a record, and the group form kept as the arm; the
per-record section's last paragraph lists the parts. The suite then prices it on
E and on the controller's passes, which the probe cannot run.

What the counts pointed at before the probe, for E: the decodes themselves, 147,000
groups a pass of 105,000 scans at the shipped memo, with 42% of the memo's
asks missing -- and the three sizes paired say a larger memo does not
buy them back (`stage4.md`, "The memo's size, counted"): the memo pays
for groups that come back while still warm in the last-level cache, and a
hit on a cold group costs about what a decode would. The design's 3.7
names an admission policy for the memo as the arm if the hit rate lands
under 0.45; after the sizes, the question it would have to answer is
which groups stay warm, not which are asked for again, and the instrument
it needs first is a micro-probe of a hit's parse against a decode over
more distinct groups than the last-level cache holds, timed as the decoder
was (`bench/examples/zdecode.rs`). The scans' start groups
(`ZCursor::seek`) are where the misses are, by the profile of the session
before (`ZFile::seek` 27% of E's thread, 22% of it the scans' start seeks).

A script of exact-once replacements is the shape a draft takes here: run it
to apply it, on a committed tree, and check `git diff` after -- never
dry-run one by disarming its `edit` function, since a draft may write a
file directly (the memo-size one did, and a dry run applied half of it).

## The per-record form in the engine: the plan

Read before the build starts; it was written from the code after the two
probes and before any engine line changed.

What stays: the page (`ZPAGE`, the header, the directory of up to sixteen
groups with each group's first key, the CRC bound to the segment), the
fence, the trailer, the filter, the fill's loop (`fill_page`,
`fit_one_group`, `seal_page`, `page_decodes_to`), the attach, the controller.
A group's payload is opaque to all of it: `stored` or one LZ4 block today.

What changes: a third payload coding, per group, in the high bit of the
directory entry's decoded-length word (`D_CODED`; the length never reaches
it), since the first build found a record alone whose head passes a page --
a 5,000-byte key the block coding compressed and a head never is -- and the
fill falls back to one block for exactly that group; the header's spare
byte stays zero. The trailer's `ZVERSION` moves to 2 so a reader from
before refuses the file at its trailer and rebuilds it (a derived file),
and this reader refuses version 1 the same way. The record-coded payload of a group
of `n` images:

    [u16 off_i; n]                        payload offsets of the records
    record i:
      varint plen, varint slen, suffix    head_i = [klen u16][n u16][key],
                                          front-coded against head_0
      varint ulen_i                       the image's length (a multiple of 4)
      varint clen_i                       the tail's stored length
      tail_i (clen_i bytes)               image_i from align_up(4 + klen_i, 4)
                                          to ulen_i: LZ4 against tail_0 as the
                                          dictionary, or raw when clen_i equals
                                          the tail's length (and always for i = 0)

The key is `head[4..]`, so a seek compares keys through the front-coded
heads without decoding anything (the probe's `cmp_key`), and a decode of
record `i` writes head_0[..plen] ++ suffix, zeros to the aligned tail start,
then the tail, into an aligned buffer: `parse_record` reads it as the image.

Where it plugs in:

- `zpages.rs`: `encode_record_group(imgs, ends, out) -> Option<usize>` for
  `FillScratch::classify` under `FillScratch::coding == Record` (the stored
  class stays as it is); `decode_record_group(payload, count, buf)` behind
  `decode_group` for a coding-1 page, every record decoded end to end, so
  the verify (`page_decodes_to`, `check_group`, `walk_group`), the memo and
  every reader of a decoded group work unchanged; `record_group_seek(payload,
  count, key) -> GroupPos` over the heads, and `decode_record(payload, count,
  i, buf)` for one image. `PageHeader::coding`, `encode_page` takes it,
  `view_with` checks it. Unit tests: round trip of both codings on the
  module's shapes (the empty key, long keys, compact and extent records,
  stored tails), the seek against the whole-group seek on every key and the
  keys between, forged pages (a head whose plen passes the first record's
  length, an offset table that points outside the payload or backwards, a
  tail that decodes short), the independent decoder test extended.
- `zfile.rs`: the point read (`ZFile::locate`) on a coding-1 group seeks
  the payload and decodes the one record into the read's buffer, no memo;
  the cursor keeps decoding groups whole through the memo in the first cut
  (a per-record decode of fourteen records costs about what the block
  decode did), and decoding as it emits is the lever after, priced in the
  suite. The builder's `FillScratch` takes the coding from the options.
- `db.rs`: `Options::zpages_record_coding` (on), threaded to the builder;
  nothing on the read side, which reads the page's byte.
- `bench/src/engines.rs`: the block coding as the arm beside the shipping
  combined and forced arms (`...zpagesblock`, `supdb-nocachezpagesblock`).
- `zselect.rs`: the sampler's estimate runs the same fill, so it follows.
- Tests beyond the unit ones: `tests/zpages.rs` windows under both codings,
  `tests/fuzz.rs` toggling the coding under `ZFUZZ_OPTVARY`, the two fuzz
  ranges, `tests/leak.rs` as it is.

The segment dictionary appended to tail_0 is the arm after this lands, a
trailer slot an empty dictionary leaves as this shape.
