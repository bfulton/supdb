# Check of plan/wall/notes.md (G-wall, rule R0)

Read only, against the export at `plan/tree` (682b2c09). Paths are relative
to the export's root unless they start with `plan/`. My reruns and scripts
are in `plan/wallcheck/`. I ran the scout's own scripts with `python3 -I`
into that directory; I did not edit the scout's files.

Verdict: the conclusion stands with corrections. The counts reproduce
exactly. Three things in the notes are wrong and should be fixed before
anyone builds from them: the "no stage deletes the envelope arm" finding
(section 4.3), the door-verb list that silently allows `vault::real`, and
the reason given for not putting the test in vault.rs. Two design costs are
not priced: per-function keys against the "only goes down" rule, and
`src/bytes.rs` left uncounted.

## 1. Counts rerun

All reproduce byte for byte.

- `python3 -I plan/wall/count.py plan/tree plan/wall/wall_cipher.json plan/wallcheck/wc <14 form files incl. src/wasmapi.rs and src/db/blocks.rs>`:
  table identical to `plan/wall/wc.table.tsv` (`diff` of the sorted tables
  is empty). Code totals by file from `wc.hits.tsv`: db.rs 232, zfile.rs 12,
  blob.rs 7, ordindex.rs 2. Sum 253. Confirmed.
- `python3 -I plan/wall/count.py plan/tree plan/wall/wall_form.json plan/wallcheck/wf src/db/vault.rs src/db/pager.rs src/crypt.rs src/db/dirkey.rs`:
  identical to `plan/wall/wf.table.tsv`. vault.rs code 110, tcode 270;
  pager.rs, crypt.rs, dirkey.rs code 0. Confirmed.
- `task_tokens.json` over the 13 form files: identical to
  `plan/wall/task.table.tsv`. `task_form_tokens.json` over the four
  encryption files: every row equals notes section 3.1.
- db.rs scopes with a code hit: 88 (`awk` over `wc.hits.tsv`, distinct
  column 4 where file is db.rs and region is code). Confirmed.
- The 25 + 7 = 32 of section 4.1: the scout's `awk` rerun on my
  `wc.hits.tsv` gives `25 7 32`. Confirmed.
- `\bpasses\(` over vault.rs code: 23 lines. Confirmed.
- Stage totals from `couplings.draft.tsv` (`awk` summing column 6 by side,
  stage, region): every row of the section 4 table matches, and form code
  sums to 253.
- Scope keys: for the 542 hits that are not top-level items, the nearest
  preceding `fn NAME` line in the raw source agrees with the lexer's scope in
  every code hit; the 6 disagreements are comment lines between two fns of a
  test module, keyed `<item:...>`, which is right
  (`plan/wallcheck/scope.py`). No two distinct functions share a key among
  the keyed scopes of db.rs, zfile.rs, vault.rs, blob.rs
  (`plan/wallcheck/dup.py`, no output).

## 2. What is wrong

### 2.1 "No stage deletes the envelope arm" (notes 0.3, 4.3, surprises 1 and 2)

Wrong. `seam/decisions.md` records the order, and its item 10 reads:
"S6: the log and the manifest as sealed records behind the door; the
default flips; the envelope arm goes." design.md's own test table (section
10, after S7) also says the vault unit tests "die with the envelope arm
(S6)". Only design.md section 10's S6 bullet says `supdb-cipherenv` keeps
the envelope "until it is deleted"; decisions.md supersedes it.

Corrections:
- Every `S3*` line (form 5 lines / 5 occurrences; enc 19 lines / 74; and
  the S3* halves of the `S1+S3*` and `R4+S3*` lines) falls at **S6**, not
  at an unscheduled deletion.
- The `LogCipher` / `passes` / `seal_manifest` conflict of 4.3 dissolves:
  the envelope arm and its sealed log go at the same stage. The 29 form and
  35 enc S6 occurrences fall at S6 as planned.
- The vault test lines staged `S3/S6` (25 lines, 270) are S6.

### 2.2 S4 is conditional, and the notes stage 20 occurrences on it as if it were not

decisions.md item 8: "S4 with decision 1, only if S3's load pairing says
the Log class pays." design.md section 10 S3 keeps the run temp's
durability "still carried by `log_run`". If S4 is not taken, the 10 S4
lines (20 occurrences: `Direct.logged`, `RUN_LOGGED`, `MemTable::logged`,
`Db::{append_pinned, commit_direct, log_run, log_run_again, run_attended,
run_again}`) survive S6 under the pages arm. Their stage should read
`S4 (conditional; else stays)`.

### 2.3 The door-verb list is wider than R0 and hides a cipher concept

notes 2.4 excludes 18 verbs from the counted token. R0 (forms.md section 3)
lists `create, open, map, rename, hard_link, remove_file, names, sync_dir`.
The scout's extra ten are `create_rw, open_write, open_append, open_slot,
open_dir, len, exists, is_file, read, real`. Search: `plan/wallcheck/door.json`
through `count.py` over db.rs, zfile.rs, ordindex.rs.
- 33 db.rs code sites use the extra ten (by name: len 12, real 6,
  open_write 5, exists 3, open_slot 2, create_rw 1, is_file 1, open_append 1,
  open_dir 1, read 1). zfile.rs and ordindex.rs use only R0 verbs (12 and 1).
- Nine of the ten are plain I/O verbs. `vault::real` is not: vault.rs:2091-2102
  answers "the file's path in the store's own directory, which only the log
  and the manifest have" for a store under a cipher. It is the log's
  pass-by-name surfaced in the engine. Its six sites are uncounted:
  - db.rs:4812 `Wal::chain` (test trace)
  - db.rs:28062, 28086 `Db::sync_wal` (test trace)
  - db.rs:28233 `Db::next_wal` (test trace)
  - db.rs:29101 `Db::open` (`wal_opened`)
  - db.rs:33171 `Db::wal_durable` (pub API; its comment: "for a store under
    a cipher is not the path the engine names it by")

  Correction: take `real` out of the door list. Form code rises from 253 to
  259 (db.rs 238). Stage: S6, where the log goes under the layer (or the
  logical-to-stored mapping becomes design.md's debug seam
  `vault::stored_name`, which is then the counted token). State in the notes
  that the other nine extra verbs are allowed beyond R0's list, and why.

### 2.4 The reason for not putting the test beside the vault test is wrong in part

notes 5.1, first bullet: vault's test module being Linux-only "would not
run on a non-Linux leg (`apple-silicon.yml` ...)". `.github/workflows/apple-silicon.yml`
builds only `bench machine` and runs no engine test; ci.yml's other legs
(`scripts/check.sh test`, `--target aarch64-unknown-linux-gnu --lib` at
ci.yml:111) are Linux. So no CI leg skips the vault module today. The
recommendation (`tests/wall.rs`) still holds on the other two reasons
(a test inside one side is itself a coupling; G-door replaces that test).
Drop the first reason or mark it "no leg today". Note also that the existing
test reads db.rs with `include_str!` and cuts it at a text marker
(`"\n#[cfg(test)]\nmod streaming_writes"`, vault.rs:2882), which is the
marked-region fragility notes 1.2 rejects as option B.

### 2.5 Smaller corrections

- notes 4.3 third bullet: `vault::paged` arrives at **S1** per design.md
  section 10 ("S1: `vault::File` with only `Os`. Includes
  `preallocate`/`recycle`/`paged`"), not S3. The conclusion (it is barred by
  R0, so `open_seg` falls only at R4) is unchanged.
- The `G1` stage on `OrdIndex::open` / `head_absent` (ordindex.rs:413, 515)
  is a prediction: G1 rewrites the CRC skip, but a rewrite may keep a
  `piecewise()` branch. Mark "not verified".
- Lines staged `S1+S3*`, `R4+S3*`, `S1/R4` hold occurrences that fall at
  different stages under one key, because the token `vault::<not a door
  verb>` lumps `vault::check`, `vault::hold`, `vault::persist`,
  `vault::persist_form`. Keying the token by the vault function's own name
  (`vault::check`, `vault::persist`, ...) makes every line single-staged.
- notes 1.1 calls `look_mask`, `writer_rests`, `not_asked`, `start_ahead`
  plumbing that "stays". Several of these names carry no token at all
  (`look_mask`, `writer_rests`, `begin_slow`, `end_slow`, `build_rest`,
  `abandon_asked`, `WRITER_OP`, `WriterOp`), so they are not in the ratchet
  whatever their stage; only the token-bearing lines inside them are. Say so,
  so a reader does not take the list for an allow-list.

## 3. What was missed

1. **`src/bytes.rs` is uncounted and is where the next predicate goes.**
   It is "in neither list". It holds `Own::Paged(Arc<dyn Paging>)`
   (bytes.rs:284), `MmapBytes::paged` (365), `piecewise` (490), and the
   `Own::Paged` arms of every advice method (417-609). `piecewise` and
   `ASK_PIECES` are exactly "is this store under a cipher" spelled as a
   byte-source method, and both live there. A new one (say
   `Bytes::decrypts()`) added to bytes.rs and called from a form module is
   caught only if its spelling is on the form-side list. Either count bytes.rs
   against a ratchet of its own seam vocabulary, or state the hole in
   section 6 of the notes.
2. **Method-call sites through a binding.** The scout named `p.check()` at
   db.rs:28642. Search `\.(hold|check|counts|sweep|oldest_lent|held|backed|ensure|poison|failed)\(`
   over db.rs and zfile.rs code, lines with no counted hit
   (`plan/wallcheck/meth2.py`): three `p.check()` (db.rs:19070
   `Reader::end_slow`, 28642 `Db::create`, 29364 `Db::open`) and
   `s.readers.oldest_lent()` at db.rs:10395 `PagerGrace::oldest`. The rest
   are the retired lists' `.sweep` and the selector's `.counts()`, false
   positives. All four are plumbing; none changes a stage.
3. **Methods named only by vault/pager.** I listed every `fn` defined in the
   four encryption files and not in any form file or bytes.rs (133 names),
   then every `.name(` call in form-file code on a line with no counted hit
   (`plan/wallcheck/meth.py`). All results are std names (`lock`, `unwrap`,
   `name`, `remove`, `chunks`). No uncounted call into vault or pager by
   method syntax.
4. **Imports and aliases.** `grep -nE '^\s*use '` over every form file and
   the encryption files: no form module imports vault, pager, crypt or dirkey,
   no `as` alias. `use super::*` appears in db.rs's test modules
   (db.rs:5681, 43821, 44052, 44145, 44219, 44578); a glob import brings
   private sibling modules into scope, so test code there can write
   `vault::x` or `pager::x` without `super::`. The tokens still match such a
   path, so this is covered; it is why the `use` token alone is not enough.
5. **Other encryption-side reaches.** `grep -nE '\b(super|crate)::'` over the
   four encryption files: beyond the two the notes name (vault.rs:642
   `zfile::read_budget`, vault.rs:2132 `Db::ord_name_for`), only `bytes::` and
   `crypt::`, and dirkey's `super::refused`, which is dirkey's own (dirkey.rs:95).
   Confirmed.
6. **Macros, type aliases, trait impls, raw strings.** `macro_rules!` only in
   blob.rs (779, 882, 941) and wasmapi.rs (332, 343); no hit is scoped to a
   macro. No `type` alias in db.rs or zfile.rs names vault or pager. Trait
   impls across the wall: db.rs:10388, 12818; zfile.rs:4544, all counted. No
   raw or byte-raw strings in `src/`. No `include!` or `#[path]`.
7. **cfg forms.** `grep -rhoE '#\[cfg\([^]]*test[^]]*\)\]' src`: `cfg(test)`
   50, `cfg(all(test, target_os = "linux"))` 3, `cfg(all(test, not(target_family
   = "wasm")))` 1, `cfg(not(test))` 1. lex.py's test regexes accept the first
   three shapes and not `not(test)`, which is right. A future
   `cfg(any(test, ...))` would not be read as test (the regex requires
   `all(`); a test of the Rust lexer should include it.
8. Outside `src/` (tests, bench, examples, web) is out of R0's scope as
   forms.md states it ("a source test over all of `src/`"). Not counted.

## 4. Sites sampled (read in the export with context)

| site | scope | stage in draft | verdict |
|---|---|---|---|
| db.rs:4386-4395 | `Wal::begin` | S6 | right: `vault::under` then `log_begin` |
| db.rs:4873-4888 | `Wal::replay` | S6 | right: `open_records` under the sealed branch |
| db.rs:10967-10969 | `MemTable::logged` | S4 | right, but S4 is conditional (2.2) |
| db.rs:12470-12477 | `manifest_image` | S6 | right; `v.persist` counted by `\bv\.persist\(` |
| db.rs:12480-12489 | `manifest_open` | S6 | right |
| db.rs:12780-12792 | `compact_job` | S1/R4 | right |
| db.rs:12801-12827 | `MergeRest`, `NoRest`, `impl MergeRest for &pager::Hold` | R4 | right; keyed `MergeRest::<item>`, `NoRest::<item>`, `Hold::<item>` |
| db.rs:13187-13193 | `merge_keys` | R4 | right |
| db.rs:13849-13856 | `tier_walk` | S1+S3* | right, S3* half is S6 (2.1) |
| db.rs:8955-8963 | `Seg::records_resident` | G1 | right (R3) |
| db.rs:19095-19125 | `Reader::paged_begin` | stays | right |
| db.rs:21556-21569 | `Reader::rest` | stays | right |
| db.rs:29395-29415 | `Db::append_pinned` | S4 | right; `self.vault.is_some()` counted by the field token |
| db.rs:30218-30226 | `Db::run_attended` | S4 | right |
| db.rs:32465-32476 | `Db::land_seal` | S1 | right |
| zfile.rs:4296-4320 | `build_file_seen` | R4+S3* | right, S3* half is S6 |
| ordindex.rs:403-432 | `OrdIndex::open` | G1 | plausible, not verified (2.5) |
| blob.rs (Sec::read 490) | `Sec::read` | stays | right per R2 |
| vault.rs:557-564 | `passes` | S6 | right |
| vault.rs:1489-1510 | `Vault::companion` | S3* | right, stage S6 |
| vault.rs:2125-2139 | `persist_one` | S3* | right, stage S6 |
| vault.rs:636-646 | `default_cache` | G3 | right |

22 sites; no misclassification of side or scope. The stage corrections are
those of section 2.

## 5. Attack on the feasibility conclusion

The ratchet is feasible. Two costs the notes do not price:

1. **Per-function keys fight "counts only go down".** With keys at
   (file, scope, token, region), moving one coupled line from one function to
   another, or renaming a function among the 100 db.rs scopes in the draft
   (`awk` over `couplings.txt`: db.rs 100 distinct scopes, 42 of them
   `stays`), lowers one key and creates or raises another. The CI step the
   notes propose ("fails on any increase unless the line's reason changed")
   reads every such refactor as growth needing owner approval under
   CODEOWNERS. That is a form change (a refactor of the read path, a merge, the
   open) that needs an edit of the wall's file: a direct cost against "not
   fragile to maintain". Fix: hold monotonicity on (file, token, stage)
   totals, and use the per-function keys only to (a) report where a change
   is and (b) hold the `stays` scopes as an allow-list. A move then changes
   no total and needs no approval; new plumbing in `Db::open` still raises
   that key's count and fails.
2. **The lexer.** Agreed it is the main risk. The seven traps in notes 5.4
   should be the Rust lexer's unit tests, plus `cfg(any(test, ..))` (3.7) and
   a fn inside a closure.

No runtime cost: the test reads source and adds no instruction to any path.
The G1 `pin::` rename it recommends may change inlining (a method on
`Reader` becoming a call into another module); G-cost would see it, and the
notes already mark this "not verified".

## 6. Corrected counts

| quantity | notes | corrected |
|---|---|---|
| form-side code occurrences | 253 | 259 with `vault::real` counted (db.rs 238) |
| S3* (form 5, enc 74, plus halves) | unscheduled | S6 |
| S3/S6 vault tests (270) | S3/S6 | S6 |
| S4 (20) | S4 | S4 if taken, else stays |
| S6 form | 29 | 35 with `vault::real` |
| door-verb sites in db.rs beyond R0's list | not stated | 33, of which `real` 6 |

Everything else in the notes' tables reproduces.

## 7. Not verified

- That the Rust lexer reproduces lex.py (as in the notes).
- Whether G1's rewrite of ordindex's CRC skip removes the `piecewise()` call.
- Whether an integration test binary that does not reference the crate
  links it; it affects the test group's link time only.
- That decisions.md is the planner's final word over design.md section 10's
  "until it is deleted"; the task says decisions.md records what was decided,
  and design.md's own test table agrees with it.
