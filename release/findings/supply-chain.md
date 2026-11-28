# Supply chain at C0

A read-only audit of what supdb takes from outside itself, at C0 = `b3a043dc` (#37). The working tree read was the coordination branch at `dd5dbbbc`, which differs from C0 under `release/` alone (`git diff --name-only b3a043dc HEAD`), so every file cited here is C0's. It builds on the history audit's G4, G5 and G6 (`release/history/secrets.md`) and does not restate them.

Nothing was built, fetched or changed in `/home/user/supdb`: no cargo, no network, git read-only. The proposed edits were made to copies in the scratchpad and checked there (YAML parse, `bash -n` of every `run:` block, `git apply --check` against C0's files). The tests that hold the workflows (`tests/source.rs`) were not run, since that needs cargo. No RustSec database is on disk (`~/.cargo/advisory-db` is absent; there is no `cargo-audit` or `cargo-deny`), so every advisory statement below is either read off the local registry index or named as from memory and to be confirmed by `cargo audit` before the tag.

    S=/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad
    R=~/.cargo/registry; IX=$R/index/index.crates.io-1949cf8c6b5b557f/.cache

## Findings

| id | sev | finding | where | proposed fix |
|---|---|---|---|---|
| SC1 | medium (high if RustSec lists 0.11.6) | `lz4_flex` is locked at 0.11.6, and the registry index on disk shows 0.11.7 published in one batch with 0.12.3, 0.13.2 and 0.14.1: four release lines patched together, the shape of a security release. 0.11.4, 0.11.5 and 0.12.0 are yanked, and 0.11.6 itself was a backport published after 0.12.1. The engine turns off `safe-decode` (`default-features = false`), so its decoder is the crate's `unsafe` one, and it decodes bytes a CRC32C has checked, which is no defence against a crafted file: every segment `Blob` opens, and in the browser every object a server sends. `Cargo.toml` asks for `0.11`, so a dependent may resolve any non-yanked 0.11.x. | `Cargo.toml:18`, `Cargo.lock` (`lz4_flex 0.11.6`), `src/block.rs:344,518` | Run `cargo audit` before the tag. Move the lock to the newest 0.11.x (`cargo update -p lz4_flex --precise 0.11.7`), and raise the requirement to it. If an advisory names the unsafe decoder, consider `safe-decode` for the wasm build, whose input is a server's. |
| SC2 | medium | `cargo publish` would ship 2,403 files, 23.2 MB (about 3.8 MB gzipped): the ten workflows, `.localmostrc` (the owner's runner sandbox), `CLAUDE.md`, `ROADMAP.md`, `scripts/`, 2,274 figure files (10.5 MB, 45% of the package), 37 test files of which `tests/source.rs` reads `.github/` and `scripts/` and cannot pass outside the repository, `experiments/ffi`, `web/`, and one session scratch path (`docs/zpages-design.md:2051`). There is no `include`, `exclude` or `publish` in `Cargo.toml` and no `.gitattributes`. A crates.io upload cannot be taken back, only yanked. | `Cargo.toml` | Until decision 3 is made, `publish = false`. For a publish, an `include` list: `/src/**/*.rs`, `/examples/*.rs`, `/Cargo.toml`, `/LICENSE`, `/README.md` is 28 files, 6.6 MB, 1.7 MB gzipped (cargo adds `Cargo.lock`, since there are examples). README images resolve against `repository` on crates.io, so leaving out `docs/figures` breaks none of them. Contents list at the end. |
| SC3 | medium (extends G5) | In `bank-row.yml` the job that needs the write token, `propose`, also builds and runs the suite: `bench/scripts/figures.sh` runs `cargo build --release --bin bench`, so every build script in `bench/Cargo.lock` (RocksDB's C++ through `librocksdb-sys`, bindgen loading libclang, `lmdb-master-sys`, `lz4-sys`, `ring`, `serde`'s proc macro) and FASTER's C++ run with `contents: write` and `pull-requests: write`, beside `Swatinem/rust-cache`. Job-level permissions and `persist-credentials: false` do not contain code that runs in the same job: steps on a hosted runner have passwordless sudo, and the token is in the runner process's memory, which is how the March 2025 tj-actions compromise read secrets. With the token, such code can push branches and tags, a release tag among them unless a ruleset stops it. | `.github/workflows/bank-row.yml:79-158` | Diff A narrows the token to `propose`. Diff B is the fix: `propose` builds with the read-only token and hands a patch to a new `push` job, which builds nothing and runs only checkout, download-artifact, git and gh. It applies only paths under `bench/runs/quick/` and `docs/figures/`. |
| SC4 | medium (extends G4, G6) | `quiet-bench.yml` grants `actions: write` at the top, so every job gets it: the owner's reusable workflow called at `@main`, and the `full` job, which builds the suite on the owner's machine. Only `pickup-watchdog` lists jobs and cancels. A token with `actions: write` can dispatch workflows, and a dispatch made with `GITHUB_TOKEN` does start a run, so a step in `full` can start `bank-row.yml`, whose jobs hold `contents: write`. On the machine itself, `.localmostrc` lets a job write all of `~/.cargo`. That covers the `cargo` and `rustc` proxies in `~/.cargo/bin`, `~/.cargo/config.toml` (a `[source]` replacement or a `rustc-wrapper`) and the extracted sources under `registry/src`, which cargo does not check again once unpacked. A build script that writes there persists into the owner's own builds and tests, and into a `cargo publish` made from that account. | `quiet-bench.yml:30-32,40`, `.localmostrc:68-69` | Diff A: `contents: read` at the top, `actions: write` on `pickup-watchdog` alone, `actions: read` on `check` (to be confirmed against localmost's `check.yaml`), and `persist-credentials: false` on `full`. Narrow the write path to `~/.cargo/registry` and `~/.cargo/git`, as G4 proposes. Publish from a fresh clone with a fresh `CARGO_HOME`. |
| SC5 | low | A foothold in any job that runs on main (push, dispatch or schedule) persists through the caches. Seven jobs (nine legs) in six workflows save and restore one `bench` rust-cache key holding target directories and `~/.cargo/registry/{index,cache}`, and every other job's key is saved from main too. A pull request's legs and the release tag's run fall back to what main saved. `comparators.yml` restores a `db_bench` binary from `actions/cache` and executes it. Pull requests from forks cannot write main's cache scope, so the foothold needs a compromised action or dependency first, which is what pinning (G6, diff A) narrows. | `shared-key: bench` at `ci.yml:141`, `bank-row.yml:59,103`, `apple-silicon.yml:56`, `cost.yml:54`, `comparators.yml:78`, `ooc-hosted.yml:63`; `comparators.yml:55-88` | Pin the actions (diff A). Optionally restore without saving in the jobs that run third-party build code with write tokens (bank-row's legs); with diff B none does. |
| SC6 | low | Dependents ignore the published `Cargo.lock`, and the requirement floors are older than what was tested: `lz4_flex = "0.11"`, `memmap2 = "0.9"`, `libc = "0.2"`. A downstream lockfile made earlier can keep any non-yanked version at or above the floor. | `Cargo.toml:18,29-30` | Raise the floors to the tested versions: `lz4_flex` 0.11.7 (after SC1's audit), `memmap2` 0.9.11, `libc` 0.2.189. |
| SC7 | low | Two dispatch inputs reach bash arithmetic, and bash evaluates a variable named inside `$(( ))` as an expression, whose array subscript runs a command. This was checked here under `set -eu`: a value such as `PATH[$(cmd)]` ran `cmd`. `ooc-hosted.yml` evaluates `bound_mib` before a `sudo systemd-run`, and `quiet-bench.yml` evaluates `pickup_timeout_minutes` in the job that holds `actions: write`. Only someone who can dispatch, who has write access already, reaches either. | `ooc-hosted.yml:86`, `quiet-bench.yml:55` | Diff C: reject anything but digits before the arithmetic. |
| SC8 | low | A pull request from a fork saves caches under its own merge ref, which `pr-caches.yml` does not delete (its guard skips forks, whose token cannot delete). They count against the repository's 10 GB for a week, and GitHub then evicts by last read, main's caches included. Approval of first-time contributors' runs is the only brake. | `pr-caches.yml:31`, every rust-cache step on `pull_request` | Accept it, or save caches from pull requests of this repository alone (`save-if` on the head repository); the latter needs a `CI_WITH` entry in `tests/source.rs`. |
| SC9 | info | `bench/Cargo.toml` has no `publish = false`, where `web/floor` and `experiments/ffi/capi` do. A publish from `bench/` would stop at the missing licence, but nothing says it is not meant to be published. | `bench/Cargo.toml:7-9` | Add `publish = false`. |
| SC10 | info | The library declares `crate-type = ["rlib", "cdylib"]` so that the wasm module can be built from it. As far as I know, cargo builds every crate type a dependency's lib declares, so each dependent's build also links a native cdylib of supdb. On native that cdylib exports nothing, since `wasmapi` is wasm-only. | `Cargo.toml:15` | Move the cdylib to a wrapper crate beside `web/floor`, or document it. |
| SC11 | info | FASTER is fetched by full commit id from an archived repository and checked against the pin (`bench/scripts/faster.sh:46-53`). Integrity holds: GitHub serves any object of the repository's fork network by id, so the id is the only identity, and it is the one checked. The repository may vanish, in which case the suite's build fails closed (`bench/build.rs:295-314`). | `bench/scripts/faster.sh`, `bench/vendor/faster/PIN` | None; keep the check. |
| SC12 | info | What the hosts provide is not pinned: apt packages at the image's versions, libclang loaded in-process by bindgen, the host's OpenSSL for `enc`, Node for `web/crypt.test.mjs`, and rustup's 1.100.0 by channel. `bench/.cargo/config.toml` builds the suite's C and C++ without stack protector, CET or fortify. None of it reaches the published crate, which has no build script of its own and whose only build-time third-party code is `libc`'s `build.rs`. | section 2 | None for the tag. |
| SC13 | info | The browser reader has no third-party JavaScript and imports only its own modules. It does trust whoever serves `supdb.wasm`: the module is fetched and instantiated with no integrity check. It trusts the object's server for plaintext objects, since a CRC is no MAC. It keeps decrypted plaintext in OPFS, and the origin can rewrite those pages. | section 4 | An optional SHA-256 of the module, checked before `WebAssembly.instantiate`; say the trust list in `web/README.md`. |

## 1. Rust dependencies

### What is locked, and what the disk can say about it

    # every registry package of both lockfiles against the cached crates.io index: checksum and yank
    # and every .crate archive on disk against the lockfile's checksum
    python3 -I $S/sc/check_locks.py      # 22 + 118 packages, crates.io only, all in the index cache, checksums equal, none yanked; 69 of 69 archives equal

- Both lockfiles take every package from crates.io; there is no git or path source beyond the repository's own crates, and no `[patch]`, `[replace]` or `[source]` in any manifest or in `bench/.cargo/config.toml`.
- All 140 registry packages, 22 in `Cargo.lock` and 118 in `bench/Cargo.lock`, are in the local index cache. Every lockfile checksum equals the index's, and none is yanked. The index was cached on 2026-11-27 at 15:00 and 17:43 ET.
- The 69 `.crate` archives on disk all hash to their lockfile checksums.

### The published crate

What a dependent links: `lz4_flex` and `twox-hash` on every target, and on native `memmap2` and `libc`. `memmap2`'s optional `stable_deref_trait` is not enabled. The crate has no `build.rs`; the one third-party build script a dependent runs is `libc`'s.

| dependency | requirement | locked | licence (from the cached manifest) | latest in the local index | maintenance | advisories |
|---|---|---|---|---|---|---|
| `lz4_flex` | `0.11`, no default features, `std`, `frame` | 0.11.6 | MIT | 0.14.2; the newest 0.11.x is 0.11.7 | active, with backports to old lines | SC1. 0.11.4, 0.11.5 and 0.12.0 are yanked. The README of the cached 0.11.6 adds a fuzz target that "asserts that the decompressor does not leak previous data when reusing an output buffer". To be confirmed by `cargo audit`. |
| `twox-hash` (via `frame`) | `2.0.0` (lz4_flex's) | 2.1.4 | MIT | 2.1.5 | active | none known to me |
| `memmap2` (native) | `0.9` | 0.9.11 | MIT OR Apache-2.0 | 0.9.12 | active | none known to me |
| `libc` (native) | `0.2` | 0.2.189 | MIT OR Apache-2.0 | 0.2.191 | rust-lang | none known to me |
| `ring` (dev only: tests and examples) | `0.17` | 0.17.14 | Apache-2.0 AND ISC (ships LICENSE-BoringSSL beside) | 0.17.14, the latest | no release in the index after 0.17.14 | RUSTSEC-2025-0009 (an AES panic with overflow checks on, which is the tests' `checked` profile) affects < 0.17.12; 0.17.14 is past it. RUSTSEC-2025-0007 ("unmaintained") was, as I recall, withdrawn when maintenance resumed. From memory. |

Transitive and dev-only, from the lockfile: `cc`, `shlex` 2.0.1 (RUSTSEC-2024-0006 affects < 1.3.0), `getrandom`, `untrusted`, `cfg-if`, `find-msvc-tools`, `wasi`, `windows-*`. All are MIT/Apache-2.0 or ISC (`untrusted`).

Licence compatibility: the crate is MIT (`LICENSE`, `Cargo.toml:9`), and everything it links is MIT or MIT OR Apache-2.0, with no copyleft. A dependent's obligation is the MIT notices of `lz4_flex` and `twox-hash`. A distributed `supdb.wasm` statically links those two and Rust's standard library (MIT OR Apache-2.0), so a release asset of the module should carry their notices. `ring` is a dev-dependency and reaches no dependent.

### The suite (`bench/`, not published)

| dependency | requirement | locked | licence | latest in the local index | note |
|---|---|---|---|---|---|
| `heed` (feature `lmdb`, default) | `0.20` | 0.20.5 | MIT; LMDB itself (via `lmdb-master-sys`, Apache-2.0 crate) is OpenLDAP Public License 2.8 | 0.23.1 | Meilisearch's |
| `rocksdb` | `0.23`, no defaults, `lz4`, `bindgen-runtime` | 0.23.0 (`librocksdb-sys` 0.17.3+10.4.2) | Apache-2.0; RocksDB is Apache-2.0 OR GPL-2.0 (`librocksdb-sys` says MIT/Apache-2.0/BSD-3-Clause) | 0.25.0 (0.19.0+11.8.1) | two lines behind; a comparator's version is part of what is measured |
| `lmdb-master3-sys` (feature `enc`) | `=0.2.6` | 0.2.6 | Apache-2.0 crate, OLDAP-2.8 code (from memory; not in the local source cache) | 0.2.6 | LMDB's unreleased 1.0 line, pinned on purpose |
| `librocksdb-sys` (feature `enc`) | `0.17` | 0.17.3+10.4.2 | MIT/Apache-2.0/BSD-3-Clause | 0.19.0+11.8.1 | named for its headers' path |
| `ring` | `0.17` | 0.17.14 | Apache-2.0 AND ISC | 0.17.14 | as above |
| `serde`, `serde_json` | `1` | 1.0.229, 1.0.151 | MIT OR Apache-2.0 | 1.0.229, 1.0.154 | |
| `memmap2`, `libc` | as the engine | as the engine | | | |
| `lz4` (feature `lz4-c`) | `1` | 1.28.1 | MIT; liblz4 BSD-2 | 1.28.1 | probes only |
| `zstd`, `snap` (feature `codecs`) | `0.13`, `1` | 0.13.4, 1.1.2 | MIT, BSD-3-Clause (from memory) | 0.14.5, 1.1.2 | probes only |
| `cc` (build) | `1` | 1.4.5 | MIT OR Apache-2.0 | 1.12.0 | |
| `lz4_flex` (dev) | as the engine | 0.11.6 | MIT | | SC1 applies here too |
| vendored `bench/vendor/lzav` | none (header 7.1) | | MIT, header kept | | probes only |
| fetched FASTER | `vendor/faster/PIN` `332ea7da` | | MIT (its LICENSE copied beside) | archived upstream | SC11 |

Not distributed: the bench binary links RocksDB, LMDB, FASTER, and with features jemalloc (BSD-2), OpenSSL's libcrypto (Apache-2.0), TBB (Apache-2.0), libaio (LGPL-2.1, dynamic) and libuuid (BSD-3). No obligation is triggered while the binary stays on CI and the owner's machines, and the artifacts are JSON rows and SVGs.

Advisories known to me against the suite's locked versions: none. `idna` 1.1.0 is past RUSTSEC-2024-0421 (< 1.0.0), and the one `rocksdb` crate advisory I recall affects releases before 0.19. From memory; `cargo audit` in `bench/` before the tag says it for certain.

### What `cargo publish` would ship

With no `include` and no `exclude`, cargo packages, in a git repository, every file not ignored by git. It always skips sub-packages (a directory with its own `Cargo.toml`: `bench/`, `experiments/ffi/capi/`, `web/floor/`) and `target/`, and it adds `Cargo.toml.orig`, `.cargo_vcs_info.json` and, since there are examples, `Cargo.lock`. Dotfiles that git tracks are included. At C0 that is:

    git ls-tree -r -l b3a043dc | awk '{print $4"\t"$5}' > $S/sc/c0-files.tsv        # 2,630 tracked, 92.9 MB
    # minus bench/, experiments/ffi/capi/, web/floor/ (227 files, 69.8 MB): $S/sc/package-list.tsv, 2,403 files, 23.2 MB
    git archive --format=tar b3a043dc -- . ':(exclude)bench' ':(exclude)experiments/ffi/capi' ':(exclude)web/floor' | gzip -9 | wc -c   # 3,813,181
    git archive --format=tar b3a043dc -- src examples Cargo.toml Cargo.lock LICENSE README.md | gzip -9 | wc -c                       # 1,679,484, the proposed include

The 10 MB crates.io limit is not reached either way. What should not go:

- The CI definitions and the owner's runner policy: `.github/workflows/*` (10 files) and `.localmostrc`.
- The repository's working notes and tooling: `CLAUDE.md`, `ROADMAP.md`, `CONTRIBUTING.md`, `scripts/` (29 files, with `scripts/raises.txt` and four stand-in macOS headers), `rust-toolchain.toml` and `.gitignore`.
- `docs/figures/` (2,274 files): 10.5 MB of the package's 23.2 MB. The SVGs reference only the SVG namespace and hold no script.
- Tests that are the repository's and not the crate's: `tests/source.rs` reads `.github/workflows`, `scripts/` and the cargo registry, and `tests/data/couplings.txt`, `cost-entries.txt` and `door*.txt` are the repository's lint lists.
- `experiments/ffi/` (7 files), `web/` (8 files) and `docs/*.md` (1.5 MB). The design docs carry one session scratch path (`docs/zpages-design.md:2051`, the P8 class) and two sentences about the session's `/home/user/supdb` (`docs/runs-form-design.md:53`, `docs/zpages-design.md:2427`). The history audit judged both benign; published, they cannot be taken back.

`release/` is not at C0. Decision 12 keeps it off main and the tag; if it ever lands, a publish would ship its 24 files and 0.9 MB, scratch paths included.

## 2. Build-time code

**The engine.** It has no `build.rs` and no proc-macro dependency, and the root has no `.cargo/config.toml`. Its tests and examples compile `ring`'s C and assembly through `cc`; on the macOS lint leg they do so against stand-in headers (`scripts/macos-headers`, never linked).

**The suite** (`bench/build.rs`):
- It runs `git` and `rustc -V` for the stamp.
- It writes and compiles `level.c`, a CPU-level check run as a constructor.
- On Linux x86-64 it compiles FASTER's C++ sources (`vendor/faster/src`, five files plus `shim.cc`) and links `aio`, `tbb`, `uuid`, `stdc++fs` and `pthread` from the host.
- With `enc` it compiles `shim/lmdb_enc.c` and `shim/rocks_enc.cc` against the RocksDB headers `librocksdb-sys` vendors, and links the host's `libcrypto`.
- With `codecs` it compiles `vendor/lzav/lzav.c`.
- It downloads nothing itself. FASTER's sources come from `bench/scripts/faster.sh`: a `git fetch --depth 1 https://github.com/microsoft/FASTER.git <pin>` into a temp repo, the fetched `HEAD` held to `vendor/faster/PIN` (`332ea7da532a688acf648ee60c4f63a1caeaf1f7`), `cc/src` and `LICENSE` copied into git-ignored paths, and the commit recorded in `vendor/faster/COMMIT`, which `build.rs` holds to the pin.

**Build scripts in the suite's graph.** These crates in `bench/Cargo.lock` have one: `bindgen`, `crossbeam-utils`, `libc`, `librocksdb-sys` (compiles RocksDB 10.4.2's C++, runs bindgen), `lmdb-master-sys`, `lz4-sys`, `proc-macro2`, `quote`, `ring`, `serde`, `serde_core`, `serde_json` and `zmij`. With features they add `lmdb-master3-sys`, `zstd-sys` and `tikv-jemalloc-sys`. `bzip2-sys` and `libz-sys` are locked but not enabled. The proc macros are `serde_derive`, and `phf_macros` (through `doxygen-rs`). Each runs with whatever the job's token is (SC3, SC4).

**libclang.** bindgen loads it in-process when `librocksdb-sys`'s build script runs (`bindgen-runtime`), from `LIBCLANG_PATH` or the standard directories. `bench/scripts/libclang.sh` trusts a `LIBCLANG_PATH` already in the environment, otherwise picks the highest `/usr/lib/llvm-*/lib` that has clang's headers, installs `libclang-dev` by apt where there is none, and on GitHub writes the directory to `$GITHUB_ENV`. On macOS it takes Xcode's (`xcode-select -p`).

**OpenSSL.** Only the `enc` feature needs it: `libssl-dev` from the image's apt, dynamically linked `libcrypto`. Nothing in the engine links OpenSSL.

**The C and C++ flags of `bench/.cargo/config.toml`.**
- x86-64-v3 with PCLMUL, or ARMv8 with CRC and crypto.
- Frame pointers as rustc keeps them.
- `-fno-stack-protector -fcf-protection=none -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -fno-semantic-interposition`, set through `[env]` as `CFLAGS_<target>` and `CXXFLAGS_<target>`, so every `cc` build in the suite's workspace takes them, the suite's copy of `ring` included.

Cargo reads the file from the directory a build starts in, so it applies to builds started in `bench/` and to nothing a dependent of the crate builds. The hardening is off in a benchmark binary that runs over generated data on CI and the owner's machines, which is a choice the file explains. It has no `[source]`, `[registries]`, `[net]` or `[build] rustc-wrapper`.

**Toolchain and packages.**
- `rust-toolchain.toml` pins channel 1.100.0 (`scripts/toolchain.sh` refuses another active rustc). rustup checks each component's hash against a manifest fetched over TLS.
- apt installs `--no-install-recommends` at the image's versions: `libclang-dev`, `libaio-dev`, `libtbb-dev`, `uuid-dev`, `libssl-dev`, `valgrind`, `cmake`, `libgflags-dev`, `gcc-aarch64-linux-gnu`, `libc6-dev-arm64-cross`, `qemu-user-static`, and `nodejs` in `scripts/deps.sh`.
- `bench/scripts/comparators.sh` builds `db_bench` with cmake from the RocksDB tree in the cargo registry, after a `cargo fetch`.

## 3. CI

### What each workflow can do

Today's token scope for the five files with no `permissions:` is the repository default, read-only by G6's log. No workflow names a secret or a variable, none uses `pull_request_target` or `workflow_run`, and no step downloads an artifact of another run.

| workflow | triggers | token (C0) | third-party code with the token | caches written | artifacts |
|---|---|---|---|---|---|
| `ci.yml` | push to main, any tag; `pull_request`; dispatch | default (read) | rust-cache; every crate build script of both lockfiles; FASTER | per job, `bench` shared by `quick` | `quick-row` |
| `apple-silicon.yml` | `pull_request` on 4 paths; dispatch | `contents: read` | rust-cache; the suite's build | `bench` | none |
| `bank-row.yml` | push to main; dispatch | `contents: write`, `pull-requests: write`, all jobs (G5) | rust-cache and the suite's build in all four legs, `propose` included (SC3) | `bench` | `row-<runner>`; pushes `bank/quick-<run>`, opens a pull request |
| `cost.yml` | dispatch | default (read) | rust-cache; the suite's build; valgrind | `bench` | none |
| `comparators.yml` | dispatch; Mondays 06:17 UTC (01:17 ET) | default (read) | rust-cache, `actions/cache`; the suite's and `db_bench`'s builds; runs a cached `db_bench` | `bench`, `comparators-tools-<key>` (an executable) | `comparators` |
| `ooc-hosted.yml` | dispatch | default (read) | rust-cache; the suite's build; `sudo systemd-run` of the bench binary | `bench` | `ooc-row` (with `ooc-stderr.log`) |
| `pr-caches.yml` | `pull_request: closed`, same-repo only | `actions: write` | none: `gh` only, no checkout | deletes the pull request's | none |
| `quiet-bench.yml` | dispatch | `actions: write`, `contents: read`, all jobs (SC4) | `bfulton/localmost` reusable workflow `@main`; on the owner's Mac the suite's build | none (the machine keeps `~/.cargo`) | `full-row` |
| `rocksdb-alloc.yml` | dispatch; Mondays 07:41 UTC (02:41 ET) | default (read) | rust-cache; two builds of the suite | own key, two targets | `rocksdb-alloc` |
| `runner-smoke.yml` | dispatch | `actions: read`, `contents: read` | `bfulton/localmost` reusable workflow `@main` | none | none |

**A pull request from a fork** triggers `ci.yml`, all seven jobs (nine legs), with its own version of the file, and `apple-silicon.yml` where it touches one of the four paths. It gets the read-only token GitHub gives every fork, and no secrets exist to give it. It saves caches under `refs/pull/<n>/merge`, which only that pull request restores (SC8), and uploads `quick-row` to its own run, which nothing else downloads. `pr-caches.yml` runs on its close and skips it. A fork's edited `ci.yml` can name any runner label, which is G4. Nothing else is reachable from a fork: every other workflow runs on main, by dispatch or on a schedule.

**The release tag.** Pushing it starts `ci.yml` on the tag (`push: tags: ['*']`), with today's read token. No workflow publishes anything, so the crate is published from wherever the owner runs `cargo publish` (SC4's last point).

### Proposed edits

Three diffs, made against C0's files with `diff -ruN`, each held to `git apply --check -p1`:
- **Diff A** is what was asked: `permissions: contents: read` at the top of every workflow, widened only on the jobs that need more; every action pinned; `persist-credentials: false` on every checkout whose token nothing later uses. It covers 14 of the 15 checkouts; the one left is `bank-row`'s `propose`, which pushes. It also edits `tests/source.rs`, which holds `ci.yml` and `apple-silicon.yml` to lists matched whole: `CI_ACTIONS`, `CI_WITH` (a new `persist-credentials` entry) and `CI_IF` name the pins; a `uses:` value drops a trailing `# vX` comment before it is matched; and the red cases that quote an action are updated. Without those, the `test` group goes red.
- **Diff B**, applied after A, is the fix for SC3.
- **Diff C**, applied after A, is SC7.

`pr-caches.yml` is unchanged: `actions: write` alone, with no checkout and no action, is already the least it needs. `bench/scripts/workflows.sh`'s five rules still hold, and the 79 `run:` blocks of the proposed files pass `bash -n`, with no `${{ }}` in any.

**The pins.** The SHAs cannot be looked up offline, so each is a placeholder written `<action>@<sha of vX> # vX`, 39 lines in all. It is always preceded by the action's name, so one substitution per action fills the workflows and `tests/source.rs` together. With the network:

    for a in actions/checkout:v7 Swatinem/rust-cache:v2 actions/upload-artifact:v7 actions/download-artifact:v8 actions/cache:v6; do
      git ls-remote "https://github.com/${a%%:*}" "refs/tags/${a##*:}" "refs/tags/${a##*:}^{}"   # take the ^{} line where the tag is annotated
    done
    git ls-remote https://github.com/bfulton/localmost refs/heads/main
    # then, per action (cache/restore and cache/save are one repository at one commit):
    sed -i 's|actions/checkout@<sha of v7>|actions/checkout@<40 hex>|g' .github/workflows/*.yml tests/source.rs

| action | uses | tag to resolve |
|---|---|---|
| `actions/checkout` | 15 | `v7` (better: the exact `v7.x.y` it points to) |
| `Swatinem/rust-cache` | 13 | `v2` (the one third-party action) |
| `actions/upload-artifact` | 6 | `v7` |
| `actions/download-artifact` | 1 | `v8` |
| `actions/cache/restore`, `actions/cache/save` | 1 + 1 | `v6` of `actions/cache` |
| `bfulton/localmost/.github/workflows/check.yaml` | 2 | `main`, which is a branch: pin its head on the day, and give the date in the comment |

After filling them: `sh scripts/check.sh lint test` (the `tests/source.rs` gates and `bench/scripts/workflows.sh`), and a dispatch of `bank-row.yml`, `quiet-bench.yml` and `runner-smoke.yml`, the three whose permissions change shape. A `.github/dependabot.yml` with the `github-actions` ecosystem keeps pins current; it is outside `.github/workflows`, so `tests/source.rs` does not read it.


## 4. The web reader

**What it loads.** `web/` is six plain ES modules, a README and a build script. There is no `package.json`, no npm dependency and no CDN. Every import is relative (`./supdb.mjs`, `./cache.mjs`, `./crypt.mjs`), and `crypt.test.mjs` adds `node:fs`, under Node in the `wasm` group. The only cryptography is the browser's WebCrypto (AES-GCM decrypt; HMAC and SHA-256 for SigV4). The only host written into the code is S3's default endpoint (`https://<bucket>.s3.<region>.amazonaws.com`, `s3.mjs:59`). Everything else comes from the caller:
- **The module, from `wasmUrl`.** `worker.mjs:28` fetches it and `WebAssembly.instantiate` instantiates it with no hash or integrity check. `supdb.wasm` is not in git: `web/build.sh` builds it (`.gitignore`), so whoever deploys the reader deploys the module.
- **The object, from `indexUrl`.** It is fetched whole into memory (`openMemory`), downloaded whole into OPFS (`fetchIntoOpfs`, `supdb.mjs:460`, with no size cap), or fetched by HTTP range into an OPFS page cache under a byte budget (`CachedBytes`, `cache.mjs`). The ranges come from `httpRangeFetcher`, or from `s3RangeFetcher` with credentials the caller passes.
- **The key of an encrypted object,** as raw bytes or a `CryptoKey`, posted to the worker.

**What a page that embeds it must trust.**
1. **The origin serving the `.mjs` files.** They run as the page's code: in the worker, with the origin's OPFS and its `fetch`, which sends cookies to same-origin URLs by default.
2. **Whoever serves `supdb.wasm`.** The module cannot run JavaScript, since its only imports are `supdb_host_len` and `supdb_host_read`. But every answer the reader gives is the module's, and the glue copies out whatever offsets and lengths it returns, bounded by the module's own memory. A module swapped on the server answers wrongly and says nothing. The proposed fix is an optional `wasmSha256` that `open` checks with `crypto.subtle.digest` before instantiating.
3. **The object's server, for a plaintext object.** Blocks carry CRC32C, which catches corruption and not a forger. A crafted object reaches the Rust parsers, `lz4_flex`'s unsafe decoder among them (SC1). That decoder stays inside the module's linear memory, so the cost of a defect is wrong answers or a trap, not the page.
4. **For an encrypted object,** every chunk is authenticated at its fetch. What is then kept in OPFS is plaintext (`crypt.mjs:29-32` says so) and is not checked again on a later load. Anything that runs in the same origin can read or rewrite it, and a cache resumed under a name that a re-rolled object reuses mixes two objects' pages; `s3.mjs` says to name the cache after the object's version.
5. **S3 credentials** live in the worker's closure for the fetcher's life and are never persisted (`s3.mjs:13-16`). Long-lived keys in a browser are the caller's choice; short-lived STS credentials, which the signer takes (`sessionToken`), are the shape to recommend.
6. **The page's CSP.** It has to allow `WebAssembly.instantiate` (`'wasm-unsafe-eval'` in the worker's `script-src`), the worker script itself, and `connect-src` to the module's and the object's hosts. The reader needs no cross-origin isolation: it uses no `SharedArrayBuffer`.

Two small things beside the trust list:
- `httpRangeFetcher` and `s3RangeFetcher` hold a 206's `Content-Range` start to the offset asked for and nothing more. A server that answers a shorter body is caught by `CachedBytes.ensure` (`cache.mjs:222-224`), which holds each answer to the length it asked for.
- `s3RangeFetcher` signs the URI-encoded key but fetches `${base}/${key}` raw (`s3.mjs:61,114`). A key with `?`, `#` or `%` fails to sign; it is not a security problem.

## Proposed diffs

### Diff A: permissions, pins, persisted credentials (apply to C0 from the repository root with `git apply -p1`)

```diff
diff -ruN a/.github/workflows/apple-silicon.yml b/.github/workflows/apple-silicon.yml
--- a/.github/workflows/apple-silicon.yml
+++ b/.github/workflows/apple-silicon.yml
@@ -35,7 +35,9 @@
     # minutes, and three times that on a slow runner.
     timeout-minutes: 60
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -50,7 +52,7 @@
       # `workspace -> target`, the target a path inside its workspace: see
       # the `test` job of ci.yml. The key is the one every job that builds
       # the suite's binary alone shares: see the `quick` job there.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
diff -ruN a/.github/workflows/bank-row.yml b/.github/workflows/bank-row.yml
--- a/.github/workflows/bank-row.yml
+++ b/.github/workflows/bank-row.yml
@@ -21,8 +21,7 @@
   workflow_dispatch:
 
 permissions:
-  contents: write
-  pull-requests: write
+  contents: read
 
 env:
   CARGO_TERM_COLOR: always
@@ -38,7 +37,9 @@
     # GitHub's six hours.
     timeout-minutes: 90
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -53,7 +54,7 @@
       # `workspace -> target`, the target a path inside its workspace: see
       # the `test` job of ci.yml. The key is the one every job that builds
       # the suite's binary alone shares: see the `quick` job there.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
@@ -69,7 +70,7 @@
           ./target/release/bench run --scale quick --out runs-ci
           ./target/release/bench gate runs-ci/quick/*.json --runs runs > runs-ci/gate.txt 2>&1 || true
           cat runs-ci/gate.txt
-      - uses: actions/upload-artifact@v7
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: always()
         with:
           name: row-${{ matrix.runner }}
@@ -79,10 +80,15 @@
   propose:
     needs: row
     if: always()
+    # The one job that writes: it pushes the branch and opens the pull
+    # request, so its checkout keeps the token for `git push`.
+    permissions:
+      contents: write
+      pull-requests: write
     runs-on: ubuntu-24.04
     timeout-minutes: 60
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -97,11 +103,11 @@
       # `workspace -> target`, the target a path inside its workspace: see
       # the `test` job of ci.yml. The key is the one every job that builds
       # the suite's binary alone shares: see the `quick` job there.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
-      - uses: actions/download-artifact@v8
+      - uses: actions/download-artifact@<sha of v8> # v8
         with:
           pattern: row-*
           path: rows
diff -ruN a/.github/workflows/ci.yml b/.github/workflows/ci.yml
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -23,6 +23,9 @@
         required: false
         default: ''
 
+permissions:
+  contents: read
+
 env:
   CARGO_TERM_COLOR: always
 
@@ -64,7 +67,9 @@
     # slower than usual at RocksDB's build.
     timeout-minutes: 120
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -88,7 +93,7 @@
       # script runs (`bindgen-runtime`, bench/Cargo.toml), so no cached
       # output names its directory. While the build script linked it, one
       # did, and a restored build whose directory had gone failed there.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: |
             . -> target
@@ -110,7 +115,9 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 90
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -135,7 +142,7 @@
       # that builds anything beside the binary keeps a key of its own, as
       # `rocksdb-alloc` does, or what it alone builds would never be
       # cached. `bench/scripts/workflows.sh` holds both.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
@@ -144,7 +151,7 @@
           ARMS: ${{ inputs.arms }}
           KEYS: ${{ inputs.keys }}
         run: sh scripts/check.sh quick
-      - uses: actions/upload-artifact@v7
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: always()
         with:
           name: quick-row
@@ -160,8 +167,9 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 15
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
         with:
+          persist-credentials: false
           fetch-depth: 0
       - name: The lists, against the base
         env:
@@ -174,12 +182,14 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 30
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
         run: sh scripts/toolchain.sh aarch64-unknown-linux-gnu
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
       # The target's C library is named: the compiler's package only
       # recommends it, and scripts/apt.sh installs no recommended extras.
       - name: Cross toolchain
@@ -204,7 +214,9 @@
     # from a cold cache: a test leg's RocksDB build twice over.
     timeout-minutes: 120
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -219,7 +231,7 @@
         run: sh scripts/apt.sh libssl-dev
       # The encrypted build's own target directory, under a key of its own:
       # nothing else builds it.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target-enc
       - name: The encrypted comparators, built, tested and run once
@@ -234,12 +246,14 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 45
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
         run: sh scripts/toolchain.sh
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
       - name: A filesystem that can encrypt
         run: sudo sh scripts/fscrypt.sh up /mnt/supdb-fscrypt "$(id -u):$(id -g)" >> "$GITHUB_ENV"
       - name: A store under a key
@@ -254,12 +268,14 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 15
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
         run: sh scripts/toolchain.sh
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
       # The group also runs web/crypt.test.mjs under the runner's own Node:
       # the browser's decrypter against the envelope the Rust tests hold the
       # writer to.
diff -ruN a/.github/workflows/comparators.yml b/.github/workflows/comparators.yml
--- a/.github/workflows/comparators.yml
+++ b/.github/workflows/comparators.yml
@@ -18,6 +18,9 @@
   schedule:
     - cron: '17 6 * * 1'
 
+permissions:
+  contents: read
+
 env:
   CARGO_TERM_COLOR: always
 
@@ -31,7 +34,9 @@
     runs-on: ${{ matrix.runner }}
     timeout-minutes: 300
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -52,7 +57,7 @@
       # may find it gone and build. Restored here and saved by a step of
       # its own after the table: the one action that does both saves only
       # where the job passed, and a cell under the margin fails the job.
-      - uses: actions/cache/restore@v6
+      - uses: actions/cache/restore@<sha of v6> # v6
         id: tools
         with:
           path: bench/comparators-tools
@@ -72,7 +77,7 @@
       # `workspace -> target`, the target a path inside its workspace: see
       # the `test` job of ci.yml. The key is the one every job that builds
       # the suite's binary alone shares: see the `quick` job there.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
@@ -81,12 +86,12 @@
           COMPARATORS_TOOLS: ${{ github.workspace }}/bench/comparators-tools
         run: sh bench/scripts/comparators.sh comparators-ci
       # The db_bench this run built, whatever the table said of the cells.
-      - uses: actions/cache/save@v6
+      - uses: actions/cache/save@<sha of v6> # v6
         if: always() && steps.tools.outputs.cache-hit != 'true' && hashFiles('bench/comparators-tools/db_bench-*') != ''
         with:
           path: bench/comparators-tools
           key: ${{ steps.tools.outputs.cache-primary-key }}
-      - uses: actions/upload-artifact@v7
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: always()
         with:
           name: comparators
diff -ruN a/.github/workflows/cost.yml b/.github/workflows/cost.yml
--- a/.github/workflows/cost.yml
+++ b/.github/workflows/cost.yml
@@ -20,6 +20,9 @@
         required: false
         default: ''
 
+permissions:
+  contents: read
+
 env:
   CARGO_TERM_COLOR: always
 
@@ -28,8 +31,9 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 150
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
         with:
+          persist-credentials: false
           fetch-depth: 0
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
@@ -48,7 +52,7 @@
       # shares (ci.yml's `quick` job): the probes build on the binary's
       # dependencies, RocksDB's among them, which the key restores, and
       # what they build beside it is the job's own each run.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
diff -ruN a/.github/workflows/ooc-hosted.yml b/.github/workflows/ooc-hosted.yml
--- a/.github/workflows/ooc-hosted.yml
+++ b/.github/workflows/ooc-hosted.yml
@@ -34,6 +34,9 @@
         required: false
         default: '1'
 
+permissions:
+  contents: read
+
 env:
   CARGO_TERM_COLOR: always
 
@@ -42,7 +45,9 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 360
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -57,7 +62,7 @@
       # `workspace -> target`, the target a path inside its workspace: see
       # the `test` job of ci.yml. The key is the one every job that builds
       # the suite's binary alone shares: see the `quick` job there.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: bench -> target
           shared-key: bench
@@ -107,7 +112,7 @@
       - name: The figures from the row
         working-directory: bench
         run: ./target/release/bench figures --runs runs-ci --out runs-ci/figures --scale ooc
-      - uses: actions/upload-artifact@v7
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: always()
         with:
           name: ooc-row
diff -ruN a/.github/workflows/quiet-bench.yml b/.github/workflows/quiet-bench.yml
--- a/.github/workflows/quiet-bench.yml
+++ b/.github/workflows/quiet-bench.yml
@@ -28,7 +28,6 @@
         default: '10'
 
 permissions:
-  actions: write
   contents: read
 
 concurrency:
@@ -37,12 +36,21 @@
 
 jobs:
   check:
-    uses: bfulton/localmost/.github/workflows/check.yaml@main
+    # What localmost's check reads; confirm against check.yaml at the
+    # pinned commit, which is another repository's.
+    permissions:
+      actions: read
+      contents: read
+    uses: bfulton/localmost/.github/workflows/check.yaml@<sha of main> # main, pinned on <date>
 
   pickup-watchdog:
     needs: [check]
     if: needs.check.outputs.runner == 'self-hosted'
     runs-on: ubuntu-24.04
+    # Lists the run's jobs and cancels the run: the one job that needs
+    # more than the tree. It checks out nothing and runs only `gh`.
+    permissions:
+      actions: write
     steps:
       - name: Wait for a self-hosted runner to claim the run
         env:
@@ -104,7 +112,9 @@
     # days; this is three, and the pickup watchdog still bounds the queue.
     timeout-minutes: 4320
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names where this machine's cargo
       # is rustup's, which reads the file whatever this step does; a cargo
       # from anywhere else never sees the file, and the row is that
@@ -148,7 +158,7 @@
       # the timeout or cancelled by hand still has the rungs that completed
       # on disk -- and this step being conditional on success is what threw
       # them away twice. `always()` covers cancellation too.
-      - uses: actions/upload-artifact@v7
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: always()
         with:
           name: full-row
diff -ruN a/.github/workflows/rocksdb-alloc.yml b/.github/workflows/rocksdb-alloc.yml
--- a/.github/workflows/rocksdb-alloc.yml
+++ b/.github/workflows/rocksdb-alloc.yml
@@ -14,6 +14,9 @@
   schedule:
     - cron: '41 7 * * 1'
 
+permissions:
+  contents: read
+
 env:
   CARGO_TERM_COLOR: always
 
@@ -22,7 +25,9 @@
     runs-on: ubuntu-24.04
     timeout-minutes: 240
     steps:
-      - uses: actions/checkout@v7
+      - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -42,7 +47,7 @@
       # since none of them builds the second directory. GitHub removes a
       # cache nothing has read for seven days and this job's schedule is
       # weekly, so a scheduled run may find its cache gone and build both.
-      - uses: Swatinem/rust-cache@v2
+      - uses: Swatinem/rust-cache@<sha of v2> # v2
         with:
           workspaces: |
             bench -> target
@@ -67,7 +72,7 @@
           set -eu
           python3 scripts/alloc_compare.py runs-glibc runs-jemalloc | tee alloc.md
           cat alloc.md >> "$GITHUB_STEP_SUMMARY"
-      - uses: actions/upload-artifact@v7
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: always()
         with:
           name: rocksdb-alloc
diff -ruN a/.github/workflows/runner-smoke.yml b/.github/workflows/runner-smoke.yml
--- a/.github/workflows/runner-smoke.yml
+++ b/.github/workflows/runner-smoke.yml
@@ -20,12 +20,16 @@
   workflow_dispatch:
 
 permissions:
-  actions: read
   contents: read
 
 jobs:
   check:
-    uses: bfulton/localmost/.github/workflows/check.yaml@main
+    # What localmost's check reads; confirm against check.yaml at the
+    # pinned commit, which is another repository's.
+    permissions:
+      actions: read
+      contents: read
+    uses: bfulton/localmost/.github/workflows/check.yaml@<sha of main> # main, pinned on <date>
 
   smoke:
     needs: check
diff -ruN a/tests/source.rs b/tests/source.rs
--- a/tests/source.rs
+++ b/tests/source.rs
@@ -4753,36 +4753,41 @@
 /// Actions a step may use, matched whole: each fetches, installs or
 /// publishes, and none checks.
 const CI_ACTIONS: &[(&str, &str)] = &[
-    ("actions/checkout@v7", "the tree"),
-    ("Swatinem/rust-cache@v2", "the build cache"),
-    ("actions/upload-artifact@v7", "publishes the quick row"),
+    ("actions/checkout@<sha of v7>", "the tree"),
+    ("Swatinem/rust-cache@<sha of v2>", "the build cache"),
+    ("actions/upload-artifact@<sha of v7>", "publishes the quick row"),
 ];
 
 /// Inputs an action may be given, matched by action and name: none
 /// chooses what is checked.
 const CI_WITH: &[(&str, &str, &str)] = &[
     (
-        "actions/checkout@v7",
+        "actions/checkout@<sha of v7>",
         "fetch-depth",
         "the whole history, which the base needs",
     ),
     (
-        "Swatinem/rust-cache@v2",
+        "actions/checkout@<sha of v7>",
+        "persist-credentials",
+        "the job token is not left in .git/config for the steps after the checkout",
+    ),
+    (
+        "Swatinem/rust-cache@<sha of v2>",
         "workspaces",
         "which target directories to cache",
     ),
     (
-        "Swatinem/rust-cache@v2",
+        "Swatinem/rust-cache@<sha of v2>",
         "shared-key",
         "the one key every job that builds the suite's binary alone restores",
     ),
-    ("actions/upload-artifact@v7", "name", "the artifact's name"),
+    ("actions/upload-artifact@<sha of v7>", "name", "the artifact's name"),
     (
-        "actions/upload-artifact@v7",
+        "actions/upload-artifact@<sha of v7>",
         "if-no-files-found",
         "a run that wrote no row publishes nothing",
     ),
-    ("actions/upload-artifact@v7", "path", "what is published"),
+    ("actions/upload-artifact@<sha of v7>", "path", "what is published"),
 ];
 
 /// The environment a workflow, a job or a step may set, matched whole,
@@ -4823,7 +4828,7 @@
 /// check runs.
 const CI_IF: &[(&str, &str, &str)] = &[(
     "always()",
-    "actions/upload-artifact@v7",
+    "actions/upload-artifact@<sha of v7>",
     "the quick row is published whether or not the run passed: it decides nothing",
 )];
 
@@ -5108,7 +5113,9 @@
                         true
                     }
                     "uses" => {
-                        st.uses = Some((ln, v.to_string()));
+                        // A pin's tag rides after it as a comment.
+                        let a = v.split_once(" #").map_or(v, |(a, _)| a).trim_end();
+                        st.uses = Some((ln, a.to_string()));
                         true
                     }
                     "run" => {
@@ -6050,7 +6057,7 @@
     let e = ci_check(
         &ci_edit(
             &yaml,
-            "uses: Swatinem/rust-cache@v2",
+            "uses: Swatinem/rust-cache@<sha of v2> # v2",
             "uses: actions-rs/clippy-check@v1",
         ),
         &sh,
@@ -6088,7 +6095,7 @@
         ("  wasm:\n", "  wasm:\n    env:\n      COST_BASE: HEAD\n", "the environment `COST_BASE: HEAD`"),
         ("  wasm:\n", "  wasm:\n    defaults:\n      run:\n        working-directory: bench\n", "the key `defaults`"),
         (lists, "        working-directory: bench\n        run: sh scripts/check.sh lists\n", "the key `working-directory`"),
-        ("        with:\n          fetch-depth: 0\n", "        with:\n          fetch-depth: 0\n          ref: HEAD~1\n", "the input `ref` of `actions/checkout@v7`"),
+        ("        with:\n          fetch-depth: 0\n", "        with:\n          fetch-depth: 0\n          ref: HEAD~1\n", "the input `ref` of `actions/checkout@<sha of v7>`"),
         (lists, "        uses: actions/checkout@v7\n        run: sh scripts/check.sh lists\n", "both a run and an action"),
         (cross, "      - <<: *anchor\n      - name: Build and test under emulation\n", "a step the test cannot read"),
         (cross, "      - name: x\n        <<: *anchor\n        run: cargo test\n      - name: Build and test under emulation\n", "a step the test cannot read"),
@@ -6098,7 +6105,7 @@
         // A name the test lists, with a value it does not.
         ("          SUPDB_PR_BASE: ${{ github.event.pull_request.base.sha }}\n", "          SUPDB_PR_BASE: HEAD~1\n", "the environment `SUPDB_PR_BASE: HEAD~1`"),
         // An input the test lists for another action.
-        ("        with:\n          fetch-depth: 0\n", "        with:\n          fetch-depth: 0\n          path: elsewhere\n", "the input `path` of `actions/checkout@v7`"),
+        ("        with:\n          fetch-depth: 0\n", "        with:\n          fetch-depth: 0\n          path: elsewhere\n", "the input `path` of `actions/checkout@<sha of v7>`"),
         // A list where the workflow has none, and a step's keys with no step.
         ("env:\n  CARGO_TERM_COLOR: always\n", "env:\n  - CARGO_TERM_COLOR: always\n", "a list outside a job's steps"),
         ("  wasm:\n", "  wasm:\n    steps:\n      name: x\n", "a key before any step"),
```

### Diff B: bank-row's build and its write in two jobs (SC3; apply after A)

```diff
--- a/.github/workflows/bank-row.yml
+++ b/.github/workflows/bank-row.yml
@@ -80,15 +80,18 @@
   propose:
     needs: row
     if: always()
-    # The one job that writes: it pushes the branch and opens the pull
-    # request, so its checkout keeps the token for `git push`.
-    permissions:
-      contents: write
-      pull-requests: write
     runs-on: ubuntu-24.04
     timeout-minutes: 60
+    # The rows and the figures, made with the workflow's read-only token:
+    # this job builds and runs the suite, every crate's build script with
+    # it, so it holds nothing that can write. What it made goes to `push`
+    # as a patch.
+    outputs:
+      rows: ${{ steps.rows.outputs.rows }}
     steps:
       - uses: actions/checkout@<sha of v7> # v7
+        with:
+          persist-credentials: false
       # The toolchain rust-toolchain.toml names, the one the local gate runs
       # under; no workflow names a version or a channel of its own.
       - name: The pinned toolchain
@@ -140,8 +143,34 @@
           fi
           sh bench/scripts/figures.sh
           sh bench/scripts/figures.sh --check
-      - name: Propose the rows as a pull request
+          mkdir -p bank
+          git add bench/runs/quick docs/figures
+          git diff --cached --binary > bank/rows.patch
+          cp body.md bank/body.md
+      - uses: actions/upload-artifact@<sha of v7> # v7
         if: steps.rows.outputs.rows != '0'
+        with:
+          name: bank
+          path: bank/
+          if-no-files-found: error
+
+  push:
+    needs: propose
+    if: needs.propose.outputs.rows != '0'
+    runs-on: ubuntu-24.04
+    timeout-minutes: 10
+    # The one job that writes, and it builds nothing: a checkout, the
+    # patch, git and gh. Its checkout keeps the token for `git push`.
+    permissions:
+      contents: write
+      pull-requests: write
+    steps:
+      - uses: actions/checkout@<sha of v7> # v7
+      - uses: actions/download-artifact@<sha of v8> # v8
+        with:
+          name: bank
+          path: bank
+      - name: Propose the rows as a pull request
         env:
           GH_TOKEN: ${{ github.token }}
           RUN_ID: ${{ github.run_id }}
@@ -152,13 +181,20 @@
           git config user.name "github-actions[bot]"
           git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
           git checkout -b "$branch"
-          git add bench/runs/quick docs/figures
+          # The patch was made by a job that ran the suite's build: it may
+          # touch the rows and the figures and nothing else of the tree.
+          git apply --index --include='bench/runs/quick/*' --include='docs/figures/*' bank/rows.patch
+          other=$(git diff --cached --name-only | grep -v -E '^(bench/runs/quick/|docs/figures/)' || true)
+          if [ -n "$other" ]; then
+            echo "the patch touches more than the rows and the figures: $other" >&2
+            exit 1
+          fi
           git commit -q -F - <<MSG
           Bank the hosted quick rows from run $RUN_ID
 
-          $(cat body.md)
+          $(cat bank/body.md)
           MSG
           git push -u origin "$branch"
           gh pr create --base main --head "$branch" \
             --title "Bank quick rows at $(git rev-parse --short "$SHA") from the hosted runners" \
-            --body-file body.md || echo "the pull request was not created; the rows are on branch $branch"
+            --body-file bank/body.md || echo "the pull request was not created; the rows are on branch $branch"
```

### Diff C: dispatch inputs held to digits before arithmetic (SC7; apply after A)

```diff
--- a/.github/workflows/ooc-hosted.yml
+++ b/.github/workflows/ooc-hosted.yml
@@ -88,6 +88,10 @@
           REPS: ${{ inputs.reps }}
         run: |
           set -eu
+          # Digits only: bash evaluates a variable named in $(( )) as an
+          # expression, and an array subscript in it runs a command.
+          case "$BOUND_MIB" in ''|*[!0-9]*) echo "bound_mib is whole MiB: '$BOUND_MIB'" >&2; exit 2 ;; esac
+          case "$REPS" in ''|*[!0-9]*) echo "reps is a count: '$REPS'" >&2; exit 2 ;; esac
           BOUND=$(( BOUND_MIB * 1048576 ))
           ARM_FLAG=""
           if [ -n "$ARMS" ] && [ "$ARMS" != all ]; then ARM_FLAG="--arms $ARMS"; fi
--- a/.github/workflows/quiet-bench.yml
+++ b/.github/workflows/quiet-bench.yml
@@ -60,6 +60,9 @@
           WINDOW: ${{ inputs.pickup_timeout_minutes || '10' }}
         run: |
           set -uo pipefail
+          # Digits only: bash evaluates a variable named in $(( )) as an
+          # expression, and an array subscript in it runs a command.
+          case "$WINDOW" in ''|*[!0-9]*) echo "pickup_timeout_minutes is whole minutes: '$WINDOW'" >&2; exit 2 ;; esac
           deadline=$(( WINDOW * 2 ))   # polls, at 30s each
           for i in $(seq 1 "$deadline"); do
             job=$(gh api "repos/$REPO/actions/runs/$RUN_ID/jobs" \
```

## Package contents: what `cargo publish` would ship at C0

Every file git tracks at `b3a043dc` outside `bench/`, `experiments/ffi/capi/` and `web/floor/` (each has its own `Cargo.toml`), with its size in bytes: 2,403 files, 23,186,111 bytes. Cargo adds `Cargo.toml.orig` and `.cargo_vcs_info.json` and normalizes `Cargo.toml`. The full list, one line a file, is `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/package-list.tsv`. The 129 files outside `docs/figures/`:

| bytes | file |
|---:|---|
| 2318 | `.github/workflows/apple-silicon.yml` |
| 6490 | `.github/workflows/bank-row.yml` |
| 12678 | `.github/workflows/ci.yml` |
| 4408 | `.github/workflows/comparators.yml` |
| 2451 | `.github/workflows/cost.yml` |
| 4996 | `.github/workflows/ooc-hosted.yml` |
| 2027 | `.github/workflows/pr-caches.yml` |
| 6406 | `.github/workflows/quiet-bench.yml` |
| 3269 | `.github/workflows/rocksdb-alloc.yml` |
| 1797 | `.github/workflows/runner-smoke.yml` |
| 480 | `.gitignore` |
| 2762 | `.localmostrc` |
| 24748 | `CLAUDE.md` |
| 2706 | `CONTRIBUTING.md` |
| 5066 | `Cargo.lock` |
| 3127 | `Cargo.toml` |
| 1070 | `LICENSE` |
| 18724 | `README.md` |
| 41173 | `ROADMAP.md` |
| 1033568 | `docs/engine.md` |
| 21550 | `docs/index-theory.md` |
| 151718 | `docs/runs-form-design.md` |
| 288821 | `docs/zpages-design.md` |
| 6510 | `examples/cipher_store.rs` |
| 4308 | `examples/crypt.rs` |
| 14940 | `examples/crypt_cost.rs` |
| 26 | `experiments/ffi/.gitignore` |
| 3760 | `experiments/ffi/README.md` |
| 3955 | `experiments/ffi/bench.py` |
| 19053 | `experiments/ffi/harness.c` |
| 5655 | `experiments/ffi/results-2026-09-29.md` |
| 1113 | `experiments/ffi/run.sh` |
| 4533 | `experiments/ffi/supdbpy.c` |
| 670 | `rust-toolchain.toml` |
| 2636 | `scripts/apt.sh` |
| 3210 | `scripts/base.sh` |
| 21068 | `scripts/check.sh` |
| 14365 | `scripts/cost-diff.py` |
| 7373 | `scripts/cost-hot.txt` |
| 28634 | `scripts/cost-red.sh` |
| 12231 | `scripts/cost.sh` |
| 4719 | `scripts/deps.sh` |
| 11450 | `scripts/door/clippy.toml` |
| 1927 | `scripts/door/lint.sh` |
| 7107 | `scripts/door/red.sh` |
| 2872 | `scripts/door/run.sh` |
| 1009 | `scripts/fmt.sh` |
| 2389 | `scripts/fscrypt.sh` |
| 5819 | `scripts/groupmem.py` |
| 414 | `scripts/macos-headers/TargetConditionals.h` |
| 216 | `scripts/macos-headers/assert.h` |
| 267 | `scripts/macos-headers/stdlib.h` |
| 409 | `scripts/macos-headers/string.h` |
| 36203 | `scripts/raises.txt` |
| 21025 | `scripts/ratchet-red.sh` |
| 13252 | `scripts/ratchet.py` |
| 1027 | `scripts/ratchet.sh` |
| 2140 | `scripts/resident.py` |
| 2472 | `scripts/seal-red-red.sh` |
| 8879 | `scripts/seal-red.sh` |
| 2087 | `scripts/signed.sh` |
| 4443 | `scripts/stallwatch.py` |
| 1607 | `scripts/toolchain.sh` |
| 156153 | `src/blob.rs` |
| 24262 | `src/block.rs` |
| 51190 | `src/bytes.rs` |
| 87511 | `src/crypt.rs` |
| 3433010 | `src/db.rs` |
| 87412 | `src/db/blocks.rs` |
| 18761 | `src/db/dirkey.rs` |
| 96925 | `src/db/onefile.rs` |
| 175658 | `src/db/pager.rs` |
| 211990 | `src/db/vault.rs` |
| 855709 | `src/db/zfile.rs` |
| 89571 | `src/db/zlz4.rs` |
| 476255 | `src/db/zpages.rs` |
| 378653 | `src/db/zselect.rs` |
| 162036 | `src/flatindex.rs` |
| 3004 | `src/format.rs` |
| 10973 | `src/index.rs` |
| 10799 | `src/lib.rs` |
| 136281 | `src/ordindex.rs` |
| 24215 | `src/reserve.rs` |
| 29605 | `src/wasmapi.rs` |
| 13346 | `tests/alloc_peaks.rs` |
| 74636 | `tests/blob.rs` |
| 16846 | `tests/checkpoint.rs` |
| 3484 | `tests/checksums_off.rs` |
| 120423 | `tests/cipher.rs` |
| 23893 | `tests/common/mod.rs` |
| 14672 | `tests/crypt.rs` |
| 52711 | `tests/data/cost-entries.txt` |
| 53317 | `tests/data/couplings.txt` |
| 5344 | `tests/data/crypt-v1.bin` |
| 3950 | `tests/data/door-libc.txt` |
| 6231 | `tests/data/door.txt` |
| 970174 | `tests/db.rs` |
| 24397 | `tests/dict.rs` |
| 15248 | `tests/dirkey.rs` |
| 635652 | `tests/durable.rs` |
| 73317 | `tests/fences.rs` |
| 9202 | `tests/forged.rs` |
| 155403 | `tests/fuzz.rs` |
| 11838 | `tests/jobs_mapped.rs` |
| 10684 | `tests/jobs_pooled.rs` |
| 12305 | `tests/kill.rs` |
| 28485 | `tests/leak.rs` |
| 28760 | `tests/lex/mod.rs` |
| 6249 | `tests/lock.rs` |
| 5572 | `tests/mapped.rs` |
| 6130 | `tests/memo_slab.rs` |
| 43651 | `tests/onefile.rs` |
| 18088 | `tests/ranges.rs` |
| 8082 | `tests/resident.rs` |
| 99848 | `tests/runlog.rs` |
| 65097 | `tests/segwriter.rs` |
| 149397 | `tests/settle.rs` |
| 61579 | `tests/share.rs` |
| 47647 | `tests/shared.rs` |
| 297202 | `tests/source.rs` |
| 960220 | `tests/zpages.rs` |
| 16718 | `web/README.md` |
| 3213 | `web/build.sh` |
| 11654 | `web/cache.mjs` |
| 9805 | `web/crypt.mjs` |
| 11996 | `web/crypt.test.mjs` |
| 4641 | `web/s3.mjs` |
| 24921 | `web/supdb.mjs` |
| 6017 | `web/worker.mjs` |

`docs/figures/`, 2,274 files (2,271 SVG and 3 HTML), 10,515,967 bytes, by directory:

| files | bytes | directory |
|---:|---:|---|
| 1 | 39015 | `docs/figures/full/` |
| 74 | 289486 | `docs/figures/full/latest/` |
| 74 | 289486 | `docs/figures/full/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor/` |
| 74 | 289879 | `docs/figures/full/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor_kids/` |
| 46 | 262795 | `docs/figures/full/x86_64_intel-r-xeon-r-processor-2.80ghz_4c_16g_hypervisor/` |
| 1 | 236987 | `docs/figures/ooc/` |
| 276 | 1145736 | `docs/figures/ooc/latest/` |
| 177 | 886874 | `docs/figures/ooc/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor_b1024m/` |
| 177 | 874121 | `docs/figures/ooc/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor_b1280m/` |
| 177 | 883507 | `docs/figures/ooc/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor_b1280m_kids/` |
| 141 | 582394 | `docs/figures/ooc/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor_b1536m/` |
| 280 | 1164583 | `docs/figures/ooc/x86_64_intel-r-xeon-r-processor-2.30ghz_4c_16g_hypervisor_b1024m/` |
| 276 | 1145736 | `docs/figures/ooc/x86_64_intel-r-xeon-r-processor-2.30ghz_4c_16g_hypervisor_b1280m/` |
| 1 | 76158 | `docs/figures/quick/` |
| 107 | 483227 | `docs/figures/quick/latest/` |
| 107 | 483031 | `docs/figures/quick/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor/` |
| 74 | 383003 | `docs/figures/quick/x86_64_intel-r-xeon-r-processor-2.10ghz_4c_16g_hypervisor_kids/` |
| 107 | 483227 | `docs/figures/quick/x86_64_intel-r-xeon-r-processor-2.30ghz_4c_16g_hypervisor/` |
| 52 | 257872 | `docs/figures/quick/x86_64_intel-r-xeon-r-processor-2.80ghz_4c_16g_hypervisor/` |
| 52 | 258850 | `docs/figures/quick/x86_64_intel-r-xeon-r-processor_4c_16g_hypervisor/` |

With the proposed `include` (`/src/**/*.rs`, `/examples/*.rs`, `/Cargo.toml`, `/LICENSE`, `/README.md`, and the `Cargo.lock` cargo adds): the 21 files of `src/`, the 3 of `examples/`, `Cargo.toml`, `Cargo.lock`, `LICENSE` and `README.md`; 28 files, 6,573,718 bytes, 1,679,484 gzipped.

## Scripts behind this file

- `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/check_locks.py`: lockfiles against the index cache and the archives.
- `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/edit_workflows.py`: makes diff A from copies of C0's files (`/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/proposed/a` to `b`).
- `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/edit_bankrow_b.py`: diff B.
- The three diffs as files: `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/workflows-A.diff`, `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/bankrow-B-over-A.diff`, `/tmp/claude-0/-home-user-supdb/45bff0df-8085-5da9-ab43-1e603cdf6933/scratchpad/sc/inputs-C-over-A.diff`.
