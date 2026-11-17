## 1. Measured costs of the current cipher design

| Cost | Figure | Source / command |
|---|---|---|
| In-core point read | 339 instructions over a plain twin of 818. Earlier it was 229 over 841. 95 of the 339 are the first ask for the whole section, refused with an error that is made and dropped (`not_lent`) | CLAUDE.md:1825-1835; `bench/examples/read_only.rs supdb-cipher 100000 1 n-none` under callgrind |
| Loads, in core | 0.84-0.92x | `pair.sh supdb-cipher supdb-cipherbase 300000`, CLAUDE.md:1817-1819 |
| Point reads, one thread | 0.92x; threaded passes unresolved | CLAUDE.md:1819 |
| Scan passes | 0.89-0.94x; threaded 0.80-0.90x. They were 0.25-0.45x before `scan_pieces` got 64-record windows | CLAUDE.md:1820, 1840-1844 |
| Lag sweep | 0.84-0.86x on the drained store; 0.65-0.80x after the 1% and 10% bursts; 0.80-0.82x after the full rewrite | CLAUDE.md:1821-1823 |
| Mixes | threaded scan mix 0.78-0.87x; E 0.83-0.90x; the rest level or unresolved. Still slow where a walk goes by rank (sparse forms, merge path, cursor) | CLAUDE.md:1823-1824, 1850-1852 |
| Ordered run, double write (log plus envelope) | ordered load 0.68x, buffered 0.52-0.57x, shuffled 0.80-0.84x; device bytes per byte stored 2.5 against 1.3 | `pair.sh supdb-cipher supdb-cipherbase 300000 --passes load`, CLAUDE.md:1480-1485 |
| Drain of that run | 77-94 ms against 26-35. The envelope is 43-55 ms of it (40 MB sealed and written in 33, synced in 20), plus 14 ms for one `unlink` of the retired log | `SUPDB_LOAD_PHASES=1 SUPDB_DRAIN_STEPS=1`, `strace` over `load_only supdb-cipher 300000 1`, CLAUDE.md:1486-1496 |
| Keeping the run, against the log-only shape | ordered 1.23x, buffered 1.4-1.56x | `pair.sh supdb-cipher supdb-cipherlog 300000 --passes load`, CLAUDE.md:1436-1439 |
| Envelope written whole at a seal's close | readable landing to fsync: 5 ms plain, 25 ms under the cipher (15 MB segment, 3 ms of it cipher). 21 ms with `vault::Ahead`; `O_DIRECT` made it 35, refuted | CLAUDE.md:1780-1796 |
| Envelope writer against cipher alone | 1.3 GB/s against 7.9 | `crypt_cost`, CLAUDE.md:1791-1792 |
| Out of cache (`supdb-cipher64`, 1M keys under 64 MiB) | point reads 0.17x (0.11-0.13x threaded); scans 0.26x (0.15-0.20x threaded); lag 0.23-0.39x, and 0.11-0.19x after the rewrite; read mixes 0.23-0.66x; A and F 0.75-0.92x; loads about level | `pair.sh supdb-cipher64 supdb-cipher 1000000 --reps 4`, CLAUDE.md:1854-1861 |
| A miss | about 4 µs. Over half is the kernel's fault on a fresh memfd page, a fifth the copy of ciphertext, a tenth the cipher | `perf` over `cipher_store 1000000 64`, CLAUDE.md:1861-1868 |
| Compact form under a bound | 0.73x of the raw form; 0.60 faults a read against 0.43 | `pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read --reps 1 --ops-div 1`, CLAUDE.md:1639-1650 |
| Its cause | a 4096-byte form page spans two 4080-byte chunks, so a cold read takes two ciphertext pages | CLAUDE.md:1650-1656 |
| The open's decrypts | "what an open reads of each file, not the store"; the test holds it to "a handful of chunks a file". Earlier, a quarter off the open came from `ReadOptions::populate` | README.md:271; CLAUDE.md:1350; ROADMAP.md:314-317 |
| Plain segment published as an envelope | load about 0.6x of `supdb-segment` | ROADMAP.md:310-313 |

## 2. "A store in the clear pays nothing"

What it means:
- A plain store's cost per operation is the same as at `origin/main`, counted by function. "Under what a pairing resolves" does not count as nothing (CLAUDE.md:1242-1244).

How it is counted (CLAUDE.md:1150-1158):
- The probes are `read_only.rs supdb 100000 1 n-none`, `lag0_probe.rs supdb 100000 1`, `work_probe.rs 100000 whole` and `merge_walk.rs 100000 4 1`.
- Each is built at `origin/main` from this tree's example sources and at the tree, then compared function by function with `callgrind_annotate`.
- Results, against the main line: point read 17 instructions under its 848; scan 55 under, per scan of 100 entries; writer's round 1.3% under; merge 4.4% under.
- Rule: every read path and each form of it is a row of the count (CLAUDE.md:1679-1682).
- The count first missed the scan through the compact form, which was 370 instructions over (CLAUDE.md:1159-1162, 1618-1632).

Shapes that cost the plain path:
- A second taker of a callback: `return self.scan_pieces(..)` cost 3-4 instructions an entry (CLAUDE.md:1192-1201).
- The closure arm `by_piece(move || ..)`: 2 an entry on the walk of unsealed keys, 7 on a point read (CLAUDE.md:1196-1197).
- The by-piece loop written into `scan_at`: 5 an entry (CLAUDE.md:1845-1848).
- The `Blob::read_exts` second block fetch spilled two registers an entry in the form walk; `read_exts_inline` fixed it (CLAUDE.md:1625-1631).
- `ZFile::view_of` not inlined: 1.8 (CLAUDE.md:1633-1634).
- `RefCell` borrow held in `emit_unsealed` (CLAUDE.md:1203-1211).
- `Blob::flat` through an `Option`, or a verdict-returning end: 3-4 instructions (CLAUDE.md:1836-1839).
- The early six: 4 for a cache test in `read_all`, 3 for a third `Sec` kind (CLAUDE.md:1142-1146).

Not verified:
- "a non-generic `end_slow`", "`Sec::lent` in `Blob::flat`" and "`with_extent` inlined into the form walk" are not found under those names in the docs or recent commit messages. Only `end_slow` appears, at CLAUDE.md:1171, as out of line.

## 3. Rules a redesign must respect
- "A feature that holds for some stores is a different feature from the one asked for, and which of the two is being built is said before it is built" (CLAUDE.md:1113-1115).
- "A pin is priced by what anyone reads while it stands, since the evictor waits for the oldest" (CLAUDE.md:1300-1302).
- "A mechanism made to work under a mode is priced under the mode before its default is chosen there, on the cell it exists for" (CLAUDE.md:1682-1684).
- "A line in a plan that says why a thing may not pay is an experiment owed before the thing is built to completion" (CLAUDE.md:1685-1687).
- "Two options that answer one question are one option" (CLAUDE.md:1091 area, rules paragraph).
- "A question a path must ask is put where the path already asks one" (CLAUDE.md:1244-1245).
- "A feature switched off under a mode is first asked which part of it the mode breaks" (CLAUDE.md:1505-1508). Also, "cannot work under a cipher" is a claim about a mechanism (CLAUDE.md:1416-1418).
- "A second way to read is priced beside the first, in one process" (CLAUDE.md, envelope section). The suite prices a new shape against the old one in one process; two runs do not compare (CLAUDE.md:732-738).
- "A fourth fsync of a kept file is a durability that is not there" (CLAUDE.md:1393-1395).

## 4. Open ROADMAP items
Each item, with its "done when":
- **Envelope beside its segment** (171-180): the writer hands finished pieces to the cipher, in an envelope whose chunks do not bind the length, with a last chunk that says it is last.
- **Read of one key larger than the cache** (190-196): a walk of one key's runs rests between blocks.
- **Miss at less than a fault** (202-206): a run of chunks is populated and decrypted as one. No explicit "done when".
- **Ordered run as its own log** (217-223): a run under a cipher writes no frame to the log.
- **Log file retired off the drain** (224-230): no explicit "done when".
- **Chunks that are whole pages** (237-250): `pair.sh supdb-cipherauto supdb-cipher 5000000 --bound 671088640 --passes read` reads the form ahead in both sittings, and the controller is on.
- **Form damage taken as the form's alone** (251-258): a failed chunk is the form's failure at every site, with the damage test saying which outcome each flip had.
- **Controller's model held to a measurement** (259-265): prediction and census printed side by side and agreeing.
- **Controller prices the kernel's cache alone** (266-273): not measured; no explicit "done when".
- **Writer waits for gone segment work** (274-278): the writer answers with the thread's failure.
- **Another platform** (279-282): no explicit "done when".
- **In its cache at the plain rate** (283-297): rank walks ask by window, a point read asks once per piece, and start and end are a look rather than two calls.
- **Envelope in one pass** (310-313): the segment writer writes the envelope itself.

## 5. What the owner requires
- Verified: the store must not need to fit in memory. "Nobody who asks for a store under a cipher means the stores that fit" (CLAUDE.md:1110-1111).
- Verified: the caller's cipher "was asked for first and built second" (CLAUDE.md:1070).
- Recorded as engineering rules, but not verified as quotes from the owner:
  - no cost to plain operations (CLAUDE.md:1149, 1242);
  - modes are one enum (CLAUDE.md, the `Encryption` paragraph near 1080-1100);
  - features are not disabled and called design (CLAUDE.md:1416-1418, 1505-1508, 1671-1675);
  - the README's statement that every option a store in the clear runs with, a store under a cipher runs with (README.md:290-291).
- Not verified as stated anywhere: "performance is the product's niche".

## Five numbers to beat
1. Out-of-cache point read: 0.17x (CLAUDE.md:1856).
2. A miss: 4 µs, over half of it the fresh-page fault (CLAUDE.md:1864).
3. Ordered load: 0.68x durable and 0.52-0.57x buffered, with 2.5 device bytes per byte (CLAUDE.md:1482-1485).
4. Compact form under a bound: 0.73x (CLAUDE.md:1650).
5. In-core point read: 339 instructions over 818; scan mix 0.78-0.87x (CLAUDE.md:1823, 1831).

## Three experiments skipped or done too late
1. The cost of a 4096-byte page against a 4080-byte chunk was in the plan from the start and was priced only after the form was built, tested, mutated, gated and pushed (CLAUDE.md:1656-1659).
2. The first out-of-core pairing was the last thing run (CLAUDE.md:1684-1685). The first pairing outside the cache then found six hidden costs (CLAUDE.md:1880).
3. The plain-store count left out the scan through the compact form, which had been 370 instructions over the main line since `eb377fe4` (CLAUDE.md:1618-1624). Before that, the "fits in memory" limit was stated only after the first shape had been built and paired (CLAUDE.md:1110-1113).