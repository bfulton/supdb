# G1 door: rule R1 and gate test G-door, scouted at 682b2c09

Read-only. Every path below is relative to the export
`/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/tree`.
Nothing was built, linted or run. Clippy's behaviour is stated from its
documentation as I know it, and every inference is marked "not verified".

## 0. What decides the work

- **No class (a) hole was found.** Every file, mapping or advice call
  outside the allowed modules either works on a handle the door gave
  (`vault::open`, `vault::create`, `vault::open_mapped`), or reads `/proc`,
  or runs only for a plain file. Under a cipher the engine's paths are
  under a decoy directory that cannot exist (`vault::Held::dir`,
  vault.rs:681-683), so a stray `std::fs` call on an engine path fails
  closed ("not a directory") and writes nothing. So the G-door lint can
  land at today's sites with `#[allow]`s and **no engine behaviour change**.
- **The lint changes no instruction.** `clippy.toml` and `#[allow]` are
  compile-time only. The plain-store cost property is untouched by G1-door.
- **Where the config lives decides the size of the change.** A root
  `clippy.toml` applies to every target `--all-targets` builds (tests,
  examples, the lib's own `#[cfg(test)]` modules) and, by clippy's upward
  search, to `bench/` too. That means 22 crate-level allows in tests and
  examples, 7 test-module allows in `src/`, and a `bench/clippy.toml`.
  A config in its own directory, run as a separate `--lib` invocation,
  needs none of those. I recommend the second (section 4.3), with a red
  input proven in the same invocation, because the first one's failure
  mode is an exemption list that grows.
- **Two gaps in today's lint invocation** matter more than any site:
  `cargo clippy --release` never lints `#[cfg(debug_assertions)]` code
  (21 such attributes in `src/` today, pager.rs 12 and db.rs 9; G2's
  conform pass is to live there), and the lint group also runs on macOS and arm runners, where
  Linux-only code is compiled out and Linux-only libc paths may not
  resolve.

## 1. The existing door test

`src/db/vault.rs:2876`, `the_engine_names_no_file_but_through_the_door`
(in vault.rs's `#[cfg(test)]` module).

What it reads:
- `include_str!("../db.rs")`, cut at `rfind("\n#[cfg(test)]\nmod streaming_writes")`
  (db.rs:43819-43820). So it scans db.rs lines 1-43818. That range
  includes two test modules, `merge_fences` (db.rs:5680) and `radix`
  (db.rs:18616), and the indented `#[cfg(test)]` items at db.rs:7745 and
  17794.
- Each line is cut at its first `//` (`line.split("//").next()`). A `//`
  inside a string literal would hide the rest of that line. No such line
  was looked for (not verified that none exists).
- `include_str!("../ordindex.rs")`: it finds `pub fn open(path: &Path, keys: usize)`
  and asserts the next 200 bytes contain `crate::db::vault::map(path)`.

What it bars (substring match, db.rs only): `File::open(`, `File::create(`,
`OpenOptions`, `fs::rename(`, `fs::hard_link(`, `fs::remove_file(`,
`fs::read_dir(`, `fs::metadata(`, `fs::read(`, `fs::write(`, `fs::copy(`,
`MmapBytes::open(`, `.exists()`.

What it does not see, at this tree:
- Any file but db.rs (and the one ordindex check). zfile.rs:3972
  `File::open(form)` in `swap_ahead` is not read.
- In db.rs itself: `std::fs::create_dir_all` (db.rs:28348, `Db::create`),
  `std::fs::read_to_string` (db.rs:13881 `thread_minflt`, 13893
  `thread_majflt`, 15860 `resident_beyond_use`; `fs::read(` does not match
  `fs::read_to_string(`), `memmap2::MmapOptions` (db.rs:12199 `RunMap::open`)
  and `memmap2::Mmap` (db.rs:12167, `struct RunMap`), and every `libc::`
  call. None of these is a hole (section 2), but the test cannot tell.
- Aliases (`use std::fs as x`), `Path::is_file`/`is_dir`/`metadata`, and
  `File::options` (no `File::options` exists in src/ today: search 2.1 #6).

## 2. Every file, mapping, advice and /proc site in src/

### 2.1 Searches run

All over `src/` of the export. The scanner is
`scan.py` in this directory; it prints file, line, the nearest preceding
`fn` declaration (so a struct field or enum variant line is labelled with
the function before it), whether the line is inside a `#[cfg(test)]`
item (found by brace matching from the attribute; `cfg(any(test, ..))`
is counted as not-test), the pattern name and the line. Comment lines
(starting `//`, which includes `///`) are skipped. Output: `hits.tsv`
(703 lines), with patterns in `pats.json`:

1. `fs`: `\bfs::|\bOpenOptions\b|\bFile::`
2. `Filetype`: `\bFile\b(?!::)`
3. `fsmeth`: `\.(sync_data|sync_all|set_len|read_exact_at|write_all_at|write_at|try_clone|metadata|as_raw_fd|into_raw_fd|from_raw_fd|exists|is_dir|is_file|canonicalize|read_to_end|read_to_string|symlink_metadata|read_link|try_exists)\(`
4. `fileext`: `FileExt|MetadataExt|AsRawFd|FromRawFd|AsFd|OwnedFd|BorrowedFd|RawFd`
5. `mmap`: `memmap2::|\bMmap(Mut|Options|Raw)?\b`
6. `libc`: `libc::(open|openat|mmap|munmap|madvise|posix_fadvise|sync_file_range|mincore|getrusage|fallocate|ftruncate|memfd_create|mprotect|pwritev|ioctl|syscall|sysconf|clock_gettime|sched_setscheduler|mallopt|malloc_trim|mallinfo2|dlsym|getrandom)\b`
7. `procsys`: `"/(proc|sys)/` (string literals; comment lines skipped)
8. `thread`: see section 3.

A second pass, `g1pats.json` -> `g1.tsv`, approximates what the proposed
clippy entries would flag (section 4.4).

Plain greps, each rerunnable from the export root:
- `grep -ohE 'libc::[a-zA-Z_0-9]+' -r src | sort | uniq -c` (every libc
  item named; the list is in the scratch history: madvise 12, posix_fadvise 9,
  mincore 6, syscall 4, sync_file_range 4, mmap 2, munmap 2, mprotect 2,
  pwritev 1, memfd_create 1, getrusage 1, ioctl 1, dlsym 1, getrandom 1,
  malloc_trim 1, mallopt 1, sched_setscheduler 1, clock_gettime 2, sysconf 4;
  no `open`, `openat`, `fallocate` or `ftruncate`).
- `grep -ohE 'memmap2::[a-zA-Z_0-9:]+' -r src | sort | uniq -c`.
- `grep -rn 'extern "C"\|use libc\|use memmap2\|macro_rules!' src`: the only
  `extern "C" {` block is wasmapi.rs:47 (wasm host imports); db.rs:15829
  is a fn-pointer type for `mallinfo2`; no `use libc`/`use memmap2`.
- `grep -nE '^\s*(pub(\([^)]*\))? )?use (std|memmap2|libc|::std)' src/*.rs src/db/*.rs`
  and `grep -nE ' as [A-Za-z_]+;' src/*.rs src/db/*.rs | grep -E 'use '`:
  the only renamed imports are `Ordering as AtomicOrdering`,
  `std::cmp::Ordering as KeyOrder`, `std::fmt::Write as _` and
  `crate::bytes::Bytes as _` (db.rs:42435). **No fs, libc, memmap2 or
  thread item is imported under another name.** `use std::fs::File` is at
  db.rs:31, zfile.rs:47, pager.rs:82, vault.rs:148 (`{File, OpenOptions}`),
  zfile.rs:5279 (`OpenOptions`, test).
- `grep -rn 'File::options\|OpenOptions' src | grep -v '^src/db/vault.rs'`:
  only zfile.rs tests (5279, 6037, 6091, 6947, 7081, 7260, 7990).
- `grep -rnE '\.read_dir\(|create_dir|set_permissions|\.is_symlink\(|DirBuilder|\bfs::' src`
  outside vault, pager, dirkey, bytes, crypt.
- `grep -rnE 'MmapBytes::(open|from_map)|Blob::open\(|OrdIndex::open\(|SegmentWriter::create|fn open\(path' src`:
  indirect openers. `MmapBytes::open` (bytes.rs:344, plain `File::open` +
  `Mmap::map`) is called in library code only from vault.rs:2208-2209.
  `SegmentWriter::create` opens through `vault::create_rw` (db.rs:7269).
  `OrdIndex::open` maps through `vault::map` (the door test holds this).
  `MmapBytes::from_map` is called at vault.rs:1177 and zfile.rs:4265.

How hidden sites were covered:
- Method-call syntax: patterns 3 and 4 match the method name on any
  receiver; each hit outside the allowed modules was read for its receiver.
- Renamed imports: none exist (above).
- Macro bodies: the five `macro_rules!` (blob.rs:779, 882, 941; wasmapi.rs:332,
  343) are text in the same files and are covered by the same text scan;
  blob.rs and wasmapi.rs have no hit.
- Trait impls: the crate's `Bytes`/`Paging` impls that map or advise are in
  bytes.rs and pager.rs (allowed). `std::os::unix::fs::FileExt` methods are
  hits of pattern 3/4.
- Not covered by text: a call through a function pointer stored elsewhere,
  and `std::io::Read`/`Write` methods (`write_all`, `read_exact`, `seek`)
  on a `File`, which cannot be told by name from the same calls on a
  buffer. Those are on door handles where I read them; not enumerated.

### 2.2 Outside the allowed modules: library code

Allowed modules, per the task: `src/db/vault.rs`, `src/bytes.rs`,
`src/db/pager.rs`, `src/crypt.rs`, `src/db/dirkey.rs`, `src/db/blocks.rs`.

Classes: (a) hole, a store under a cipher would bypass the door here;
(b) plain-only, or an operation on a handle the door gave, correct as it
is; (c) a memory or accounting read R12 moves into `Memory` at G3;
(d) `#[cfg(test)]`.

| site | function | what | class |
|---|---|---|---|
| db.rs:28348 | `Db::create` | `std::fs::create_dir_all(dir)` of the store's real directory, before the vault exists | (b). Under a cipher the real directory must exist to hold the marker. |
| db.rs:12167 | `struct RunMap` | field `map: memmap2::Mmap` | (b) |
| db.rs:12199 | `RunMap::open` | `memmap2::MmapOptions::new().len(cap).map(&file)` over the file `vault::open_mapped` gave | (b). Under a cipher it maps the run's memory file, by design (`vault::Mapped` holds it). |
| db.rs:6567-6574 | `pwritev_at` | `libc::pwritev(file.as_raw_fd(), ..)` | (b), door handle |
| db.rs:6689-6697 | `start_writeback` | `libc::sync_file_range(.., WRITE)` | (b), door handle; a no-op on a memory file (not verified) |
| db.rs:15764-15771 | `fadvise_dontneed_range` (linux) | `posix_fadvise(DONTNEED)` | (b), door handle. Callers: `Wal::drop_synced` db.rs:4777 (the log, which under a cipher is a real file in the real directory), `compact_run` db.rs:13607 |
| db.rs:15781-15786 | `fadvise_dontneed` (linux) | `posix_fadvise(DONTNEED)` | (b), door handle. Callers: `Seg::drop_raw_pages` db.rs:8925, `ZFile::drop_cache` zfile.rs:3258 |
| db.rs:13581 | `compact_run` | `Vec<Option<std::fs::File>>` from `vault::open` | (b). Under a cipher `vault::open` of a kept file with an envelope is `Err` (`Vault::reopen_written`, vault.rs:1113-1126), so the drop behind is skipped: an R3 no-op, not a bypass |
| db.rs:9791-9806 | `populate_ahead` | `madvise(MADV_POPULATE_WRITE)` on the writer's anonymous buffer | (b), not a file |
| db.rs:13880-13881 | `thread_minflt` | `std::fs::read_to_string("/proc/thread-self/stat")` (finishsteps trace) | (c) |
| db.rs:13892-13893 | `thread_majflt` | same, merge trace | (c) |
| db.rs:15796-15809 | `resident_share` | `libc::mincore` over a borrowed slice | (c) |
| db.rs:15858-15860 | `resident_beyond_use` | `std::fs::read_to_string("/proc/self/status")` (trim) | (c) |
| db.rs:15829-15836 | `heap::mallinfo2` | `libc::dlsym(RTLD_DEFAULT, "mallinfo2")` | (c), allocator count; not in R1's list |
| db.rs:15892, 15899 | `heap::trim`, `set_mmap_threshold` | `malloc_trim`, `mallopt` | not a file; not in R1's list |
| db.rs:305-310, 322-325 | `idle_io_priority`, `io_priority_class` | `libc::syscall(SYS_ioprio_*)` | not a file; listed because `libc::syscall` is the generic escape (4.2) |
| db.rs:10438-10442, 10453-10457 | `register_membarrier`, `membarrier_all_threads` | `libc::syscall(SYS_membarrier)` | same |
| db.rs:34363-34367 | `idle_cpu_priority` | `sched_setscheduler` | not a file; not in R1's list |
| db.rs:15962-15968, 34374-34382 | `mono_secs`, `thread_cpu_ns` | `clock_gettime` | not a file; not in R1's list |
| zfile.rs:3926-3940 | `drop_written_range` (linux) | `sync_file_range(WAIT_BEFORE|WRITE|WAIT_AFTER)` then `posix_fadvise(DONTNEED)` on the build's output | (b). The output is `vault::create(tmp)` (zfile.rs:4607); under a cipher a memory file, and `persist_form` writes its envelope (zfile.rs:4373) |
| zfile.rs:3957-3986 | `swap_ahead` (linux) | `posix_fadvise(DONTNEED)` on the segment handle, then **`File::open(form)`** (zfile.rs:3972) and `posix_fadvise(WILLNEED)` | (b), see below |
| zfile.rs:4002-4019 | `drop_behind` (linux) | `madvise(DONTNEED)` on the mapping, `posix_fadvise(DONTNEED)` on the handle | (b), plain build only (`Walked::Mapped`) |
| zfile.rs:4035-4042 | `ask_ahead` (linux) | `madvise(WILLNEED)` on the mapping base | (b), plain build only |
| zfile.rs:4198, 4207, 4262-4265 | `SegSrc::File`, `open_seg`, `build_file_seen` | `memmap2::Mmap::map(&file)` of a `vault::open` handle | (b). `open_seg` takes this arm only where `vault::kept(path)` is false (zfile.rs:4204), i.e. a plain store |
| zfile.rs:1869, 2023, 3255-3259 | `ZFile.file`, attach, `drop_cache` | `Option<std::fs::File>` from `vault::open(path).ok()` | (b). `None` under a cipher once the file has its envelope |
| zfile.rs:4380 | `build_file_seen` | `vault::open(out)` then `sync_data` | (b), door handle |
| zfile.rs:4942-4949 | `thread_major_faults` | `libc::getrusage(RUSAGE_THREAD)` | (c) |
| zfile.rs:5210-5211 | `read_budget` | `std::fs::read_to_string` of `/proc/meminfo`, `/proc/self/cgroup`, `/proc/mounts`, `/proc/self/status` and the cgroup files (`budget_from`, zfile.rs:5217-5264) | (c) |
| flatindex.rs, blob.rs, ordindex.rs (lib), zselect.rs, zpages.rs (lib), zlz4.rs, block.rs, index.rs, format.rs, reserve.rs, wasmapi.rs | -- | none | -- |

**`swap_ahead`'s `File::open(form)` (zfile.rs:3972).** It opens a form
file by path, past the door. Its only caller, `Db` at db.rs:42447, reaches
it only where `raw.record_region_span()` is `Some` (db.rs:42429). That
function asks `self.src.slice_at(0, 1)` (blob.rs:1414), which a paged
mapping refuses, its lendable length being zero (bytes.rs:267, 478; the
refusal path itself not read line by line). The cipher's arm is the branch
above it (db.rs:42430-42441), which forwards through `advise_dontneed_range`
and `vault::map(&form)`. So under a cipher the call is unreachable, and if
it were reached the decoy path would make `File::open` fail and the
function return 0. Class (b), plain-only. It is the one site where G1 could
remove a call with no behaviour change: `vault::open(form)` returns the same
`File` for a plain store (vault.rs:1892-1898, `locate` is `None` when no
vault is registered, vault.rs:667-673). That swap is optional; the lint does
not need it (an `#[allow]` with this paragraph's reason does).

The File type and the `std::os::unix` traits on door handles (pattern 2, 3
and 4 hits in db.rs and zfile.rs library code) are listed in appendix A3.
They are handles the door gave. They are not barred at G1 (section 4.2).

### 2.3 Outside the allowed modules: test code (class d)

From `hits.tsv`, test lines with a file/mapping/proc kind:
- db.rs: 11 lines, in `mod streaming_writes` (db.rs:43820; lines 43825,
  43826, 43925, 43926), `mod landing_facts` (44051; 44062, 44063, 44083,
  44100), `mod snap_arena` (44144; 44234), `mod sweep_gate` (44577; 44593,
  44646).
- zfile.rs: 61 lines of pattern 1/3/4/5/6/7 kinds, all inside `mod tests`
  (zfile.rs:5276 to the file's end). `std::fs::read`/`write`/`copy`/
  `remove_file`, `OpenOptions::new().write(true).open`, `FileExt::write_at`
  and `write_all_at`, `Path::exists`, `File::open` + `posix_fadvise` in
  `evict` (7456-7461), `/proc/thread-self/stat` (5450), and the fake
  machine's `/proc` names in `read_budget`'s test (5279-5319, strings only).
- zpages.rs: 3 lines in `mod tests` (zpages.rs:3539; 7507, 7508, 7630).
- ordindex.rs: 17 lines in `mod tests` (ordindex.rs:1129): `std::fs::write`
  (1207) and `std::fs::remove_file` (16 sites).
- zselect.rs: 2 `/sys/fs/cgroup` string literals in `mod tests`
  (2502, 2506), no call.
These are linted under `--all-targets` (the lib's test target is built with
`cfg(test)`). They need an allow under option A of section 4.3 and nothing
under option B.

### 2.4 Inside the allowed modules

Library sites by function are in appendix A1 (generated from `hits.tsv`).
Summary of what each module does that the lint would bar:
- vault.rs: the door itself, every `std::fs` free function and
  `File`/`OpenOptions` constructor (`create`, `create_rw`, `open`,
  `open_write`, `open_append`, `open_slot`, `rename`, `open_mapped`,
  `hard_link`, `remove_file`, `exists`, `is_file`, `len`, `names`, `read`,
  `sync_dir`, `open_dir`), the vault's own file work (`write_marker`,
  `refuse_marked`, `key_id`, `rewrap`, `sweep`, `persist_with`,
  `persist_form`, `write_envelope_at`, `reopen`, which builds
  `/proc/self/fd/{}` at vault.rs:709), `memmap2::Mmap::map` (1177),
  `sync_file_range`/`posix_fadvise` (`write` 746, `drop_cached` 2169-2177),
  `getrandom` (286). 33 test lines.
- pager.rs: `memfd_create` (`memfile`, 810), `Mmap::map` and `advise`
  (`Back::open`, 736-746; `Paged::advise`, 1368-1370), `mmap`/`munmap`/
  `madvise`/`mprotect`/`mincore`/`posix_fadvise` over the plaintext mapping
  (`map_shared`, `adopt`, `fill`, `free`, `drop`, `resident`, `forget`,
  `willneed`, `give_back`). 15 test lines.
- bytes.rs: `MmapBytes::open` (`File::open` + `Mmap::map`, 345-350),
  `from_map`, and the advice and residency methods (`advise_random`,
  `advise_normal`, `advise_dontneed_range`, `resident_at`, `advise_dontneed`,
  `advise_willneed`, `resident_pages`, `os_page_size`). 2 test lines.
- crypt.rs: `File::open("/dev/urandom")` (`encrypt`, 383), an anonymous
  `MmapRaw` with `madvise` (`Kept::new` 661-666, `populate` 732). 1 test line.
- dirkey.rs: `File::open` of directories, `canonicalize`, `metadata`, and
  `ioctl` (`ask`, `state`, `protect`, `unlock`, `filesystem_root`).
- blocks.rs: anonymous `mmap`/`munmap` and `mincore` (`map`, `unmap`,
  `resident_of`).

forms.md's allowlist names `vault`, `pagefile`, `bytes`, `pager`,
`layer::Memory` and the wrappers; it does not name crypt.rs, dirkey.rs or
blocks.rs. Each of the three needs an allow too, for the reasons above
(randomness and anonymous memory; the kernel-key ioctls; the engine's own
anonymous blocks). Recommend adding them to the list by name.

## 3. Threads, parks, joins and waits (S1's half, same lint)

Classified from pattern 8 (`thread::spawn|thread::Builder|\bBuilder::new\(\)|\.spawn(_scoped)?\(|thread::scope|thread::park|park_timeout|thread::sleep|\bsleep\(|\.join\(\)|\bCondvar\b|\.wait(_timeout|_while|_timeout_while)?\(|\.recv(_timeout)?\(|try_recv|mpsc::|thread::yield_now`),
output `threads.tsv` (163 lines), classified into `threads_classified.tsv`.
Removed as false hits: `crate::ordindex::Builder::new()` (db.rs:8756, 9244;
ordindex.rs:1189), `b.job.join()` at db.rs:39280 (calls the crate's
`ZJob::join`, db.rs:5377, whose own `h.join()` at 5379-5380 are counted),
and `self.sync_helper...wait()` at db.rs:39701 (the crate's
`SyncHelper::wait`, whose `recv` at db.rs:38554 is counted). Cross-check:
`grep -rhoE 'std::thread::[a-zA-Z_]+(::[a-z_]+)?' src | sort | uniq -c`
gives Builder::new 11, spawn 14, scope 5, park 2, park_timeout 16,
sleep 24, yield_now 9.

Counts of the calls R1 names (spawn, Builder::spawn, scope and Scope::spawn,
park, park_timeout, sleep, JoinHandle::join, Condvar wait*, mpsc recv*);
channel creation, `try_recv` (non-blocking) and `yield_now` are not counted:

| module | library sites | functions | test sites |
|---|---|---|---|
| db.rs | 73 | 55 | 12 |
| pager.rs | 6 | 6 | 13 |
| flatindex.rs | 4 | 1 (`encode_inline`, `thread::scope` + `Scope::spawn` at 1603, 1606, 1680, 1689; an in-memory encode, reads no file) | 0 |
| crypt.rs | 0 | 0 | 3 (incl. `Barrier::wait`) |
| zfile.rs | 0 | 0 | 2 (`sleep`) |
| zpages.rs | 0 | 0 | 2 |

Library kinds: Builder::spawn 11, thread::spawn 8, thread::scope 2,
Scope::spawn 2, park 2, park_timeout 16, sleep 13, JoinHandle::join 25,
Condvar::wait_timeout 1 (pager.rs:592), mpsc recv 3 (db.rs:32574, 38523,
38554). Not counted: `try_recv` db.rs:22864, 38880, 38899; `yield_now`
crypt.rs:992, db.rs:34873, pager.rs:1108, 1147, 1188, 1223. `for x in rx`
(Receiver as an iterator) was searched with the same pattern and not found
(not verified beyond pattern 8).

Library sites by function (file, function:line of its declaration, kind@line):

- `src/db.rs` `join:5377`: JoinHandle::join@5379, JoinHandle::join@5380
- `src/db.rs` `spawn:6766`: Builder::spawn@6792
- `src/db.rs` `run:6801`: park@6870
- `src/db.rs` `stop:6887`: JoinHandle::join@6891
- `src/db.rs` `trim_loop:15979`: sleep@15991, park@15996
- `src/db.rs` `start_trimmer:16017`: Builder::spawn@16030
- `src/db.rs` `upkeep_held:22758`: sleep@22764
- `src/db.rs` `install_ahead:22844`: JoinHandle::join@22975
- `src/db.rs` `close_run:30333`: JoinHandle::join@30422, Builder::spawn@30452, park_timeout@30493
- `src/db.rs` `take_fresh_mem:31182`: park_timeout@31231
- `src/db.rs` `start_ahead:32207`: thread::spawn@32325
- `src/db.rs` `stop_ahead:32363`: JoinHandle::join@32371
- `src/db.rs` `join_ahead:32379`: JoinHandle::join@32382
- `src/db.rs` `go_away:32398`: Builder::spawn@32414
- `src/db.rs` `land_seal:32453`: park_timeout@32476
- `src/db.rs` `hand_seal:32503`: thread::spawn@32522
- `src/db.rs` `maint_request:32558`: mpsc recv@32574
- `src/db.rs` `release_seal_landing_after:32652`: thread::spawn@32657, sleep@32658
- `src/db.rs` `release_seal_landing_once_taken:32678`: thread::spawn@32680, sleep@32682, sleep@32684
- `src/db.rs` `release_seal_durable_after:32695`: thread::spawn@32700, sleep@32701
- `src/db.rs` `zpages_settle_with:33495`: sleep@33509
- `src/db.rs` `keep:34405`: park_timeout@34422, park_timeout@34425
- `src/db.rs` `take_back:34963`: park_timeout@35001
- `src/db.rs` `keep_upkeep:35137`: park_timeout@35161, park_timeout@35180
- `src/db.rs` `start_upkeep:35394`: Builder::spawn@35401
- `src/db.rs` `hold_within:35690`: park_timeout@35706
- `src/db.rs` `hold_for_upkeep:35715`: park_timeout@35731
- `src/db.rs` `join_upkeep:35741`: sleep@35761
- `src/db.rs` `stop_upkeep:35774`: JoinHandle::join@35780
- `src/db.rs` `start_keeper:35967`: Builder::spawn@35976
- `src/db.rs` `settle_keeper:36007`: sleep@36025
- `src/db.rs` `stop_keeper:36029`: JoinHandle::join@36036
- `src/db.rs` `stop_trimmer:36056`: JoinHandle::join@36073
- `src/db.rs` `release_trim_after:36140`: thread::spawn@36142, sleep@36145
- `src/db.rs` `zbuild_hold_wait:38346`: park_timeout@38352
- `src/db.rs` `seal_hold_wait:38358`: park_timeout@38360
- `src/db.rs` `spawn:38517`: Builder::spawn@38520, mpsc recv@38523
- `src/db.rs` `wait:38552`: mpsc recv@38554
- `src/db.rs` `drop:38560`: JoinHandle::join@38563
- `src/db.rs` `run:38868`: park_timeout@38894, JoinHandle::join@38902
- `src/db.rs` `answer:38946`: park_timeout@38981
- `src/db.rs` `land_front_readable:39120`: JoinHandle::join@39150
- `src/db.rs` `stop:39272`: JoinHandle::join@39283, JoinHandle::join@39290, JoinHandle::join@39298, JoinHandle::join@39301
- `src/db.rs` `join_seal:39502`: park_timeout@39524
- `src/db.rs` `land_front:39543`: JoinHandle::join@39561
- `src/db.rs` `land_readable:39622`: helper.wait (custom; recv inside SyncHelper::wait at 38554)@39701
- `src/db.rs` `start_tier:40222`: thread::spawn@40245
- `src/db.rs` `join_tier:40260`: JoinHandle::join@40267
- `src/db.rs` `z_sample_start:41671`: Builder::spawn@41708
- `src/db.rs` `z_sample_collect:41770`: JoinHandle::join@41785
- `src/db.rs` `z_start:42931`: Builder::spawn@43083
- `src/db.rs` `z_collect:43131`: JoinHandle::join@43149, JoinHandle::join@43156
- `src/db.rs` `z_warm:43423`: Builder::spawn@43430
- `src/db.rs` `start_compact:43484`: thread::spawn@43637, park_timeout@43665
- `src/db.rs` `join_merge:43717`: JoinHandle::join@43735
- `src/db.rs` `drop:43789`: JoinHandle::join@43812
- `src/db/pager.rs` `new:317`: Builder::spawn@341
- `src/db/pager.rs` `stop:415`: JoinHandle::join@420
- `src/db/pager.rs` `breathe_swamped:529`: sleep@537
- `src/db/pager.rs` `breathe_unless:557`: sleep@565
- `src/db/pager.rs` `run:571`: Condvar::wait_timeout@592
- `src/db/pager.rs` `pass:605`: sleep@645
- `src/flatindex.rs` `encode_inline:1450`: thread::scope@1603, Scope::spawn@1606, thread::scope@1680, Scope::spawn@1689

Of these, the test hooks `release_seal_landing_after`, `_once_taken`,
`release_seal_durable_after` and `release_trim_after` are library code
(public seams for tests/), not `#[cfg(test)]`.

## 4. The lint

### 4.1 What clippy does, from its documentation, and what I infer

From the documentation (as I know it; not checked against the toolchain
CI installs, `dtolnay/rust-toolchain@stable`):
- `disallowed_methods` and `disallowed_types` are configured in
  `clippy.toml` (`disallowed-methods = [ "path", { path = "...", reason = "..." } ]`).
  They are in the `style` group, so they warn by default once configured,
  and `-D warnings` makes them errors.
- The configuration file is `clippy.toml` or `.clippy.toml`, found in
  `CLIPPY_CONF_DIR` if set, else in the package's manifest directory,
  and if not there, in its parent directories.
- `cargo clippy --all-targets` lints `--lib --bins --tests --benches
  --examples`: the lib, the lib's unit-test target (so `#[cfg(test)]` code
  in src/), every `tests/*.rs`, every `examples/*.rs`. All targets of one
  package share one configuration; there is no per-target or per-module
  configuration. Exemption is by `#[allow(clippy::disallowed_methods)]` /
  `#[allow(clippy::disallowed_types)]` on a crate, module, item or
  statement.
- Clippy lints only workspace members (cargo's `RUSTC_WORKSPACE_WRAPPER`).
  `bench/` is its own workspace and depends on `supdb` by path
  (bench/Cargo.toml:23), so `supdb` is compiled there without clippy.

Inferred, not verified:
- The lint matches by the resolved definition, not by spelling. So a call
  through a re-export or a `use .. as` alias is caught; `file.sync_data()`
  (an inherent method) is caught under `std::fs::File::sync_data`; a
  method call through a trait (`FileExt::read_at`) resolves to the trait's
  item and is caught under `std::os::unix::fs::FileExt::read_at`; a libc
  function called through `use libc as c` is caught under `libc::madvise`.
- A configured path that goes through libc's glob re-exports
  (`libc::madvise` is defined in a platform submodule) resolves. Whether
  an unresolvable path is silently ignored or reported depends on the
  clippy version: older clippy ignored it (a lint that is not running),
  newer clippy warns, and with `-D warnings` that may fail the macOS
  runner for Linux-only names (`sync_file_range`, `memfd_create`,
  `posix_fadvise`, which libc does not define on macOS). Some versions
  accept `allow-invalid = true` per entry. **Check this on the toolchain
  before landing**; the red inputs of 4.6 are what prove each family
  resolves.
- A function named as a value (`let f = libc::madvise; f(..)`,
  `.map(std::fs::read)`) may or may not be flagged, depending on the
  version. Probe it in the red run; if it is not flagged, the source test
  of 4.5 carries it.
- Code compiled out is not linted: `#[cfg(not(target_os = "linux"))]` on
  Linux, `#[cfg(target_os = "linux")]` on macOS, and
  `#[cfg(debug_assertions)]` under `--release`.

What cannot be expressed in `clippy.toml` and stays a source test:
1. Where an allow may stand. Nothing stops a new
   `#[allow(clippy::disallowed_methods)]` in zfile.rs. A ratchet:
   count `allow(clippy::disallowed_` per file against a checked-in list
   (`tests/data/door-allows.txt`, one line per allow, with its reason);
   a count may not grow, and a fall is lowered in the same commit.
2. Foreign declarations: a new `extern "C" { fn madvise(..); }` in the crate
   bypasses `libc::madvise`. Bar `extern "C" {` outside wasmapi.rs (its one
   block, wasmapi.rs:47, is wasm-only).
3. Names looked up at run time: `libc::dlsym` (db.rs:15836) and
   `libc::syscall` with a number. Bar both symbols in clippy; the four
   `syscall` sites and the one `dlsym` site get allows with reasons.
4. Operations on a `std::fs::File` the door handed out: `std::io::Write`/
   `Read`/`Seek` methods cannot be barred for `File` alone, since the
   config names the trait's item, which a `Vec<u8>` writer also calls.
   S1's typed handles carry this; at G1 the handles are the door's, so
   the calls are correct.
5. Code compiled out on the lint's platform or profile (above).
6. `/proc` and `/sys` reads are only caught by barring the `std::fs`
   functions they use (every one today is `std::fs::read_to_string`).
   A `/proc` path handed to an allowed function, or opened inside an
   allowed module, is not a symbol. A source test can bar the string
   literals `"/proc/` and `"/sys/` outside the allowed modules and
   `layer::Memory`.

### 4.2 Proposed entries (G1, the file, mapping, advice and /proc half)

Not barred at G1: the `std::fs::File` type and its methods (`sync_data`,
`sync_all`, `set_len`, `metadata`, `try_clone`, `as_raw_fd`, `FileExt`).
The door hands out `std::fs::File`; pattern 2 matches 39 lines of db.rs
(three of them, 35488, 35631 and 35637, are the unrelated `Hand::File`
variant) and 15 of zfile.rs, and barring it would put an allow on the
writer's hot functions for no hole. S1's typed handles are where the type
is barred.

```toml
# clippy.toml -- the door (forms.md R1, gate test G-door).
# Files, mappings, advice and process reads go through crate::db::vault,
# crate::bytes, crate::db::pager (and later layer::Memory). An allow names
# its reason and is counted by tests/data/door-allows.txt.
disallowed-methods = [
  # std::fs by path
  { path = "std::fs::File::open", reason = "open through crate::db::vault" },
  { path = "std::fs::File::create", reason = "create through crate::db::vault" },
  { path = "std::fs::File::create_new", reason = "create through crate::db::vault" },
  { path = "std::fs::File::options", reason = "open through crate::db::vault" },
  { path = "std::fs::OpenOptions::new", reason = "open through crate::db::vault" },
  { path = "std::fs::OpenOptions::open", reason = "open through crate::db::vault" },
  "std::fs::read", "std::fs::read_to_string", "std::fs::write",
  "std::fs::read_dir", "std::fs::rename", "std::fs::remove_file",
  "std::fs::remove_dir", "std::fs::remove_dir_all", "std::fs::create_dir",
  "std::fs::create_dir_all", "std::fs::hard_link", "std::fs::copy",
  "std::fs::metadata", "std::fs::symlink_metadata", "std::fs::canonicalize",
  "std::fs::read_link", "std::fs::set_permissions", "std::fs::exists",
  "std::os::unix::fs::symlink",
  # paths asked of the filesystem
  "std::path::Path::exists", "std::path::Path::try_exists",
  "std::path::Path::is_file", "std::path::Path::is_dir",
  "std::path::Path::is_symlink", "std::path::Path::metadata",
  "std::path::Path::symlink_metadata", "std::path::Path::read_dir",
  "std::path::Path::canonicalize", "std::path::Path::read_link",
  # the crate's own plain mapping by path
  { path = "supdb::bytes::MmapBytes::open", reason = "map through crate::db::vault::map" },
  # mappings
  "memmap2::Mmap::map", "memmap2::MmapMut::map_mut", "memmap2::MmapMut::map_anon",
  "memmap2::MmapRaw::map_raw", "memmap2::MmapOptions::new",
  "memmap2::Mmap::advise", "memmap2::Mmap::advise_range",
  "memmap2::Mmap::unchecked_advise", "memmap2::Mmap::unchecked_advise_range",
  # libc: files, mappings, advice, syncs, accounting, escapes
  "libc::open", "libc::open64", "libc::openat", "libc::creat",
  "libc::mmap", "libc::mmap64", "libc::munmap", "libc::mremap", "libc::mprotect",
  "libc::madvise", "libc::posix_madvise", "libc::posix_fadvise", "libc::posix_fadvise64",
  "libc::readahead", "libc::sync_file_range", "libc::mincore", "libc::getrusage",
  "libc::fallocate", "libc::fallocate64", "libc::ftruncate", "libc::ftruncate64",
  "libc::memfd_create", "libc::msync", "libc::fsync", "libc::fdatasync",
  "libc::pread", "libc::pwrite", "libc::preadv", "libc::pwritev",
  "libc::ioctl", "libc::syscall", "libc::dlsym",
]
disallowed-types = [
  { path = "std::fs::OpenOptions", reason = "open through crate::db::vault" },
  "memmap2::Mmap", "memmap2::MmapMut", "memmap2::MmapRaw", "memmap2::MmapOptions",
]
```

The crate path for `MmapBytes::open` assumes clippy resolves the local
crate's own items by crate name (`supdb::...`); not verified. Several libc
names above (`open64`, `mmap64`, `posix_fadvise64`, `fallocate64`,
`ftruncate64`, `readahead`, `sync_file_range`, `memfd_create`) are
Linux-only; see 4.1 on unresolvable paths.

S1 adds the thread half:

```toml
  "std::thread::spawn", "std::thread::Builder::spawn",
  "std::thread::Builder::spawn_scoped", "std::thread::scope",
  "std::thread::Scope::spawn", "std::thread::park", "std::thread::park_timeout",
  "std::thread::sleep", "std::thread::JoinHandle::join",
  "std::thread::ScopedJoinHandle::join",
  "std::sync::Condvar::wait", "std::sync::Condvar::wait_while",
  "std::sync::Condvar::wait_timeout", "std::sync::Condvar::wait_timeout_while",
  "std::sync::Condvar::wait_timeout_ms",
  "std::sync::mpsc::Receiver::recv", "std::sync::mpsc::Receiver::recv_timeout",
  "std::sync::mpsc::Receiver::iter", "std::sync::Barrier::wait",
```

My recommendation is to add these at S1, not G1. At G1 they need allows
on 55 functions of db.rs and one of flatindex.rs (section 3), each saying
"owed to S1's `pin::` wrapper", and they move nothing until the wrappers
exist. The growth they guard against can be held at G1 more cheaply by the
source ratchet (4.5) counting `std::thread::` per file. If the owner wants
forms.md's "G-door at today's counts" literally, the list of 56 functions
is section 3; pager.rs is covered by its module allow.

### 4.3 Where the configuration lives: two options

**Option A: `clippy.toml` at the repository root, the lint group as it is.**
- The root config applies to every target of `--all-targets`, so:
  - `#![allow(clippy::disallowed_methods, clippy::disallowed_types)]` at the
    top of each of the 19 `tests/*.rs` files and each of the 3
    `examples/*.rs` files: **22 crate roots**. (Text grep: 18 of the 19 test
    files name a barred item themselves; `tests/resident.rs` does not but
    includes `tests/common/mod.rs`, which does. A `#![allow]` inside
    `tests/common/mod.rs` would cover the 15 crates that include it, and the
    four that do not -- blob, checksums_off, dict, ranges -- still need
    their own.)
  - A module allow on each `#[cfg(test)]` module in src/ with hits:
    db.rs `streaming_writes`, `landing_facts`, `snap_arena`, `sweep_gate`
    (file half; `threading` and `lockfree` too at S1), zfile.rs `tests`,
    zpages.rs `tests`, ordindex.rs `tests`: **7 modules** (9 at S1).
  - The clippy upward search makes `bench/` and `experiments/ffi/capi`
    inherit the root file (inferred). Each needs its own `clippy.toml`
    (an empty file stops the search): **2 files**.
- Cost: 31 exemptions that are not the door, which the ratchet must also
  carry, and a rule CLAUDE.md states the other way ("No module carries a
  blanket lint allowance").

**Option B (recommended): the door's config in its own directory, run as
its own invocation over the library alone.**
- `scripts/door/clippy.toml` holds the entries. Nothing at the root, so
  the main lint, tests, examples and bench see no config.
- The door run, added to the lint group:
  ```sh
  CLIPPY_CONF_DIR=scripts/door cargo clippy --profile checked --lib \
      --target-dir target/door -- -D warnings
  ```
  - `--lib` excludes `#[cfg(test)]` code, so class (d) needs nothing.
  - `--profile checked` has `debug-assertions = true`, so debug-only code
    is linted, which `--release` misses (21 `cfg(debug_assertions)`
    attributes in src/ today: pager.rs 12, db.rs 9; G2's
    conform pass is to live there).
  - `--target-dir target/door`: its own artifacts, so a run without the
    config can never answer for it. Whether clippy tracks
    `CLIPPY_CONF_DIR` in cargo's fingerprint is not verified, and with a
    shared directory a cached check would be a lint that did not run.
  - Linux only (the lint group runs on macOS and arm runners too:
    `.github/workflows/ci.yml:38`). On other hosts the script says
    "door: skipped, not Linux" and the CI Linux leg is the gate.
- Cost: a check build of the lib and its three dependencies in a second
  target directory (time not measured), and no exemption outside src/'s
  library code.

### 4.4 The `#[allow]` sites under option B (G1, file half)

Module level, `#![allow(clippy::disallowed_methods, clippy::disallowed_types)]`
with a one-line reason at the top of the file:
- `src/db/vault.rs`: the door.
- `src/db/pager.rs`: the cache's memory file and its mappings.
- `src/bytes.rs`: the mapped byte source and its advice (`MmapBytes`).
- `src/crypt.rs`: `/dev/urandom` for an envelope's id, and the reader's
  anonymous plaintext mapping.
- `src/db/dirkey.rs`: the kernel-key ioctls on directory descriptors.
- `src/db/blocks.rs`: the engine's anonymous blocks, mapped and counted.
CLAUDE.md's sentence "No module carries a blanket lint allowance" is
amended to name these six and say why.

Item level (`#[allow(...)]` on the fn or struct), each with its reason:

| site | item | reason |
|---|---|---|
| db.rs:305, 322 | `idle_io_priority`, `io_priority_class` | `syscall(SYS_ioprio_*)`: not a file |
| db.rs:6567 | `pwritev_at` | `pwritev` on a handle the door gave (S1: the handle's method) |
| db.rs:6689 | `start_writeback` | `sync_file_range` on a door handle (S1: `writeback`) |
| db.rs:9791 | `populate_ahead` | `madvise` on the writer's anonymous buffer |
| db.rs:10438, 10453 | `register_membarrier`, `membarrier_all_threads` | `syscall(SYS_membarrier)`: not a file |
| db.rs:12166 | `struct RunMap` | `memmap2::Mmap` of the run's file, from `vault::open_mapped` |
| db.rs:12191 | `RunMap::open` | `MmapOptions::map` of that handle (S1: `as_os()`) |
| db.rs:13880, 13892 | `thread_minflt`, `thread_majflt` | `/proc/thread-self/stat` for a trace (G3: `Memory`) |
| db.rs:15764 | `fadvise_dontneed_range` (linux) | advice on a door handle (S1/R3) |
| db.rs:15781 | `fadvise_dontneed` (linux) | advice on a door handle (S1/R3) |
| db.rs:15796 | `resident_share` | `mincore` (G3: `Memory`) |
| db.rs:15831 | `heap::mallinfo2` | `dlsym` of glibc's allocator count |
| db.rs:15858 | `resident_beyond_use` | `/proc/self/status` (G3: `Memory`) |
| db.rs:28345 | `Db::create` | `create_dir_all` of the store's real directory, before any vault |
| zfile.rs:3926 | `drop_written_range` (linux) | writeback and drop on the build's own output, a door handle |
| zfile.rs:3957 | `swap_ahead` (linux) | advice on a door handle; `File::open(form)` is plain-only (2.2), or replace it with `vault::open` and drop that half of the reason |
| zfile.rs:4002 | `drop_behind` (linux) | advice in the plain build only (`Walked::Mapped`) |
| zfile.rs:4035 | `ask_ahead` (linux) | advice in the plain build only |
| zfile.rs:4242 | `build_file_seen` | `Mmap::map` of a plain segment's door handle (`SegSrc::File`) |
| zfile.rs:4942 | `thread_major_faults` | `getrusage` (G3: `Memory`) |
| zfile.rs:5210 | `read_budget` | `/proc` and cgroup reads (G3: `Memory`) |

That is 17 items in db.rs and 7 in zfile.rs (24), plus 6 module
allows. Under option A add the 22 + 7 + 2 of 4.3.

The approximation of what the lint would flag (`g1pats.json`, `g1.tsv`;
`g1_outside.tsv` for outside the allowed modules) found these sites and no
others in library code outside the allowed modules, after dropping three
false hits of `\.advise\(`, which are the crate's own `Reader::advise`
(db.rs:21157, 21718, 27778). It is a text approximation of a symbol lint:
the red run of 4.6 and the first real run are the measurement.

### 4.5 The source test that stays

Rewrite `the_engine_names_no_file_but_through_the_door` (vault.rs:2876)
into what clippy cannot hold (4.1):
- the allow ratchet over every file in src/ (count of
  `allow(clippy::disallowed_` against `tests/data/door-allows.txt`);
- no `extern "C" {` outside wasmapi.rs;
- no `"/proc/` or `"/sys/` literal outside the allowed modules and the
  items allowed for G3 (today: db.rs 13881, 13893, 15860; zfile.rs
  5218-5264; vault.rs:709; zselect.rs test literals);
- at G1, the count of `std::thread::` per file, as a ratchet, if the
  thread half waits for S1;
- keep the ordindex check (`OrdIndex::open` maps through `vault::map`).
The red input for this test: add `#[allow(clippy::disallowed_methods)]`
to a function in zfile.rs; the count for zfile.rs grows and the test fails.

### 4.6 The red input that proves the gate fires

forms.md's: `std::fs::File::open` added to zfile.rs. Make it a script step
of the door run, never a manual edit:

1. Copy `Cargo.toml`, `Cargo.lock` and `src/` to a fresh directory under
   `$TMPDIR` (the build's own; CLAUDE.md on shared temp directories).
2. Find the line `fn drop_written(file: &File) {` (zfile.rs:3917, a
   function compiled on every target and given no allow in 4.4) in the
   copy's `src/db/zfile.rs`, assert it occurs exactly once, and insert
   after it:
   ```rust
   let _ = std::fs::File::open("/");                         // a path call
   { use std::fs as red; let _ = red::read("/"); }           // an alias
   let _ = std::path::Path::new("/").exists();               // a method on a value
   let _ = memmap2::MmapOptions::new();                      // memmap2
   unsafe { use libc as c; c::madvise(std::ptr::null_mut(), 0, c::MADV_NORMAL); } // libc by alias
   let _red: fn(&str) -> std::io::Result<String> = |p| std::fs::read_to_string(p); // inside a closure
   ```
   An anchor inside a function that carries an allow would mask the red
   input; the step asserts the anchor's function is not in
   `tests/data/door-allows.txt`.
3. Run the same door command against the copy, with its own target
   directory.
4. Require: a non-zero exit; and the output names zfile.rs with one
   `disallowed` diagnostic per inserted line, six. A count short of six
   names the family whose path did not resolve.
5. Optionally probe `let f = std::fs::read; let _ = f("/");` and record
   whether it is flagged; the result says whether 4.5 must also cover
   functions named as values.

At S1, add `std::thread::sleep(std::time::Duration::ZERO);` to the same
insertion and require seven.

## 5. What lands at G1 without changing engine behaviour

All of G-door as above: `scripts/door/clippy.toml`, the door step and its
red step in `scripts/check.sh`'s lint group (or a group of its own), 6
module allows and 24 item allows, the rewritten source test with
`tests/data/door-allows.txt`, and the CLAUDE.md amendment. None of it
changes an instruction of a build, so G-cost has nothing to count for it.

Holes of class (a) to fix first: **none found.** The candidates read and
why each is not one:
- zfile.rs:3972 `File::open(form)` in `swap_ahead`: unreachable under a
  cipher, and fail-closed if reached (2.2). A test that would show it
  becoming reachable: a store under a cipher with `ZPages::Always` and
  `zpages_land_drop_past` driven to the crossing, asserting that
  `ahead_swapped` counts and that the swap went by the paged branch
  (db.rs:42430-42441). Not written; not required for G1. Whether an
  existing test of tests/zpages.rs drives that crossing under a cipher is
  not verified (CLAUDE.md says "what the probe alone decides is the swap at
  the crossing, which the test now waits for").
- db.rs:13581 `compact_run` and db.rs:8925 `Seg::drop_raw_pages`: advice
  on `vault::open` handles, which under a cipher are `Err` for a file with
  an envelope, so the advice is skipped. That is R3's silent no-op (the
  paged mapping's own advice is forwarded by `Blob::advise_dontneed`), not
  a door bypass. It belongs to G1's no-op rewrites (forms.md R3), not to
  the door.
- db.rs:28348 `create_dir_all`: the real directory, needed under a cipher.
- db.rs:12199 and zfile.rs:4265 mappings: handles the door gave, mapped
  as designed.

Not verified, from this note:
- every clippy behaviour marked inferred in 4.1, above all path
  resolution through libc's re-exports, crate-local paths, trait methods,
  function values, unresolvable-path handling per toolchain, and whether
  `CLIPPY_CONF_DIR` is in cargo's fingerprint;
- that `posix_fadvise` and `sync_file_range` on a memory file do nothing;
- that `Paged`'s `slice_at` refuses every range (read from bytes.rs's
  comments at 267 and 478, not from the code path);
- the door run's time;
- that no db.rs line before 43819 carries a `//` inside a string that hides
  a barred call from today's text test.

## Files in this directory


- `scan.py`, `pats.json`, `hits.tsv`: the scan of 2.1.
- `g1pats.json`, `g1.tsv`, `g1_outside.tsv`: the approximation of 4.4.
- `threads.tsv`, `threads_classified.tsv`, `thread_fns.md`: section 3.
- `appendix.md`: A1 (allowed modules by function), A2 (their test lines),
  A3 (File type and unix traits on door handles outside them), appended
  below. The function column is the nearest preceding `fn` declaration, so
  a struct field or enum variant is labelled with the function before it.

## Appendix

### A1. Allowed modules, library code (not cfg(test)), file/mapping/advice/proc kinds

Kinds: fs = std::fs path or OpenOptions/File:: constructor; fsmeth = a file or path method; mmap = memmap2; libc = a listed libc function; procsys = a /proc or /sys string; Filetype/fileext = the File type or a std::os::unix trait named.

- `src/bytes.rs` `slice_at:249`: 283(mmap)
- `src/bytes.rs` `open:344`: 345(fs), 350(mmap)
- `src/bytes.rs` `from_map:354`: 354(mmap)
- `src/bytes.rs` `map:382`: 382(mmap)
- `src/bytes.rs` `resident_pages:422`: 434(libc)
- `src/bytes.rs` `os_page_size:449`: 451(libc)
- `src/bytes.rs` `advise_random:513`: 516(mmap)
- `src/bytes.rs` `advise_normal:521`: 524(mmap)
- `src/bytes.rs` `advise_dontneed_range:530`: 546(libc), 557(libc)
- `src/bytes.rs` `resident_at:561`: 572(libc), 577(libc)
- `src/bytes.rs` `advise_dontneed:580`: 590(libc)
- `src/bytes.rs` `advise_willneed:604`: 630(mmap)
- `src/crypt.rs` `encrypt:375`: 383(fs)
- `src/crypt.rs` `default:590`: 627(mmap)
- `src/crypt.rs` `new:654`: 661(mmap), 666(libc)
- `src/crypt.rs` `populate:723`: 732(libc)
- `src/db/blocks.rs` `resident_of:82`: 84(libc), 90(libc)
- `src/db/blocks.rs` `map:153`: 158(libc)
- `src/db/blocks.rs` `unmap:172`: 174(libc)
- `src/db/dirkey.rs` `refused:95`: 108(fileext)
- `src/db/dirkey.rs` `ask:217`: 217(fs), 217(Filetype), 222(fsmeth), 222(libc)
- `src/db/dirkey.rs` `policy:232`: 232(fs), 232(Filetype)
- `src/db/dirkey.rs` `add:259`: 259(fs), 259(Filetype)
- `src/db/dirkey.rs` `remove:279`: 279(fs), 279(Filetype)
- `src/db/dirkey.rs` `held:295`: 295(fs), 295(Filetype)
- `src/db/dirkey.rs` `state:333`: 334(fs)
- `src/db/dirkey.rs` `protect:346`: 347(fs)
- `src/db/dirkey.rs` `unlock:366`: 367(fs)
- `src/db/dirkey.rs` `filesystem_root:395`: 395(fs), 395(Filetype), 396(fs), 396(fileext), 397(fsmeth), 398(fsmeth), 401(fsmeth), 403(fs), 414(fs)
- `src/db/pager.rs` `None`: 82(fs), 82(Filetype)
- `src/db/pager.rs` `bump:710`: 720(mmap)
- `src/db/pager.rs` `open:732`: 733(fs), 736(mmap), 746(mmap)
- `src/db/pager.rs` `memfile:806`: 806(Filetype), 807(fileext), 810(libc), 815(fs)
- `src/db/pager.rs` `memfile:819`: 819(Filetype)
- `src/db/pager.rs` `make:827`: 829(Filetype)
- `src/db/pager.rs` `adopt:876`: 878(Filetype), 884(fs), 884(fileext), 885(fsmeth), 911(libc)
- `src/db/pager.rs` `sparse:919`: 922(fsmeth)
- `src/db/pager.rs` `fill:966`: 996(libc)
- `src/db/pager.rs` `free:1210`: 1238(libc)
- `src/db/pager.rs` `drop:1284`: 1297(libc)
- `src/db/pager.rs` `advise:1365`: 1368(mmap), 1370(mmap)
- `src/db/pager.rs` `resident:1380`: 1396(libc)
- `src/db/pager.rs` `forget:1435`: 1451(libc), 1457(fs), 1458(fileext), 1461(libc), 1462(fsmeth)
- `src/db/pager.rs` `willneed:1476`: 1491(libc)
- `src/db/pager.rs` `map_shared:1529`: 1529(Filetype), 1530(fileext), 1534(libc), 1539(fsmeth), 1552(libc)
- `src/db/pager.rs` `give_back:1585`: 1589(libc)
- `src/db/vault.rs` `None`: 148(fs), 148(Filetype)
- `src/db/vault.rs` `random:279`: 286(libc), 300(fs)
- `src/db/vault.rs` `key_id:397`: 398(fs)
- `src/db/vault.rs` `rewrap:417`: 418(fs)
- `src/db/vault.rs` `write_marker:435`: 443(fs), 445(fsmeth), 447(fs), 448(fs), 448(fsmeth)
- `src/db/vault.rs` `refuse_marked:453`: 454(fsmeth)
- `src/db/vault.rs` `disk_parse:521`: 551(Filetype)
- `src/db/vault.rs` `passes:562`: 612(Filetype), 627(Filetype)
- `src/db/vault.rs` `reopen:707`: 707(Filetype), 708(fileext), 709(fsmeth), 709(procsys), 710(fs), 719(fsmeth), 732(Filetype)
- `src/db/vault.rs` `write:738`: 742(fileext), 746(libc), 747(fsmeth)
- `src/db/vault.rs` `create:803`: 804(fs)
- `src/db/vault.rs` `open:820`: 823(fs), 850(fs)
- `src/db/vault.rs` `make:889`: 889(Filetype), 891(fsmeth)
- `src/db/vault.rs` `page_of:919`: 930(fsmeth)
- `src/db/vault.rs` `len:958`: 965(fsmeth), 977(fs)
- `src/db/vault.rs` `remove:981`: 992(fs), 1005(fs)
- `src/db/vault.rs` `map_written:1039`: 1039(Filetype)
- `src/db/vault.rs` `unmapped:1070`: 1070(Filetype)
- `src/db/vault.rs` `rewrite:1090`: 1090(Filetype)
- `src/db/vault.rs` `reopen_written:1113`: 1113(Filetype)
- `src/db/vault.rs` `names:1131`: 1133(fs)
- `src/db/vault.rs` `write_envelope:1157`: 1157(Filetype)
- `src/db/vault.rs` `write_envelope_at:1162`: 1162(Filetype), 1164(fsmeth), 1168(fs), 1177(mmap), 1182(fsmeth)
- `src/db/vault.rs` `keeps:1196`: 1196(Filetype), 1200(fsmeth)
- `src/db/vault.rs` `settled:1220`: 1224(Filetype), 1253(fsmeth)
- `src/db/vault.rs` `persist_with:1299`: 1352(fs), 1393(fs), 1393(fsmeth)
- `src/db/vault.rs` `persist_form:1425`: 1459(fs), 1463(fs), 1464(fs), 1479(fs)
- `src/db/vault.rs` `companion:1489`: 1505(fsmeth)
- `src/db/vault.rs` `adopt:1527`: 1557(fsmeth)
- `src/db/vault.rs` `sweep:1593`: 1596(fs), 1601(fs), 1618(fs)
- `src/db/vault.rs` `create:1863`: 1863(Filetype), 1866(fs), 1867(fs)
- `src/db/vault.rs` `create_rw:1874`: 1874(Filetype), 1877(fs)
- `src/db/vault.rs` `open:1892`: 1892(Filetype), 1895(fs), 1896(fs)
- `src/db/vault.rs` `open_write:1903`: 1903(Filetype), 1905(fs)
- `src/db/vault.rs` `open_append:1914`: 1914(Filetype), 1917(fs)
- `src/db/vault.rs` `open_slot:1935`: 1935(Filetype), 1938(fs)
- `src/db/vault.rs` `rename:1955`: 1958(fs), 1961(fs)
- `src/db/vault.rs` `open_mapped:1977`: 1977(Filetype), 1980(fs), 1981(fs)
- `src/db/vault.rs` `hard_link:1990`: 1993(fs), 1996(fs)
- `src/db/vault.rs` `remove_file:2009`: 2012(fs), 2013(fs)
- `src/db/vault.rs` `exists:2019`: 2022(fsmeth), 2023(fsmeth)
- `src/db/vault.rs` `is_file:2029`: 2032(fsmeth), 2033(fsmeth)
- `src/db/vault.rs` `len:2039`: 2042(fs), 2043(fs)
- `src/db/vault.rs` `names:2050`: 2056(fs)
- `src/db/vault.rs` `read:2065`: 2068(fs), 2069(fs)
- `src/db/vault.rs` `sync_dir:2083`: 2086(fs), 2086(fsmeth), 2087(fs), 2087(fsmeth)
- `src/db/vault.rs` `open_dir:2105`: 2105(Filetype), 2108(fs), 2109(fs)
- `src/db/vault.rs` `drop_cached:2162`: 2162(Filetype), 2165(fileext), 2169(libc), 2170(fsmeth), 2177(fsmeth), 2177(libc)

### A2. Allowed modules, cfg(test) code: sites per file

- `src/bytes.rs`: 2 lines
- `src/crypt.rs`: 1 lines
- `src/db/pager.rs`: 15 lines
- `src/db/vault.rs`: 33 lines

### A3. Other modules, library code, the File type and std::os::unix traits only (handles that came through the door)

False hits in this list: db.rs 35488, 35631, 35637 (`Hand::File`, an enum variant of the upkeep's hand-over).

- `src/db.rs` `None`: 31(Filetype)
- `src/db.rs` `write_ord:99`: 102(fsmeth)
- `src/db.rs` `write_ord_unsynced:117`: 117(Filetype)
- `src/db.rs` `write_ord_tmp:125`: 125(Filetype)
- `src/db.rs` `default:3954`: 4202(Filetype)
- `src/db.rs` `prefill:4435`: 4443(fsmeth)
- `src/db.rs` `open_append:4522`: 4524(fsmeth)
- `src/db.rs` `sync:4686`: 4687(fsmeth)
- `src/db.rs` `put_back:4706`: 4718(fsmeth)
- `src/db.rs` `replay:4846`: 4857(fsmeth)
- `src/db.rs` `seg_order:5759`: 5811(Filetype)
- `src/db.rs` `new:5913`: 5913(Filetype)
- `src/db.rs` `get_ref:6195`: 6195(Filetype)
- `src/db.rs` `into_inner:6207`: 6207(Filetype)
- `src/db.rs` `read_section:6234`: 6235(fileext), 6238(fsmeth)
- `src/db.rs` `write_buf_with:6497`: 6521(fileext), 6526(fsmeth), 6527(fsmeth), 6529(fsmeth)
- `src/db.rs` `pwritev_all:6558`: 6558(Filetype)
- `src/db.rs` `pwritev_at:6567`: 6567(Filetype), 6568(fileext), 6569(fsmeth)
- `src/db.rs` `run_write_fault:6629`: 6629(Filetype), 6630(fileext), 6656(fsmeth), 6674(fsmeth)
- `src/db.rs` `start_writeback:6689`: 6689(Filetype), 6692(fileext), 6697(fsmeth)
- `src/db.rs` `spawn:6766`: 6767(Filetype), 6775(fsmeth)
- `src/db.rs` `run:6801`: 6801(Filetype), 6802(fileext), 6817(fsmeth), 6842(fsmeth)
- `src/db.rs` `sync_with:7593`: 7617(fsmeth)
- `src/db.rs` `flush_block:8079`: 8142(fsmeth)
- `src/db.rs` `finish:8228`: 8229(fsmeth)
- `src/db.rs` `finish_unsynced:8238`: 8238(Filetype)
- `src/db.rs` `finish_unsynced_from:8248`: 8253(Filetype)
- `src/db.rs` `finish_unsynced_with:8261`: 8265(Filetype), 8449(fileext), 8451(fsmeth), 8469(fsmeth), 8476(fsmeth), 8477(fsmeth), 8483(fsmeth), 8484(fsmeth), 8493(fsmeth), 8494(fsmeth), 8547(fsmeth)
- `src/db.rs` `file_clone:8773`: 8773(Filetype), 8774(fsmeth)
- `src/db.rs` `finish_unsynced:8883`: 8883(Filetype)
- `src/db.rs` `finish_unsynced_from:8892`: 8895(Filetype)
- `src/db.rs` `manifest_write_slot:12498`: 12507(fsmeth)
- `src/db.rs` `manifest_write_slot_unsynced:12516`: 12524(Filetype), 12525(fileext), 12529(fsmeth), 12532(fsmeth)
- `src/db.rs` `manifest_write:12581`: 12593(fsmeth)
- `src/db.rs` `manifest_read_all:12709`: 12715(fsmeth)
- `src/db.rs` `compact_run:13438`: 13581(Filetype)
- `src/db.rs` `finish_steps_on:13955`: 13969(Filetype)
- `src/db.rs` `fadvise_dontneed_range:15764`: 15764(Filetype), 15765(fileext), 15771(fsmeth)
- `src/db.rs` `fadvise_dontneed_range:15775`: 15775(Filetype)
- `src/db.rs` `fadvise_dontneed:15781`: 15781(Filetype), 15782(fileext), 15786(fsmeth)
- `src/db.rs` `fadvise_dontneed:15791`: 15791(Filetype)
- `src/db.rs` `with_tables:17085`: 17253(Filetype)
- `src/db.rs` `sync_wal:28037`: 28056(fsmeth)
- `src/db.rs` `open:28680`: 28769(fileext), 28772(fsmeth), 28775(fsmeth), 29100(fsmeth), 29101(fsmeth)
- `src/db.rs` `close_run:30333`: 30425(fsmeth), 30428(fsmeth), 30460(fsmeth), 30498(fsmeth)
- `src/db.rs` `seal_table:30583`: 30829(Filetype), 30970(fsmeth), 30973(fsmeth)
- `src/db.rs` `injected_presync:32772`: 32772(Filetype), 32773(fileext), 32774(fsmeth)
- `src/db.rs` `hand_over_upkeep:35415`: 35488(Filetype)
- `src/db.rs` `finish_hand:35630`: 35631(Filetype), 35637(Filetype)
- `src/db.rs` `stands_in:38491`: 38511(Filetype)
- `src/db.rs` `spawn:38517`: 38518(Filetype), 38527(fsmeth)
- `src/db.rs` `start:38544`: 38544(Filetype)
- `src/db.rs` `drop:38560`: 38645(Filetype)
- `src/db.rs` `write_slot:40839`: 40845(Filetype)
- `src/db.rs` `sync_slot:40872`: 40872(Filetype), 40881(fsmeth)
- `src/db/zfile.rs` `None`: 47(Filetype)
- `src/db/zfile.rs` `popcount:1841`: 1869(Filetype)
- `src/db/zfile.rs` `put:3825`: 3836(Filetype)
- `src/db/zfile.rs` `new:3842`: 3842(Filetype)
- `src/db/zfile.rs` `drop_written:3917`: 3917(Filetype)
- `src/db/zfile.rs` `drop_written_range:3926`: 3926(Filetype), 3927(fileext), 3933(fsmeth), 3940(fsmeth)
- `src/db/zfile.rs` `drop_written_range:3945`: 3945(Filetype)
- `src/db/zfile.rs` `swap_ahead:3957`: 3957(Filetype), 3958(fileext), 3966(fsmeth), 3975(fsmeth), 3980(fsmeth)
- `src/db/zfile.rs` `swap_ahead:3992`: 3992(Filetype)
- `src/db/zfile.rs` `drop_behind:4002`: 4002(Filetype), 4003(fileext), 4015(fsmeth)
- `src/db/zfile.rs` `drop_behind:4024`: 4024(Filetype)
- `src/db/zfile.rs` `build:4184`: 4198(Filetype)
- `src/db/zfile.rs` `open_seg:4203`: 4207(Filetype)
- `src/db/zfile.rs` `build_file_seen:4242`: 4262(Filetype), 4380(fsmeth)
- `src/db/zfile.rs` `asked_bytes:4429`: 4437(Filetype)
