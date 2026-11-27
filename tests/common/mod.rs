//! What the integration tests share.
//!
//! Every test binary that names this module compiles the whole of it and
//! uses a part, so each item is dead in some binary; the allowance is for
//! that and nothing else.
#![allow(dead_code)]

/// A scan's callback as a trait object. `scan` and `read_all` are generic
/// over the callback, so every closure handed to one compiles its own copy
/// of the engine's whole read path into the test binary: `tests/db.rs` held
/// a thousand copies of `read_exts` and as many of `values_at`, and its
/// compile was nine tenths of the test group's. A `&mut dyn FnMut` is one
/// type, so every call site that hands one over shares one copy.
pub fn kv(f: &mut dyn FnMut(&[u8], &[u8])) -> &mut dyn FnMut(&[u8], &[u8]) {
    f
}

/// A point read's callback as a trait object, as `kv` is a scan's.
pub fn vf(f: &mut dyn FnMut(&[u8])) -> &mut dyn FnMut(&[u8]) {
    f
}

/// The switch that runs the tests which hold a store's answers whatever form
/// serves it with the stride form forced: `SUPDB_TEST_STRIDE=1`, read here
/// and nowhere else. Off, the default, it changes nothing and the gate runs
/// the tests as they are written. The stride form is asked for only by a
/// build of a form file, and those run only where the compact form is
/// forced, so a switch that set `ZStride::Forced` alone would leave every
/// test that names no form in the controller's FITS, raw, with the stride
/// code never reached; so where a test forces the compact form the switch
/// adds the stride form beside it (`with_stride`), and where a test names
/// no form it starts from the compact form forced as well (`options`).
pub fn stride_switch() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SUPDB_TEST_STRIDE").is_ok_and(|v| v == "1"))
}

/// `o` with the form a build of a form file asks for named: the stride form
/// under the switch (`stride_switch`), and the block form without it,
/// whichever the engine ships. For a test that forces the compact form
/// itself, at the site where it does: such a test is written against the
/// block form's files, counts and codings, and the switch is what runs it
/// over the stride form.
pub fn with_stride(mut o: supdb::Options) -> supdb::Options {
    o.zpages_stride = if stride_switch() {
        supdb::ZStride::Forced
    } else {
        supdb::ZStride::Off
    };
    o
}

/// The options a test builds on in place of `Options::default()`: the
/// default, and under the switch (`stride_switch`) the compact form forced
/// (`ZPages::Always`) with its floor lowered, since the floor declines every
/// partition a test can afford to write, and the stride form asked for. A
/// test that names a form keeps it: the switch moves what a test builds on
/// and nothing it says, so a field a test sets is the test's.
pub fn options() -> supdb::Options {
    let o = supdb::Options {
        jobs_mapped: !jobs_heap_switch(),
        one_file: one_file_switch(),
        ..supdb::Options::default()
    };
    if !stride_switch() {
        return o;
    }
    with_stride(supdb::Options {
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        ..o
    })
}

/// Whether the tests' stores are kept in one file (`Options::one_file`):
/// `SUPDB_TEST_ONEFILE=1`, read once. The option is off in the engine, so
/// a run of the tests under the switch is the one run that takes every
/// seal, merge, form build and open of theirs through the store's data
/// file. A test that is about the files a store of files keeps in its
/// directory -- their names, their bytes, a link to one -- names that
/// store (`files`).
pub fn one_file_switch() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SUPDB_TEST_ONEFILE").is_ok_and(|v| v == "1"))
}

/// `o` as a store of files, whatever the switch says: for a test of the
/// directory's own files.
#[allow(dead_code)]
pub fn files(o: supdb::Options) -> supdb::Options {
    supdb::Options {
        one_file: false,
        ..o
    }
}

/// Whether the tests' stores take their jobs' large arrays from the
/// allocator (`Options::jobs_mapped` off): `SUPDB_TEST_JOBSHEAP=1`, read
/// once. The option is on in the engine, so a run of the tests under the
/// switch is the one run that takes every seal, merge and close of theirs
/// through the allocator's buffers, the shape before and the arm's.
pub fn jobs_heap_switch() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SUPDB_TEST_JOBSHEAP").is_ok_and(|v| v == "1"))
}

/// A segment file without the part of it that is a function of when it was
/// written.
///
/// The superblock records `SystemTime::now()` in seconds as its third field,
/// and the FNV-1a over the fields that follows it covers that second, in each
/// of the two slots. So two writes of identical input differ in eighteen-odd
/// bytes whenever they straddle a tick, and comparing whole files is a test
/// that passes on a fast machine and fails on a slow one -- which is what it
/// did: green here, red on the macOS runner, on the arm of a loop where
/// nothing about the setting under test touches a byte.
///
/// Everything else is compared, including the superblock's offsets, which are
/// what a mis-applied setting would actually move. Only a segment has the
/// superblock; the caller hands this nothing else.
pub fn without_the_clock(bytes: &[u8]) -> Vec<u8> {
    const SLOT: usize = 512;
    const TS: usize = 16;
    const FNV: usize = 136;
    let mut out = bytes.to_vec();
    for slot in [0, SLOT] {
        for at in [slot + TS, slot + FNV] {
            out[at..at + 8].fill(0);
        }
    }
    out
}

/// The names of a store's files, wherever it keeps them: the directory's
/// own, and for a store in one file that is open there the names its
/// data file holds (`Db::one_names`). A store in one file that is closed
/// has only the directory's, which is its log, its slots and its runs: a
/// test that reads a closed store's segments by name is a test of a
/// store of files (`files`).
pub fn store_names(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.extend(supdb::Db::one_names(d));
    v.sort();
    v.dedup();
    v
}

/// The bytes of a store's file at `p`: the directory's own, or for a
/// store in one file that is open there, the file its data file keeps
/// under that name (`Db::one_read`). For a test that reads a file a store
/// wrote -- a form file's fence, a trailer -- and changes nothing of it.
pub fn store_file(p: impl AsRef<std::path::Path>) -> Vec<u8> {
    let p = p.as_ref();
    match std::fs::read(p) {
        Ok(b) => b,
        Err(e) => {
            let name = p.file_name().unwrap().to_string_lossy();
            supdb::Db::one_read(p.parent().unwrap(), &name)
                .unwrap_or_else(|| panic!("{}: {e}", p.display()))
        }
    }
}

/// Every file of the store at `d` as a path under it (`store_names`): the
/// directory's own and, for a store in one file that is open there, each
/// file its data file keeps, under the name the store has it by, which is
/// no file of the directory's. A test that goes on to ask something of
/// such a path asks it where the bytes lie (`store_place`, `store_len`,
/// `store_file`, `cached_pages`); one that opens, removes or changes a
/// file by its path is a test of a store of files (`files`), and fails on
/// the path here where a listing of the directory would have found
/// nothing and passed.
#[allow(dead_code)]
pub fn store_paths(d: impl AsRef<std::path::Path>) -> Vec<std::path::PathBuf> {
    let d = d.as_ref();
    store_names(d).into_iter().map(|n| d.join(n)).collect()
}

/// Where the bytes of a store's file at `p` lie: the file to open, and the
/// offset and the length in it. A file of the directory's own is itself,
/// from its start; a file of a store in one file that is open there is a
/// range of the store's data file (`Db::one_place`).
#[allow(dead_code)]
pub fn store_place(p: impl AsRef<std::path::Path>) -> (std::path::PathBuf, u64, u64) {
    let p = p.as_ref();
    match std::fs::metadata(p) {
        Ok(m) => (p.to_path_buf(), 0, m.len()),
        Err(e) => {
            let dir = p.parent().unwrap();
            let name = p.file_name().unwrap().to_string_lossy();
            let (at, len) =
                supdb::Db::one_place(dir, &name).unwrap_or_else(|| panic!("{}: {e}", p.display()));
            (dir.join("store.one"), at, len)
        }
    }
}

/// The length of a store's file at `p`, wherever it lies (`store_place`).
#[allow(dead_code)]
pub fn store_len(p: impl AsRef<std::path::Path>) -> u64 {
    store_place(p).2
}

/// The store's segments by name, in name order (`store_names`).
pub fn seg_names(d: &std::path::Path) -> Vec<String> {
    let mut v = store_names(d);
    v.retain(|n| n.ends_with(".sup"));
    v
}

/// The store's ordered indexes by name, in name order (`store_names`).
pub fn ord_names(d: &std::path::Path) -> Vec<String> {
    let mut v = store_names(d);
    v.retain(|n| n.starts_with("ord-"));
    v
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// The live partitions' fences, from their names: `(lo, hi)`, an open
/// fence empty, in key order.
pub fn partition_fences(d: &std::path::Path) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let mut v: Vec<(Vec<u8>, Option<Vec<u8>>)> = seg_names(d)
        .iter()
        .filter(|n| n.starts_with("par-"))
        .map(|n| {
            let stem = n.trim_end_matches(".sup");
            let f: Vec<&str> = stem.split('-').collect();
            let hi = if f[4].is_empty() {
                None
            } else {
                Some(unhex(f[4]))
            };
            (unhex(f[3]), hi)
        })
        .collect();
    v.sort();
    v
}

/// The mappings this process holds of form files under `dir`, as
/// `/proc/self/maps` names them: a renamed file shows under its new name,
/// an unlinked one with ` (deleted)`. One line a mapping, deduplicated by
/// the file the line names, since a mapping may be split into several.
/// The name is the whole of the line after its five fixed fields, so a
/// path with a space in it -- a `TMPDIR` with one -- and the ` (deleted)`
/// suffix are kept; the sixth token alone was neither, and an unlinked
/// file read as the live one under the same name.
#[cfg(target_os = "linux")]
pub fn zpg_mappings(dir: &std::path::Path) -> Vec<String> {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let prefix = dir.join("zpg-");
    let prefix = prefix.to_string_lossy();
    let mut v: Vec<String> = maps
        .lines()
        .filter_map(maps_path)
        .filter(|p| p.starts_with(&*prefix))
        .map(str::to_owned)
        .collect();
    v.sort();
    v.dedup();
    v
}

/// `zpg_mappings` with the inode each mapping is of, read from its line's
/// fifth field: a mapped file renamed to a dead name and unlinked is still
/// the inode it was under its live name, which is how a test names the
/// files it began with among every retired one a pin holds.
#[cfg(target_os = "linux")]
pub fn zpg_mapped_inodes(dir: &std::path::Path) -> Vec<(u64, String)> {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let prefix = dir.join("zpg-");
    let prefix = prefix.to_string_lossy();
    let mut v: Vec<(u64, String)> = maps
        .lines()
        .filter_map(|line| {
            let path = maps_path(line)?;
            let ino = line.split_whitespace().nth(4)?.parse().ok()?;
            path.starts_with(&*prefix).then(|| (ino, path.to_owned()))
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The path a `/proc/self/maps` line names: everything after its address,
/// permissions, offset, device and inode, the padding before it trimmed;
/// none for an anonymous mapping.
#[cfg(target_os = "linux")]
fn maps_path(line: &str) -> Option<&str> {
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        rest = &rest[rest.find(char::is_whitespace)?..];
    }
    Some(rest.trim_start()).filter(|p| !p.is_empty())
}

/// A dead arena's shape: a thread takes `bytes` in pieces of a megabyte,
/// touches every page, frees them all and exits, leaving the pages resident
/// in free chunks of its arena -- the shape the seal and build threads leave
/// in the suite. One allocation past the dynamic mmap threshold's start and
/// under its cap, freed first, raises the threshold to its size and the trim
/// threshold to twice it, so the pieces come from the thread's heap and
/// nothing glibc does at the frees gives the heap back. A keeper of two
/// megabytes allocated after the pieces is returned to the caller alive:
/// glibc carves from its bins before its top, so the keeper comes from the
/// top exactly when the last pieces did and lies above them, and the freed
/// pieces are a bin chunk a trim reaches, not the heap's top, which a trim
/// shrinks only past the trim threshold. The caller drops the keeper when
/// its reading is done.
pub fn dead_arena(bytes: usize) -> Vec<u8> {
    std::thread::spawn(move || {
        {
            let mut raise = vec![0u8; 24 << 20];
            raise[0] = 1;
            std::hint::black_box(&raise);
        }
        let mut held: Vec<Vec<u8>> = Vec::with_capacity(bytes >> 20);
        for _ in 0..bytes >> 20 {
            held.push(vec![1u8; 1 << 20]);
        }
        let mut keeper = vec![0u8; 2 << 20];
        keeper[0] = 1;
        std::hint::black_box(&held);
        keeper
    })
    .join()
    .unwrap()
}

/// Whether each live mapping of this process whose path contains `needle` is
/// advised `MADV_RANDOM`, as the kernel says it (`rr` in `VmFlags`): the
/// mapping is the only thing that can be asked, and a store's own record of
/// what it last asked of it is a claim the kernel does not have to share. A
/// mapping of a file that was unlinked (`(deleted)`) is a retired segment's,
/// held until the last reader that could have it leaves, and is nobody's to
/// advise any more.
#[cfg(target_os = "linux")]
pub fn mappings_advised_random(needle: &str) -> Vec<(String, bool)> {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    for line in smaps.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if let Some(flags) = line.strip_prefix("VmFlags:") {
            if let Some(p) = path.take() {
                out.push((p, flags.split_whitespace().any(|f| f == "rr")));
            }
        } else if first.contains('-') && first.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            // A mapping's first line: range, permissions, offset, device,
            // inode and the path, if it has one.
            let mut f = line.split_whitespace().skip(5);
            let deleted = line.ends_with("(deleted)");
            path = f
                .next()
                .filter(|p| p.contains(needle) && !deleted)
                .map(str::to_string);
        }
    }
    out
}

/// `mappings_advised_random` with each mapping's offset in its file: for a
/// store kept in one file, whose every file is a mapping of a range of the
/// data file and is told from the others by where it begins.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn mappings_at(needle: &str) -> Vec<(String, u64, bool)> {
    let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
    let mut out = Vec::new();
    let mut at: Option<(String, u64)> = None;
    for line in smaps.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if let Some(flags) = line.strip_prefix("VmFlags:") {
            if let Some((p, off)) = at.take() {
                out.push((p, off, flags.split_whitespace().any(|f| f == "rr")));
            }
        } else if first.contains('-') && first.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            let f: Vec<&str> = line.split_whitespace().collect();
            let deleted = line.ends_with("(deleted)");
            at = match (f.get(2), f.get(5)) {
                (Some(off), Some(p)) if p.contains(needle) && !deleted => {
                    Some((p.to_string(), u64::from_str_radix(off, 16).unwrap()))
                }
                _ => None,
            };
        }
    }
    out
}

/// How many pages of `[off, off + len)` of the store's file at `path` the
/// page cache holds, wherever the file lies (`store_place`): `mincore` over
/// a mapping of the file's own that touches nothing.
#[cfg(target_os = "linux")]
pub fn cached_pages(path: impl AsRef<std::path::Path>, off: usize, len: usize) -> usize {
    use std::os::unix::io::AsRawFd;
    let path = path.as_ref();
    let (file, at, _) = store_place(path);
    let off = off + at as usize;
    let f = std::fs::File::open(&file).unwrap();
    let size = f.metadata().unwrap().len() as usize;
    // SAFETY: a read-only shared mapping of the whole file, advised over a
    // range inside it, and unmapped before it returns; nothing reads it.
    unsafe {
        let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
        let base = libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            f.as_raw_fd(),
            0,
        );
        assert_ne!(base, libc::MAP_FAILED, "mapping {}", path.display());
        let (first, last) = (off / page, (off + len).div_ceil(page));
        let mut vec = vec![0u8; last - first];
        let r = libc::mincore(
            (base as usize + first * page) as *mut libc::c_void,
            (last - first) * page,
            vec.as_mut_ptr().cast(),
        );
        let err = std::io::Error::last_os_error();
        libc::munmap(base, size);
        assert_eq!(r, 0, "mincore: {err}");
        vec.iter().filter(|b| **b & 1 != 0).count()
    }
}

use ring::aead::{Aad, LessSafeKey, Nonce, Tag, UnboundKey, AES_256_GCM};

/// AES-256-GCM, `ring`'s, as the hook takes it: the whole of what a caller
/// writes.
pub struct Gcm(LessSafeKey);

impl Gcm {
    pub fn new(key: &[u8; 32]) -> Gcm {
        Gcm(LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key).expect("a 32-byte key"),
        ))
    }
}

impl supdb::crypt::Aead for Gcm {
    fn encrypt(
        &self,
        nonce: &[u8; supdb::crypt::NONCE],
        aad: &[u8],
        buf: &mut [u8],
    ) -> [u8; supdb::crypt::TAG] {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        let tag = self
            .0
            .seal_in_place_separate_tag(nonce, Aad::from(aad), buf)
            .expect("a chunk is far under the cipher's limit");
        tag.as_ref().try_into().expect("a sixteen-byte tag")
    }
    fn decrypt(
        &self,
        nonce: &[u8; supdb::crypt::NONCE],
        aad: &[u8],
        buf: &mut [u8],
        tag: &[u8; supdb::crypt::TAG],
    ) -> bool {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        self.0
            .open_in_place_separate_tag(nonce, Aad::from(aad), Tag::from(*tag), buf, 0..)
            .is_ok()
    }
}

/// CRC32C (Castagnoli) resumed from `seed`, as the manifest's header
/// carries it over its sequence and body, for a slot a test writes by hand.
pub fn crc32c_from(seed: u32, data: &[u8]) -> u32 {
    let mut crc = !seed;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A plain version-3 manifest's covered sequence and names as the
/// version-2 manifest of `seq` a writer before the pending flags would have
/// written: the 25-byte header (magic, sequence, body length, CRC over the
/// sequence and the body), then the covered sequence, a count, and per name
/// a u16 length and the bytes.
pub fn manifest_v2_of(v3: &[u8], seq: u64) -> Vec<u8> {
    assert_eq!(&v3[..9], b"SUPDBMAN\x03", "a plain version-3 manifest");
    let body3 = &v3[25..];
    let n = u32::from_le_bytes(body3[8..12].try_into().unwrap()) as usize;
    let mut body = Vec::new();
    body.extend_from_slice(&body3[..8]);
    body.extend_from_slice(&(n as u32).to_le_bytes());
    let mut p = 12;
    for _ in 0..n {
        let l = u16::from_le_bytes(body3[p..p + 2].try_into().unwrap()) as usize;
        body.extend_from_slice(&body3[p..p + 2 + l]);
        // The version-3 flag byte after the name has no version-2 place.
        p += 2 + l + 1;
    }
    let crc = crc32c_from(crc32c_from(0, &seq.to_le_bytes()), &body);
    let mut v2 = Vec::new();
    v2.extend_from_slice(b"SUPDBMAN\x02");
    v2.extend_from_slice(&seq.to_le_bytes());
    v2.extend_from_slice(&(body.len() as u32).to_le_bytes());
    v2.extend_from_slice(&crc.to_le_bytes());
    v2.extend_from_slice(&body);
    v2
}

/// Whether `name` in `d` is a run's temp file: `direct-*.tmp` with
/// something in it. An empty one is the spare a store makes ahead of its
/// next run (`Options::run_spare`), which the open removes and no run
/// has written.
#[allow(dead_code)]
pub fn is_run_file(d: &std::path::Path, name: &str) -> bool {
    name.starts_with("direct-")
        && name.ends_with(".tmp")
        && std::fs::metadata(d.join(name)).is_ok_and(|m| m.len() > 0)
}

/// A watchdog's exit: the message on the process's own stderr, past the
/// harness's capture, then the exit. The harness captures what a test's
/// threads print, a watchdog's thread among them, so a watchdog that
/// printed through `eprintln!` and exited left the whole binary ending with
/// code 101 after a hundred and sixty-six tests, no failure named and no
/// summary -- the dev binary under the whole file's load, where a window
/// a minute holds in the checked profile runs several times longer.
pub fn watchdog_exit(what: &str) -> ! {
    use std::io::Write;
    let said = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/stderr")
        .and_then(|mut e| writeln!(e, "watchdog: {what}"));
    if said.is_err() {
        eprintln!("watchdog: {what}");
    }
    std::process::exit(101);
}

/// Exits the process unless dropped within a minute: for a test whose
/// failure is a wait that never returns, so it reports itself rather
/// than hanging. Dropped as the waits end, or by the unwind of a failure
/// in them, so an assertion reports itself and not the watchdog.
pub struct Watchdog {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    watch: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    pub fn arm(what: &'static str) -> Watchdog {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = {
            let done = done.clone();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    if t.elapsed() > std::time::Duration::from_secs(60) {
                        watchdog_exit(what);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })
        };
        Watchdog {
            done,
            watch: Some(watch),
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(w) = self.watch.take() {
            let _ = w.join();
        }
    }
}
