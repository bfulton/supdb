# formsites: notes

Item: every site that reads a form page, for the damaged-page contract
(forms.md R8; design.md "A Regen-lazy (form) file"). Deliverable:
`sites.md`. Tool: `sites.py` (the two searches and the reconciliation),
`found.txt` (its `--list` output on the tree).

## What I read

- seam/forms.md R8 (row and §4), the cost-table line; design.md 300-335
  (Trust: the Regen-lazy paragraph), 439 and 602 (the owed list).
- plan/tree CLAUDE.md: the compact form under a cipher, the four
  "site that reads a group's bytes was missed" corollaries.
- src/db/zfile.rs: the whole of 150-3300 (ZRead/ZScan/ZSeek, Group,
  ZCursor, Opened, open_group, view, view_of, group_of_*, locate_rank,
  locate, read_all, count, scan_at, scan_in, ask_ahead, values_at,
  key_at_into, seek, owner_span(_from), cut_in, the META methods), attach
  and check_sample (1953-2140), warm_file (3778), build_file_seen and
  write_pages (4184-4740), sample/sample_of (5052-5216), the skew test
  (8480-8570).
- src/bytes.rs: Paging (295-330), MmapBytes::as_slice, slice_slow,
  slice_at, lend_at (370-495).
- src/db/pager.rs: module doc "What can go wrong" (69-77), Pager::poison and
  check (424-450), Back::open (729-758), Paged (762-), fill (965-1025),
  slow (1027-1110), ensure and piece_from (1312-1360).
- src/db/vault.rs: Kind (468), Disk::same (500), back_of (907), page_of
  (919-956), rename (1013), companion (1489-1520), map and check
  (2205-2222).
- src/db.rs: the dispatch 20585-21125, the PartReader/PWalk 38000-38262,
  z_attach 33255-33330, the controller's callers of damaged/drop_cache/
  prefetch_all/warm_file, z_cross 42410-42450, scan_rested 21652-21700,
  Entered::finish 17270-17290 and end_slow 19064-19070.
- tests/cipher.rs: tiny (313), a_changed_byte_is_refused_or_is_where_the_log_ends
  (887-940), a_changed_byte_in_a_form_file_is_refused_or_the_form_is
  (1935-2000).

## Findings as I went

1. There is one page accessor (`ZFile::page_bytes`) and two more
   `slice_at`s, both in `attach`. Everything else reads either through
   them or through a borrow already taken (`Group::Page`/`Coded`,
   `Opened::Borrowed`/`Coded`).
2. Under a cipher, a failed tag is a `None` from `slice_at` **after**
   `Pager::poison` (pager.rs:1102). Every zfile site already answers that
   `None` by `ZFile::fail` and the raw form; the store's refusal comes only
   from the latch, read at `Entered::finish` -> `end_slow` -> `Pager::check`.
   So today under a cipher no wrong answer comes from a failed form chunk:
   the poison overrides. With the poison removed for form files, no wrong
   answer either: every `None` arm is F or R (sites.md legend).
3. The `None` arms are reachable only under a cipher. A plain store meets
   damage as bytes that fail a check (`Fail::Check`), never as `None`
   (a mapping answers past-the-end only). The unit tests reach a `None`
   from `page_bytes` once, through the `skewed` seam with an empty buffer,
   and only through `count` (zfile.rs:8567). So the arms are untested;
   T4 in sites.md is owed whatever R8's shape.
4. Two places do not `fail` at the first `None` and re-ask:
   `ZFile::seek`'s coded path (3021-3022) and `ZCursor::open_at` (572).
   Both re-ask through `open_group` and fail there. Harmless (the second
   ask fails the same way: a fill failure is a function of the stored
   bytes), but under R8 they should take the `Damaged` where they meet it.
5. Three places answer a failure without `fail`, so the form stays in
   service: `Fail::Misaligned` in `open_group`/`locate`/`seek` (unreachable
   on a mapping, page-aligned under the pager too), `owner_span_from`'s
   `v.group(j)?` (3119), and `ZCursor::skip_to`/`seek` on a fence entry
   that does not decode. All go raw; none is a wrong answer.
6. Options where "did not read" and "not there / cannot say" are one value:
   `page_bytes` (out of range / did not read), `open_group` (failed /
   misaligned), `view_of`, `group_of_index`, `group_of_key` (failed read /
   failed parse), `owner_span(_from)` (did not read / out of service /
   several-record `ZNOPAGE` / unresolved group), `ZCursor::skip_to` (fell,
   any cause), `ZCursor::key` (end of partition: over bytes already read,
   so it cannot mean did-not-read while the borrow is live). The only one
   whose `None` could be read as an *absence* is `ZCursor::key`, and it is
   over held bytes. No site turns a `None` into `Absent` or `Served{n:0}`;
   `Locate::Absent` is made only from the filter, the fence, a `ZNOPAGE`
   entry, `rank >= nkeys`, or a seek over a page that read.
7. The sampler and the builder read the **segment**, not the form; the
   builder's verify is of its own stack page; the warm reads nothing under
   a cipher (`as_slice()` of a paged mapping is empty). So under a cipher
   the "builder's verify", "sampler" and "warm" items of the brief are not
   form-page reads; under a plain store the warm reads one byte a page with
   no check (SIGBUS on a device error, as every mapped read).
8. No durable job reads a form file. Merges read `seg.raw()`; form reads
   feed only answers and the block cache's forms, which are not durable.
   So a form failure cannot reach a manifest, and the merge's
   `vault::check` (db.rs:13684, 13852) keeps its meaning if form failures
   stop poisoning.
9. The form file's `Disk` copies the segment's (`companion`, vault.rs:
   1501), so the two envelopes are under one wrapped key, and a chunk's
   AAD is the envelope header, which does not bind the kind. A segment
   envelope renamed to `x-T.z` decrypts as the form file and is refused by
   the attach's trailer check. This does not change with the proposed
   change (the class comes from the `Disk.kind` the vault found, and the
   segment's reads go through `Kind::Seg`); recorded because it is a fact
   about what a tag proves for a form file: authenticity of bytes under the
   segment's key, not that the bytes are a form file.
10. `Reader::scan_rested` rests between `scan_body` calls, after the
    cursors are dropped, so a ZCursor's held borrow does not cross a rest
    there. The builders' `build_rest` was not traced.

## Design choices (for the smallest change), with the reason

- **Class on `Back`, not on the name.** The fill fails against an
  envelope; `Back` is made from a `Disk` whose `kind` the vault already
  holds (`back_of`). A class from the engine name (`is_zpg_file(label)`)
  would also work today but would be read at `Paged::adopt`, before a
  built form has its envelope, and is a second classifier of names. A fill
  with no `Back` stays store-fatal (invariant, not damage).
- **No change in zfile.rs or db.rs.** Every site already handles `None` as
  failure (finding 2). The smallest change is where the latch is set.
- **Count per file.** So T1 can assert the mechanism fired, and the
  controller (already reading `damaged()`) needs nothing new.

## Model runs (the searches)

No protocol model: this item is a scout. `sites.py` is the instrument.

1. `cd .../seam/work/owed/formsites && timeout 600 nice -n 10 python3 -I sites.py ../../../../plan/tree/src`
   Result: `functions: A 63, B 123, union 124; READS 62 (both 61, A only 0,
   B only 1); unclassified 0; READS rows unfound 0`, exit 0. The one B-only
   READS row: `ZCursor::group` (a match over the held borrow; B's root
   `Group::Page(`/`Group::Coded(`).
   Earlier runs of earlier versions (kept in this note because they shaped
   the searches): v1 with B unrestricted found 25 unclassified, among them
   six `PartReader` users and four `cache_owner_of` callers I had not
   listed (now ABOVE); v2 with B closed transitively through every db.rs
   caller found 381 unclassified (the closure reaches the commit path) and
   B was cut at layer 3; v2 also missed calls split across lines
   (`z\n.read_all(`), fixed by joining a method chain's continuation onto
   its line.
2. `... python3 -I sites.py ../../../../plan/tree/src --list > found.txt`,
   exit 0; 124 FOUND rows: READS 62, META 34, ABOVE 16, OTHER 12.
3. Mutants, each `TMPDIR=<this dir> timeout 600 nice -n 10 python3 -I sites.py ../../../../plan/tree/src --mutant N`:
   - M1, a new `ZFile` method reading `self.map.slice_at` under a new name:
     **caught**, `UNCLASSIFIED ('db/zfile.rs', 'peek_first_byte') A B`, exit 1.
   - M2, a new db.rs dispatch through `self.zc(seg)` and `z.seek`:
     **caught**, by A and B, exit 1.
   - M3, a form page read outside zfile.rs by `vault::map(zpg_name(..))`
     and `slice_at`: **caught by B only**, exit 1 (A has no accessor name
     for it: this is the case the second search exists for).
   - M4, a new ZFile method reaching the page through `self.key_at_into`:
     **caught by B only** (the call-graph closure), exit 1.
   - M5, a new caller of `cache_key_at` (a layer-3 dispatch) in db.rs:
     **not caught**, exit 0. By design: B stops at layer 3, because a
     caller of a dispatch receives answers, not failures. So the
     completeness claim is for layers 1-3 and not for ABOVE; ABOVE is
     listed for orientation only and is not claimed complete.

## Not verified

- No Rust was compiled or run: the proposed change, T1-T4 and their
  mutants are written down, not run. Whether T1's positions reach a page
  read (the `> 0` it asserts) is not known.
- Which byte positions make today's form-flip test `refused` at the open
  (rather than `unread`) was not determined; after the change T1 requires
  none, and if the open reads a form envelope's header somewhere I did not
  find, T1 will say so.
- `sites.py` is regex over source text, not a parser: macros, a call
  through a function pointer or a trait object, or a receiver bound under
  another name in db.rs (`let q = seg.z.get()` then `q.read_all` — B still
  finds the function by `.z.get()`, A would not) are its blind spots. The
  enclosing-function attribution is "the last `fn` above", which misnames
  item-level lines (one OTHER row, `cut_at`, is struct `PartReader`'s doc).
- Files searched: zfile.rs, db.rs (both searches); vault.rs, pager.rs,
  blob.rs, zpages.rs, bytes.rs, flatindex.rs, zselect.rs, block.rs,
  index.rs, ordindex.rs (names of the API and types only). wasmapi.rs,
  crypt.rs, blocks.rs, dirkey.rs, zlz4.rs, reserve.rs, format.rs were not
  searched; zlz4 is a decoder over bytes it is handed.
- tests/ were not searched for form readers (they are not sites).
- The vault's `persist_form` and its `cold` path were not read in full; I
  assumed it writes the envelope from the memory file and decrypts nothing.
- `build_rest` (the block builders' rests) and whether any PartReader or
  ZCursor lives across one was not traced (note A in sites.md).
- That `Options::default()` leaves a cipher store's form off (so the
  existing tamper test has no form files) is inferred from CLAUDE.md
  (`zpages_under_cipher` off), not read in the options code.
- The ROADMAP.md line numbers design.md cites (256-257) do not hold the
  owed item in this tree's ROADMAP; not chased.
