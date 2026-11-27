//! A store's segments in one file.
//!
//! A store in the clear keeps a file a segment, with an ordered index and
//! a form file beside each, and pays the filesystem for every one of them
//! at both ends of its life: a name made durable when the file lands, and
//! its blocks freed when it retires, which on a device that discards what
//! is freed is a round trip the size of the file, on whichever thread drops
//! the last mapping. This module keeps those files as extents of one file,
//! the store's data file, so that a segment's landing makes no name and its
//! retirement frees nothing: the space is written over by the next file.
//!
//! What it is to the engine is a directory. The engine goes on naming its
//! files as it did -- `seal-00000007.tmp`, renamed `pcs-...sup`, linked
//! under a partition's name, removed -- and the door (`vault`) sends each
//! call here where the store is kept this way and the name is one this
//! module holds. A name is a row of a table in memory; a rename, a link and
//! a removal are changes to the table and touch no file. The table reaches
//! the device in the store's manifest, which lists each named file's place
//! behind its own bytes (`image`, `adopt`): the manifest is what names the
//! store's files already, and here it also says where they are.
//!
//! The rules it keeps:
//!
//! - **A file is one extent.** A reader maps it as it maps a file of its
//!   own, at the extent's offset, and every reader of the format is the
//!   code it was. A writer is given room where it says how much it will
//!   write (`expect`) or at its first write, grows in place where the room
//!   after it is free, and is moved, its bytes copied, where it is not. A
//!   file that has been mapped is never moved. What a writer did not use
//!   goes back when its last handle closes.
//! - **A manifest names only bytes the device holds.** A file is listed
//!   once a sync has covered its last write: its own (`ExtFile::sync`), or
//!   any later sync of the data file for one that asked to ride it
//!   (`ExtFile::ride`). A file nothing has covered is left out of the
//!   manifest, and after a crash it is free space: for the files a store
//!   regenerates, that is their contract already.
//! - **Space is used again only when nothing can read it.** An extent
//!   goes back to the free list once no name, handle or mapping holds it,
//!   and, where a manifest named it, once a later manifest that does not
//!   is known durable (`durable`): until then an open after a crash could
//!   take the manifest that names it.
//! - **Nothing is freed.** The data file grows by reservation and never
//!   shrinks; a store that has reached its size writes into blocks it has
//!   written before.
//!
//! No cipher is in it and no format of a segment: it holds bytes at
//! offsets under names. A store under a cipher (`Encryption::Cipher`)
//! kept this way has a table of its own, its vault's (`vault::Vault::one`),
//! and the files in it are the store's envelopes, each under the name it
//! had in the directory: the rules above are kept for them as for a
//! segment, and nothing here knows which it holds.

// The door (scripts/door/clippy.toml, tests/data/door.txt): the files of a
// store kept in one, opened, written, synced and mapped here.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use crate::bytes::MmapBytes;
use std::collections::{BTreeMap, HashMap};
use std::fs::{File as OsFile, OpenOptions};
use std::io::{Error, ErrorKind, Result};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// The data file's name in the store's directory.
pub(crate) const NAME: &str = "store.one";

/// The head of the data file, which no extent is given: a page that says
/// what the file is, and room the stages after this one put the manifest's
/// slots in.
pub(crate) const HEAD: u64 = 1 << 20;
const MAGIC: &[u8; 8] = b"SUPDBONE";
/// The unit of space: a page.
const GRAIN: u64 = 4096;
/// The alignment of a file of a mebibyte and up: the segment writer's
/// piece, so a piece written at a piece's offset in its file is at one in
/// this file, and the page cache makes of it the folio it made
/// (`SegmentOptions::write_piece`).
const BIG: u64 = 1 << 20;
/// The least the data file is made longer by, and the least room a file
/// that outgrows what it was given is given more of: sixteen pages. A
/// mebibyte was both for a day, and a store of a few small segments, a
/// test's or a phone's, reserved two to five of them from the device for
/// a few hundred kilobytes: 771 of the tests' stores left 3.8 GB behind
/// one run of them, and the test binary that ran next died of a full
/// disk.
const STEP: u64 = 16 * GRAIN;
/// The most the data file is made longer by beyond what is asked for: a
/// gibibyte, the eighth of a store of eight (`One::room`).
const AHEAD_MOST: u64 = 1024 * BIG;
/// What a data file of `end` bytes is made longer by beyond what is asked
/// for: an eighth of it, a step at the least and `AHEAD_MOST` at the most.
fn ahead(end: u64) -> u64 {
    (end / 8).clamp(STEP, AHEAD_MOST)
}
/// The magic of the section a manifest carries behind its own bytes.
const SECTION: &[u8; 8] = b"SUPDBEXT";

/// A row of the table a manifest lists: a file's name, its offset in the
/// data file, its length, and the checksum of its first page.
pub(crate) type Row = (String, u64, u64, u32);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, msg.into())
}

fn up(n: u64, to: u64) -> u64 {
    n.div_ceil(to) * to
}

/// The alignment a file expected to hold `bytes` is placed at: the next
/// power of two at or over its size, a page at the least and `BIG` at the
/// most, so a small file's writes fall as they fall in a file of its own.
fn align_for(bytes: u64) -> u64 {
    bytes.next_power_of_two().clamp(GRAIN, BIG)
}

// ------------------------------------------------------------ the space --

/// The free ranges of the data file past its head, by offset, none
/// touching another.
#[derive(Default)]
struct Space {
    free: BTreeMap<u64, u64>,
}

impl Space {
    /// `len` bytes at a multiple of `align`, from the lowest free range
    /// that holds them: low offsets first, so a store that has reached its
    /// size keeps to the blocks it has written.
    fn take(&mut self, len: u64, align: u64) -> Option<u64> {
        let (off, at, n) = self.free.iter().find_map(|(&off, &n)| {
            let at = up(off, align);
            (at + len <= off + n).then_some((off, at, n))
        })?;
        self.free.remove(&off);
        if at > off {
            self.free.insert(off, at - off);
        }
        if at + len < off + n {
            self.free.insert(at + len, off + n - (at + len));
        }
        Some(at)
    }

    /// `[at, at + len)` where all of it is free: a file growing where it
    /// stands.
    fn take_at(&mut self, at: u64, len: u64) -> bool {
        let Some((&off, &n)) = self.free.range(..=at).next_back() else {
            return false;
        };
        if at + len > off + n {
            return false;
        }
        self.free.remove(&off);
        if at > off {
            self.free.insert(off, at - off);
        }
        if at + len < off + n {
            self.free.insert(at + len, off + n - (at + len));
        }
        true
    }

    /// The free bytes from `at` on, nothing where `at` is not free.
    fn run_at(&self, at: u64) -> u64 {
        match self.free.range(..=at).next_back() {
            Some((&off, &n)) if off + n > at => off + n - at,
            _ => 0,
        }
    }

    /// `[at, at + len)` free again, joined to the free ranges it touches.
    fn give(&mut self, mut at: u64, mut len: u64) {
        if len == 0 {
            return;
        }
        if let Some((&off, &n)) = self.free.range(..at).next_back() {
            debug_assert!(off + n <= at, "space given back twice");
            if off + n == at {
                self.free.remove(&off);
                at = off;
                len += n;
            }
        }
        if let Some(&n) = self.free.get(&(at + len)) {
            self.free.remove(&(at + len));
            len += n;
        }
        debug_assert!(
            self.free.range(at..at + len).next().is_none(),
            "space given back twice"
        );
        self.free.insert(at, len);
    }

    fn bytes(&self) -> u64 {
        self.free.values().sum()
    }
}

// ------------------------------------------------------------- one file --

/// Where a file stands in the data file.
#[derive(Clone, Copy)]
struct Loc {
    base: u64,
    /// The room it was given, a multiple of `GRAIN`.
    cap: u64,
}

/// What the table knows of a file's first page, for the checksum a
/// manifest lists the file under (`One::image`).
///
/// The checksum was first taken by reading the page back where the file
/// was first listed. A file dropped from the page cache as it is written
/// -- a form file built ahead of its need, kept out of the raw form's
/// cache -- was then read from the device for it, a page a file on the
/// thread every landing goes through, and the page stood in the cache
/// after, which the test that holds such a file to no page there read as
/// one. The writes that make the page are the table's to see, so it keeps
/// the page as they make it and reads nothing.
enum Head {
    /// Being written: the page's bytes as the writes made them, as far as
    /// a write has reached into it, zeroes where a gap was filled.
    Page(Vec<u8>),
    /// Listed: the checksum a manifest carries for it.
    Sum(u32),
    /// Written or cut since it was listed, or by a write that failed:
    /// read back from the data file where it is next listed.
    Unread,
}

/// A file kept as an extent: what its names, its handles and its mappings
/// share.
pub(crate) struct Ext {
    loc: RwLock<Loc>,
    /// Its bytes: the furthest a write has reached, or `set_len`'s answer.
    len: AtomicU64,
    /// The writes made to it, counted.
    wrote: AtomicU64,
    /// One more than the count of writes a sync has covered: zero for
    /// none, so a file never written and never synced is not durable.
    covered: AtomicU64,
    /// One more than the count of writes at its last ride, and the data
    /// file's syncs begun by then: a sync begun after that covers them.
    rode: Mutex<(u64, u64)>,
    /// The newest manifest sequence whose image listed it; zero for none.
    named: AtomicU64,
    /// The names it has.
    names: AtomicUsize,
    /// The handles and mappings on it.
    users: AtomicUsize,
    /// The handles opened to write it.
    writers: AtomicUsize,
    /// A mapping has been made of it: it is never moved again.
    mapped: AtomicBool,
    /// Its first page, for the checksum a manifest lists it under.
    head: Mutex<Head>,
    /// Bytes a writer said it will hold (`One::expect`), for its room.
    expect: AtomicU64,
}

impl Ext {
    fn new() -> Ext {
        Ext {
            loc: RwLock::new(Loc { base: 0, cap: 0 }),
            len: AtomicU64::new(0),
            wrote: AtomicU64::new(0),
            covered: AtomicU64::new(0),
            rode: Mutex::new((0, 0)),
            named: AtomicU64::new(0),
            names: AtomicUsize::new(0),
            users: AtomicUsize::new(0),
            writers: AtomicUsize::new(0),
            mapped: AtomicBool::new(false),
            head: Mutex::new(Head::Page(Vec::new())),
            expect: AtomicU64::new(0),
        }
    }

    /// A write at `at`, inside the first page, of the first `n` bytes of
    /// `parts` in turn, or one that failed where `n` is `None`: the page
    /// kept as the write left it. A page already listed is no longer
    /// known, and is read back where the file is next listed.
    #[cold]
    fn head_wrote<'a>(&self, mut at: u64, n: Option<usize>, parts: impl Iterator<Item = &'a [u8]>) {
        let mut h = lock(&self.head);
        let (Head::Page(v), Some(mut n)) = (&mut *h, n) else {
            *h = Head::Unread;
            return;
        };
        for part in parts {
            let part = &part[..part.len().min(n)];
            if part.is_empty() || at >= GRAIN {
                break;
            }
            let take = part.len().min((GRAIN - at) as usize);
            let (a, b) = (at as usize, at as usize + take);
            if v.len() < b {
                v.resize(b, 0);
            }
            v[a..b].copy_from_slice(&part[..take]);
            n -= part.len();
            at += part.len() as u64;
        }
    }

    /// The file's length set to `len`, where that changes its first page:
    /// what was written past `len` is gone, and what the length reaches
    /// past the writes is zeroes.
    #[cold]
    fn head_cut(&self, len: u64) {
        let mut h = lock(&self.head);
        match &mut *h {
            Head::Page(v) => v.truncate(len.min(GRAIN) as usize),
            _ => *h = Head::Unread,
        }
    }

    fn loc(&self) -> Loc {
        *self.loc.read().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn len(&self) -> u64 {
        self.len.load(Ordering::Acquire)
    }
}

/// A test's counts of what a store in one file did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OneCounts {
    /// The data file's length as reserved.
    pub end: u64,
    /// Bytes of it free to be given to a file.
    pub free: u64,
    /// Bytes given back and not yet free: named by a manifest no later
    /// one has replaced on the device.
    pub waiting: u64,
    /// Files in the table.
    pub files: u64,
    /// Bytes the files in the table hold.
    pub held: u64,
    /// Files moved for want of room where they stood.
    pub moves: u64,
    /// Files given room where they stood.
    pub grown: u64,
    /// Times the data file was made longer.
    pub growths: u64,
    /// Extents given to a file out of space a file held before.
    pub reused: u64,
    /// Syncs of the data file.
    pub syncs: u64,
    /// Files of the table a manifest has listed: what an open after a
    /// kill finds.
    pub listed: u64,
    /// Bytes of room given back whose pages the page cache was asked to
    /// drop (`One::dropped`).
    pub forgotten: u64,
    /// The newest manifest sequence the table knows durable: the slot an
    /// open took, and each one told since (`One::durable`).
    pub durable: u64,
    /// Files whose first page a manifest's listing read back from the data
    /// file for its checksum: none whose writes the table saw whole.
    pub head_reads: u64,
}

struct Inner {
    names: HashMap<String, Arc<Ext>>,
    space: Space,
    /// The data file's length as reserved.
    end: u64,
    /// Ranges given back that a manifest on the device may still name:
    /// `(the newest sequence that listed it, offset, length)`.
    waiting: Vec<(u64, u64, u64)>,
    /// The newest manifest sequence known durable.
    durable: u64,
    /// Ranges a file has let go that are neither free nor waiting yet:
    /// their pages are dropped from the page cache first, outside the
    /// table's lock, by whoever let them go (`One::release`,
    /// `One::dropped`), each with the sequence that listed it.
    dropping: Vec<(u64, u64, u64)>,
}

/// A store's data file and the table of the files in it.
pub(crate) struct One {
    /// The store's directory, as its engine names it.
    pub(crate) dir: PathBuf,
    path: PathBuf,
    /// A description of the data file of the table's own: its growth and
    /// the copies of a file moved.
    file: OsFile,
    s: Mutex<Inner>,
    /// The data file's syncs begun, and the newest of them that has
    /// returned, by the count it began at.
    sync_begun: AtomicU64,
    sync_done: AtomicU64,
    moves: AtomicU64,
    grown: AtomicU64,
    growths: AtomicU64,
    reused: AtomicU64,
    /// Rides and syncs so far (`news`).
    news: AtomicU64,
    /// Bytes of room given back whose pages were dropped (`forget`).
    forgotten: AtomicU64,
    head_reads: AtomicU64,
    /// The store this table was of is gone, as a process's death takes it
    /// (`kill`): nothing more is written through it.
    dead: AtomicBool,
    /// A test's: the room a file is given at its first write where it
    /// said nothing, in place of the write's own length, so that a file
    /// outgrows its room at its second.
    #[cfg(test)]
    tight: AtomicBool,
    /// A test's: the device has no room for the step ahead of what is
    /// asked for (`room`), as a nearly full one has not.
    #[cfg(test)]
    no_room_ahead: AtomicBool,
    /// A test's: run inside a file's sync, after the sync has read how
    /// many writes the file has had and before it returns, which is where
    /// a write beside the sync falls.
    #[cfg(test)]
    mid_sync: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

/// Whether a name the engine makes is a file this module keeps: a seal's
/// or a merge's output under its temp name, an ordered index, a form
/// file. Every other name is the directory's own -- the log, the
/// manifest's slots, a run of ordered ingest and what it leaves, the lock
/// -- and a name becomes one of these by a rename or a link from one.
pub(crate) fn takes(name: &str) -> bool {
    ["seal-", "compact-", "tier-", "ord-", "zpg-"]
        .iter()
        .any(|p| name.starts_with(p))
}

impl One {
    /// The head page of a data file.
    fn head_page() -> Vec<u8> {
        let mut h = vec![0u8; GRAIN as usize];
        h[..8].copy_from_slice(MAGIC);
        h[8..12].copy_from_slice(&1u32.to_le_bytes());
        let crc = crate::block::crc32(&h[..12]);
        h[12..16].copy_from_slice(&crc.to_le_bytes());
        h
    }

    /// A new store's data file in `dir`: its head written and synced, and
    /// nothing in it.
    pub(crate) fn create(dir: &Path) -> Result<Arc<One>> {
        let path = dir.join(NAME);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.write_all_at(&One::head_page(), 0)?;
        file.set_len(HEAD)?;
        file.sync_all()?;
        Ok(One::over(dir, path, file, HEAD))
    }

    /// An existing store's data file in `dir`, its table empty until a
    /// manifest's is taken (`adopt`).
    pub(crate) fn open(dir: &Path) -> Result<Arc<One>> {
        let path = dir.join(NAME);
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut h = vec![0u8; 16];
        file.read_exact_at(&mut h, 0)
            .map_err(|_| bad(format!("{} is no supdb data file", path.display())))?;
        if &h[..8] != MAGIC
            || crate::block::crc32(&h[..12]) != u32::from_le_bytes(h[12..16].try_into().unwrap())
        {
            return Err(bad(format!("{} is no supdb data file", path.display())));
        }
        let end = up(file.metadata()?.len().max(HEAD), GRAIN);
        Ok(One::over(dir, path, file, end))
    }

    fn over(dir: &Path, path: PathBuf, file: OsFile, end: u64) -> Arc<One> {
        let mut space = Space::default();
        space.give(HEAD, end - HEAD);
        Arc::new(One {
            dir: dir.to_path_buf(),
            path,
            file,
            s: Mutex::new(Inner {
                names: HashMap::new(),
                space,
                end,
                waiting: Vec::new(),
                durable: 0,
                dropping: Vec::new(),
            }),
            sync_begun: AtomicU64::new(0),
            sync_done: AtomicU64::new(0),
            moves: AtomicU64::new(0),
            grown: AtomicU64::new(0),
            growths: AtomicU64::new(0),
            reused: AtomicU64::new(0),
            news: AtomicU64::new(0),
            forgotten: AtomicU64::new(0),
            head_reads: AtomicU64::new(0),
            dead: AtomicBool::new(false),
            #[cfg(test)]
            tight: AtomicBool::new(false),
            #[cfg(test)]
            no_room_ahead: AtomicBool::new(false),
            #[cfg(test)]
            mid_sync: Mutex::new(None),
        })
    }

    pub(crate) fn counts(&self) -> OneCounts {
        let s = lock(&self.s);
        let mut seen: Vec<*const Ext> = Vec::new();
        let (mut held, mut listed) = (0, 0);
        for e in s.names.values() {
            let p = Arc::as_ptr(e);
            if !seen.contains(&p) {
                seen.push(p);
                held += e.len();
                listed += u64::from(e.named.load(Ordering::Acquire) != 0);
            }
        }
        OneCounts {
            end: s.end,
            free: s.space.bytes(),
            waiting: s.waiting.iter().map(|w| w.2).sum(),
            files: seen.len() as u64,
            held,
            moves: self.moves.load(Ordering::Relaxed),
            grown: self.grown.load(Ordering::Relaxed),
            growths: self.growths.load(Ordering::Relaxed),
            reused: self.reused.load(Ordering::Relaxed),
            syncs: self.sync_begun.load(Ordering::Relaxed),
            listed,
            forgotten: self.forgotten.load(Ordering::Relaxed),
            durable: s.durable,
            head_reads: self.head_reads.load(Ordering::Relaxed),
        }
    }

    // ------------------------------------------------------ the space --

    /// `len` bytes at `align`, from the free ranges or, where none holds
    /// them, from the data file made longer: by what is asked for and an
    /// eighth of the file besides, so a store that is growing asks the
    /// filesystem a few times a doubling, and a small one reserves little
    /// (`STEP`). The eighth is held to `AHEAD_MOST`, and where the device
    /// has no room for it the file is made longer by what is asked for
    /// alone.
    ///
    /// The ask is `fallocate` under the table's lock, on whichever thread
    /// wanted the room, and the filesystem reads a block bitmap from the
    /// device for each group it has not looked at: a few milliseconds
    /// beside a load, and a third of a second an ask where every small
    /// read waits behind megabytes of read-ahead. The eighth was held to
    /// 64 MiB for a while, which past half a gigabyte is no eighth: a
    /// store of seven gigabytes asked eighty times in a row of the suite,
    /// twice of them in the read pass after its load on the thread
    /// building the form files those reads were waiting for.
    fn room(&self, s: &mut Inner, len: u64, align: u64) -> Result<u64> {
        debug_assert!(len.is_multiple_of(GRAIN) && len > 0);
        if let Some(at) = s.space.take(len, align) {
            return Ok(at);
        }
        let at = up(s.end, align);
        let more = ahead(s.end);
        let mut end = up(at + len + more, if more < BIG { STEP } else { BIG });
        #[cfg(test)]
        let ahead = if self.no_room_ahead.load(Ordering::Relaxed) {
            Err(Error::from_raw_os_error(libc::ENOSPC))
        } else {
            self.reserve(s.end, end)
        };
        #[cfg(not(test))]
        let ahead = self.reserve(s.end, end);
        if let Err(e) = ahead {
            // No room for the step ahead is not no room for the file.
            if e.raw_os_error() != Some(libc::ENOSPC) {
                return Err(e);
            }
            end = up(at + len, GRAIN);
            self.reserve(s.end, end)?;
        }
        self.growths.fetch_add(1, Ordering::Relaxed);
        s.space.give(s.end, end - s.end);
        s.end = end;
        let at = s
            .space
            .take(len, align)
            .expect("the data file was made longer for this");
        Ok(at)
    }

    /// The data file's blocks from `from` to `to` reserved and its length
    /// `to`: `fallocate` on Linux, which reserves and writes nothing, so a
    /// device with no room refuses here and not at a page's writeback; the
    /// length alone elsewhere, and where the filesystem has no such call.
    fn reserve(&self, from: u64, to: u64) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: a reservation over a descriptor this owns; no memory
            // of ours is read or written.
            let rc = unsafe {
                libc::fallocate(
                    self.file.as_raw_fd(),
                    0,
                    from as libc::off_t,
                    (to - from) as libc::off_t,
                )
            };
            if rc == 0 {
                return Ok(());
            }
            let e = Error::last_os_error();
            if !matches!(
                e.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::ENOSYS)
            ) {
                return Err(e);
            }
        }
        let _ = from;
        self.file.set_len(to)
    }

    /// A range a file held given back. Free at once where no manifest
    /// listed it; else waiting for a later manifest to be durable.
    fn release(&self, s: &mut Inner, named: u64, at: u64, len: u64) {
        if len == 0 {
            return;
        }
        s.dropping.push((named, at, len));
    }

    /// The room `gone` lists (`Inner::dropping`, taken out under the
    /// lock by the caller that let it go) given back: its pages dropped
    /// from the page cache, with the table's lock not held, and then the
    /// range free, or waiting where a manifest on the device may still
    /// name it.
    ///
    /// A file of its own that is unlinked leaves the page cache at once.
    /// Room in the data file that a merge retired does not: its pages
    /// are the file's still, written once and read by nobody, which is
    /// how the kernel ranks the segments and form files written since,
    /// so under a bound a reclaim took those with them. At ten million
    /// keys under 1.25 GiB the scan pass after the lag sweep's full
    /// rewrite refaulted 110 thousand pages where a store of files
    /// refaulted 25, took nine thousand major faults where that store
    /// took none, and ran at a third of its rate, with the burst before
    /// it a quarter faster for the frees it no longer made. The range is
    /// dropped before it can be given to another file: a drop made after
    /// would take the new file's pages.
    ///
    /// A page still dirty is started on its way to the device and stays
    /// (`POSIX_FADV_DONTNEED`), which a retired file's are only where
    /// nothing synced it: a file that rode and was retired before the
    /// sync it rode for.
    fn dropped(&self, gone: Vec<(u64, u64, u64)>) {
        if gone.is_empty() {
            return;
        }
        for &(_, at, len) in &gone {
            self.forget(at, len);
        }
        let mut s = lock(&self.s);
        for (named, at, len) in gone {
            if named != 0 && named >= s.durable {
                s.waiting.push((named, at, len));
            } else {
                s.space.give(at, len);
            }
        }
    }

    /// The data file's pages over `[at, at + len)` dropped from the page
    /// cache, where the platform has the advice.
    fn forget(&self, at: u64, len: u64) {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: an advice over a descriptor this owns; no memory of
            // ours is read or written.
            unsafe {
                libc::posix_fadvise(
                    self.file.as_raw_fd(),
                    at as libc::off_t,
                    len as libc::off_t,
                    libc::POSIX_FADV_DONTNEED,
                );
            }
        }
        let _ = at;
        self.forgotten.fetch_add(len, Ordering::Relaxed);
    }

    /// The manifest of sequence `seq` is on the device, and with it every
    /// one before it is replaced: the ranges that only those listed are
    /// free.
    pub(crate) fn durable(&self, seq: u64) {
        let mut s = lock(&self.s);
        if seq <= s.durable {
            return;
        }
        s.durable = seq;
        let mut i = 0;
        while i < s.waiting.len() {
            if s.waiting[i].0 < seq {
                let (_, at, len) = s.waiting.swap_remove(i);
                s.space.give(at, len);
            } else {
                i += 1;
            }
        }
    }

    /// The store this table was of taken away as a process's death takes
    /// one, for a test that emulates a crash and opens the store again in
    /// the same process (`Db::crash`): from here nothing is written
    /// through this table's handles, and no write of theirs is in flight.
    /// A dead process's threads write nothing after it; a store forgotten
    /// in a live one leaves its merges running, and in a file a store of
    /// its own they would write into room the store opened after it has
    /// given to something else.
    pub(crate) fn kill(&self) {
        self.dead.store(true, Ordering::Release);
        let files: Vec<Arc<Ext>> = lock(&self.s).names.values().cloned().collect();
        for e in files {
            // A write holds its file's lock for reading across the call:
            // taken for writing, none is in flight.
            drop(e.loc.write().unwrap_or_else(|e| e.into_inner()));
        }
    }

    // ------------------------------------------------------ the names --

    /// The file under `name`, counted as in use before the table's lock is
    /// let go: its room cannot be given back between the look and the use.
    fn using(self: &Arc<One>, name: &str) -> Option<Using> {
        let s = lock(&self.s);
        let e = s.names.get(name)?.clone();
        e.users.fetch_add(1, Ordering::AcqRel);
        Some(Using {
            one: self.clone(),
            ext: e,
        })
    }

    /// Whether `name` is a file of the table.
    pub(crate) fn holds(&self, name: &str) -> bool {
        lock(&self.s).names.contains_key(name)
    }

    /// The table's names.
    pub(crate) fn names(&self) -> Vec<String> {
        lock(&self.s).names.keys().cloned().collect()
    }

    /// A file's length, where the table has it.
    pub(crate) fn len(&self, name: &str) -> Option<u64> {
        lock(&self.s).names.get(name).map(|e| e.len())
    }

    /// The bytes of the file under `name`, read from the data file: a
    /// test's, which reads a store's files by their names (`Db::one_read`).
    /// Under the table's lock, so no move of the file is in flight.
    pub(crate) fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        let s = lock(&self.s);
        let e = s.names.get(name)?;
        let mut out = vec![0u8; e.len() as usize];
        self.file.read_exact_at(&mut out, e.loc().base).ok()?;
        Some(out)
    }

    /// Where the file under `name` stands in the data file and how long
    /// it is: a test's, to cut or change its bytes as a crash would.
    pub(crate) fn place(&self, name: &str) -> Option<(u64, u64)> {
        let s = lock(&self.s);
        let e = s.names.get(name)?;
        Some((e.loc().base, e.len()))
    }

    /// One name less on `e`, and its room given back where that was the
    /// last thing holding it.
    fn unname(&self, s: &mut Inner, e: &Arc<Ext>) {
        e.names.fetch_sub(1, Ordering::AcqRel);
        self.let_go(s, e);
    }

    /// `e`'s room given back if nothing holds it: no name, no handle, no
    /// mapping.
    fn let_go(&self, s: &mut Inner, e: &Ext) {
        if e.names.load(Ordering::Acquire) != 0 || e.users.load(Ordering::Acquire) != 0 {
            return;
        }
        let mut loc = e.loc.write().unwrap_or_else(|e| e.into_inner());
        let (at, len) = (loc.base, loc.cap);
        loc.cap = 0;
        drop(loc);
        self.release(s, e.named.load(Ordering::Acquire), at, len);
    }

    /// A new file under `name`, of no bytes and no room yet, open to be
    /// written; a file that had the name loses it.
    pub(crate) fn make(self: &Arc<One>, name: &str) -> Result<ExtFile> {
        let e = Arc::new(Ext::new());
        e.names.store(1, Ordering::Release);
        e.users.store(1, Ordering::Release);
        let u = Using {
            one: self.clone(),
            ext: e.clone(),
        };
        let gone = {
            let mut s = lock(&self.s);
            if let Some(old) = s.names.insert(name.to_string(), e) {
                self.unname(&mut s, &old);
            }
            std::mem::take(&mut s.dropping)
        };
        self.dropped(gone);
        ExtFile::over(u, true)
    }

    /// The file under `name` opened: to be read, or with `write` to be
    /// written where its caller says, cut nowhere. `None` where it is no
    /// file of the table.
    pub(crate) fn reopen(self: &Arc<One>, name: &str, write: bool) -> Option<Result<ExtFile>> {
        Some(ExtFile::over(self.using(name)?, write))
    }

    /// The bytes the file under `name` is to hold, said before its first
    /// write, for its room (`vault::expect`).
    pub(crate) fn expect(&self, name: &str, bytes: u64) {
        if let Some(e) = lock(&self.s).names.get(name) {
            e.expect.store(bytes, Ordering::Release);
        }
    }

    /// `e`'s bytes as they stand ride the next sync of the data file
    /// (`ExtFile::ride`): the writes it has had, and the syncs begun so
    /// far, one begun after which covers them.
    fn rides(&self, e: &Ext) {
        let w = e.wrote.load(Ordering::Acquire);
        *lock(&e.rode) = (w + 1, self.sync_begun.load(Ordering::Acquire));
        self.news.fetch_add(1, Ordering::AcqRel);
    }

    /// A count that moves whenever a file of the table rides or is
    /// synced: what the segment work reads to know whether a file a
    /// manifest could list has come since it last looked (`owed`).
    pub(crate) fn news(&self) -> u64 {
        self.news.load(Ordering::Acquire)
    }

    /// Of `with`, the files beside the store's segments, what no manifest
    /// has listed: whether any is covered by a sync, which the next
    /// manifest lists, and whether any rode and waits for one.
    pub(crate) fn owed(&self, with: &[String]) -> (bool, bool) {
        let s = lock(&self.s);
        let (mut list, mut sync) = (false, false);
        for name in with {
            let Some(e) = s.names.get(name) else {
                continue;
            };
            if e.named.load(Ordering::Acquire) != 0 {
                continue;
            }
            if self.is_durable(e) {
                list = true;
            } else if lock(&e.rode).0 == e.wrote.load(Ordering::Acquire) + 1 {
                sync = true;
            }
        }
        (list, sync)
    }

    /// The data file synced through the table's own description, for the
    /// files that rode and that no sync of a file's own has come to cover
    /// (`ExtFile::ride`): files the store regenerates and checks where it
    /// opens them, so that an error this description does not report is a
    /// file made again. A segment is covered by its own handle's sync
    /// alone (`ExtFile::sync`), which answers its writes' errors.
    pub(crate) fn sync(&self) -> Result<()> {
        let b = self.sync_begun.fetch_add(1, Ordering::AcqRel) + 1;
        self.file.sync_data()?;
        self.sync_done.fetch_max(b, Ordering::AcqRel);
        self.news.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// `ExtFile::ride` for the file under `name`, by a caller that has
    /// closed it.
    pub(crate) fn ride(&self, name: &str) {
        if let Some(e) = lock(&self.s).names.get(name) {
            self.rides(e);
        }
    }

    /// `b` made a name of the file `a` names, which loses that name: a
    /// rename. A file that had `b` loses it. `None` where `a` is no file
    /// of the table.
    pub(crate) fn rename(&self, a: &str, b: &str) -> Option<()> {
        let gone = {
            let mut s = lock(&self.s);
            let e = s.names.remove(a)?;
            if let Some(old) = s.names.insert(b.to_string(), e) {
                self.unname(&mut s, &old);
            }
            std::mem::take(&mut s.dropping)
        };
        self.dropped(gone);
        Some(())
    }

    /// `b` made a second name of the file `a` names: a hard link, refused
    /// as one is where `b` is a name already. `None` where `a` is no file
    /// of the table.
    pub(crate) fn link(&self, a: &str, b: &str) -> Option<Result<()>> {
        let mut s = lock(&self.s);
        let e = s.names.get(a)?.clone();
        if s.names.contains_key(b) {
            return Some(Err(Error::new(
                ErrorKind::AlreadyExists,
                format!("{b} is a file of the store already"),
            )));
        }
        e.names.fetch_add(1, Ordering::AcqRel);
        s.names.insert(b.to_string(), e);
        Some(Ok(()))
    }

    /// The name `name` taken off its file, whose room is given back once
    /// nothing else holds it. `None` where it is no file of the table.
    pub(crate) fn remove(&self, name: &str) -> Option<()> {
        let gone = {
            let mut s = lock(&self.s);
            let e = s.names.remove(name)?;
            self.unname(&mut s, &e);
            std::mem::take(&mut s.dropping)
        };
        self.dropped(gone);
        Some(())
    }

    /// The file under `name` mapped for reading, as a file of its own is
    /// mapped: its bytes at its offset, through an open of the data file
    /// of the mapping's own, so the kernel's readahead over it is its own.
    /// A file that has been mapped is never moved, and its room is held
    /// for as long as the mapping stands.
    pub(crate) fn map(self: &Arc<One>, name: &str) -> Option<Result<MmapBytes>> {
        Some(self.using(name)?.map())
    }

    /// The file under `name` mapped as its bytes alone, for a reader that
    /// keeps the mapping itself and reads it by offsets of its own -- the
    /// cache of a store under a cipher, whose envelopes are files here
    /// (`pager::Back`): the mapping, what holds the file's room for as long
    /// as the reader keeps it, and where in the data file the file begins.
    /// `None` where it is no file of the table.
    #[allow(clippy::type_complexity)]
    pub(crate) fn map_held(
        self: &Arc<One>,
        name: &str,
    ) -> Option<Result<(memmap2::Mmap, Box<dyn std::any::Any + Send + Sync>, u64)>> {
        let u = self.using(name)?;
        Some(u.map_raw().map(|(m, hold, at)| {
            let hold: Box<dyn std::any::Any + Send + Sync> = Box::new(hold);
            (m, hold, at)
        }))
    }

    /// The data file's path, for an advice by a descriptor of the caller's
    /// own over a range of it.
    pub(crate) fn data_path(&self) -> &Path {
        &self.path
    }

    /// Whether `name`'s bytes are what a manifest listed for it: its first
    /// page under the checksum the manifest carried. `None` where it is no
    /// file of the table.
    pub(crate) fn on_device(&self, name: &str) -> Option<bool> {
        let e = lock(&self.s).names.get(name)?.clone();
        let want = match *lock(&e.head) {
            Head::Sum(want) => Some(want),
            Head::Page(_) | Head::Unread => None,
        };
        Some(match want {
            Some(want) => self.head_of(&e).is_ok_and(|got| got == want),
            None => true,
        })
    }

    /// The checksum a file's first page is listed under: the format's.
    fn head_sum(page: &[u8]) -> u32 {
        crate::block::crc32(page)
    }

    /// The checksum of a file's first page as the data file holds it.
    fn head_of(&self, e: &Ext) -> Result<u32> {
        let loc = e.loc();
        let n = e.len().min(GRAIN) as usize;
        let mut page = vec![0u8; n];
        self.file.read_exact_at(&mut page, loc.base)?;
        Ok(One::head_sum(&page))
    }

    /// Whether a sync has covered `e`'s last write: its own, or for one
    /// that rode, a sync of the data file begun after the ride.
    fn is_durable(&self, e: &Ext) -> bool {
        let w = e.wrote.load(Ordering::Acquire) + 1;
        if e.covered.load(Ordering::Acquire) == w {
            return true;
        }
        let (rode, at) = *lock(&e.rode);
        rode == w && self.sync_done.load(Ordering::Acquire) > at
    }

    // --------------------------------------------------- the manifest --

    /// What the manifest of sequence `seq` carries behind its own bytes:
    /// the place of each of `segs` the table holds, and of each of `with`
    /// -- the files beside them, which a store regenerates -- that a sync
    /// has covered. A segment the table holds and no sync has covered is
    /// refused: a manifest names only what the device holds.
    pub(crate) fn image(&self, seq: u64, segs: &[String], with: &[String]) -> Result<Vec<u8>> {
        let s = lock(&self.s);
        let mut rows: Vec<(&str, Arc<Ext>)> = Vec::new();
        for name in segs {
            if let Some(e) = s.names.get(name) {
                if !self.is_durable(e) {
                    return Err(bad(format!(
                        "a manifest would name {name}, whose bytes no sync has covered"
                    )));
                }
                rows.push((name, e.clone()));
            }
        }
        for name in with {
            if let Some(e) = s.names.get(name) {
                if self.is_durable(e) {
                    rows.push((name, e.clone()));
                }
            }
        }
        let mut body = Vec::new();
        body.extend_from_slice(&s.end.to_le_bytes());
        body.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        for (name, e) in &rows {
            let loc = e.loc();
            // From the writes that made the page (`Head`), and read back
            // only where those are not the page any more.
            let head = {
                let mut h = lock(&e.head);
                let c = match &mut *h {
                    Head::Sum(c) => *c,
                    Head::Page(v) => {
                        // A page and no more, whatever the writes were: a
                        // write piece is a megabyte, kept a file at a time.
                        debug_assert!(v.len() <= GRAIN as usize, "{}", v.len());
                        v.resize(e.len().min(GRAIN) as usize, 0);
                        One::head_sum(v)
                    }
                    Head::Unread => {
                        self.head_reads.fetch_add(1, Ordering::Relaxed);
                        self.head_of(e)?
                    }
                };
                *h = Head::Sum(c);
                c
            };
            body.extend_from_slice(&(name.len() as u16).to_le_bytes());
            body.extend_from_slice(name.as_bytes());
            body.extend_from_slice(&loc.base.to_le_bytes());
            body.extend_from_slice(&e.len().to_le_bytes());
            body.extend_from_slice(&head.to_le_bytes());
            e.named.fetch_max(seq, Ordering::AcqRel);
        }
        let mut out = Vec::with_capacity(body.len() + 16);
        out.extend_from_slice(SECTION);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&One::section_crc(seq, &body).to_le_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// A section's checksum: over the manifest's sequence and the body, so
    /// a section is its manifest's alone. A slot is written in place and
    /// a write torn between the manifest and its section leaves the
    /// manifest of one publish before the section of an earlier one, each
    /// whole under a checksum of its own bytes.
    fn section_crc(seq: u64, body: &[u8]) -> u32 {
        crate::block::crc32_resume(crate::block::crc32(&seq.to_le_bytes()), body)
    }

    /// The table the section of the manifest of sequence `seq` lists,
    /// parsed and checked: each file's name, offset, length and first
    /// page's checksum.
    pub(crate) fn parse(seq: u64, section: &[u8]) -> Result<Vec<Row>> {
        let torn = || bad("a manifest's table of extents is torn");
        if section.len() < 16 || &section[..8] != SECTION {
            return Err(torn());
        }
        let n = u32::from_le_bytes(section[8..12].try_into().unwrap()) as usize;
        let crc = u32::from_le_bytes(section[12..16].try_into().unwrap());
        let body = section.get(16..16 + n).ok_or_else(torn)?;
        if One::section_crc(seq, body) != crc {
            return Err(torn());
        }
        let mut at = 12;
        let count = u32::from_le_bytes(body.get(8..12).ok_or_else(torn)?.try_into().unwrap());
        let mut rows = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let l = u16::from_le_bytes(body.get(at..at + 2).ok_or_else(torn)?.try_into().unwrap())
                as usize;
            at += 2;
            let name = std::str::from_utf8(body.get(at..at + l).ok_or_else(torn)?)
                .map_err(|_| torn())?
                .to_string();
            at += l;
            let f = body.get(at..at + 20).ok_or_else(torn)?;
            at += 20;
            rows.push((
                name,
                u64::from_le_bytes(f[..8].try_into().unwrap()),
                u64::from_le_bytes(f[8..16].try_into().unwrap()),
                u32::from_le_bytes(f[16..20].try_into().unwrap()),
            ));
        }
        Ok(rows)
    }

    /// The bytes of a section at the front of `buf`, for a reader that
    /// finds it behind a manifest.
    pub(crate) fn section_len(buf: &[u8]) -> Option<usize> {
        if buf.len() < 16 || &buf[..8] != SECTION {
            return None;
        }
        let n = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
        (buf.len() >= 16 + n).then_some(16 + n)
    }

    /// The table made what the manifest of sequence `seq` listed, at an
    /// open, before any file of it is opened: every row a file, durable
    /// and named by that manifest, and every byte past the head that no
    /// row holds free. Refused where two rows overlap or one lies outside
    /// the data file, which is a manifest that is not this file's.
    pub(crate) fn adopt(&self, seq: u64, rows: &[Row]) -> Result<()> {
        let mut s = lock(&self.s);
        if s.names
            .values()
            .any(|e| e.users.load(Ordering::Acquire) != 0)
        {
            return Err(bad("a table of extents adopted under a file in use"));
        }
        let end = s.end;
        let mut space = Space::default();
        space.give(HEAD, end - HEAD);
        let mut names: HashMap<String, Arc<Ext>> = HashMap::new();
        let mut by_place: HashMap<u64, Arc<Ext>> = HashMap::new();
        for (name, base, len, head) in rows {
            let cap = up(*len, GRAIN);
            // Two names of one file: a link, listed under each.
            if let Some(e) = by_place.get(base) {
                if e.len() != *len {
                    return Err(bad(format!(
                        "the manifest's place for {name} is another file's"
                    )));
                }
                e.names.fetch_add(1, Ordering::AcqRel);
                names.insert(name.clone(), e.clone());
                continue;
            }
            let fits = *base >= HEAD
                && base % GRAIN == 0
                && base.checked_add(cap).is_some_and(|e| e <= end);
            if !fits || (cap != 0 && !space.take_at(*base, cap)) {
                return Err(bad(format!(
                    "the manifest's place for {name} is outside the data file or another file's"
                )));
            }
            let e = Arc::new(Ext::new());
            *e.loc.write().unwrap_or_else(|e| e.into_inner()) = Loc { base: *base, cap };
            e.len.store(*len, Ordering::Release);
            e.covered.store(1, Ordering::Release);
            e.named.store(seq, Ordering::Release);
            e.names.store(1, Ordering::Release);
            *lock(&e.head) = Head::Sum(*head);
            if cap != 0 {
                by_place.insert(*base, e.clone());
            }
            names.insert(name.clone(), e);
        }
        s.names = names;
        s.space = space;
        s.waiting.clear();
        s.durable = seq;
        Ok(())
    }

    /// The file under `name` copied to `to`, a file of its own, and
    /// synced: one file of a copy of the store. `None` where it is no file
    /// of the table.
    pub(crate) fn copy_out(self: &Arc<One>, name: &str, to: &Path) -> Option<Result<u64>> {
        let u = self.using(name)?;
        Some((|| {
            let h = ExtFile::over(u, false)?;
            let out = OsFile::create(to)?;
            let mut buf = vec![0u8; 1 << 20];
            let (mut at, len) = (0u64, h.len());
            while at < len {
                let n = ((len - at) as usize).min(buf.len());
                h.read_exact_at(&mut buf[..n], at)?;
                out.write_all_at(&buf[..n], at)?;
                at += n as u64;
            }
            out.sync_all()?;
            Ok(len)
        })())
    }
}

/// A file of the table in use: by a handle or by a mapping. Its room is
/// not given back while one stands.
struct Using {
    one: Arc<One>,
    ext: Arc<Ext>,
}

impl Using {
    /// A second use of the same file: safe with no lock, since this one
    /// already keeps the count from nothing.
    fn again(&self) -> Using {
        self.ext.users.fetch_add(1, Ordering::AcqRel);
        Using {
            one: self.one.clone(),
            ext: self.ext.clone(),
        }
    }

    /// The file mapped for reading, the mapping holding a use of its own.
    fn map(&self) -> Result<MmapBytes> {
        let (m, hold, _) = self.map_raw()?;
        Ok(MmapBytes::from_map_held(m, Box::new(hold)))
    }

    /// The file's bytes mapped, the use that holds its room for as long as
    /// the mapping stands, and where in the data file they begin.
    fn map_raw(&self) -> Result<(memmap2::Mmap, Using, u64)> {
        let hold = self.again();
        let loc = {
            // Under the file's own lock, so a move in flight is over
            // before the mapping is said to stand.
            let loc = hold.ext.loc.write().unwrap_or_else(|e| e.into_inner());
            hold.ext.mapped.store(true, Ordering::Release);
            *loc
        };
        let len = hold.ext.len() as usize;
        // Through an open of the data file of the mapping's own, closed
        // once the mapping stands, as a file of its own is mapped. The
        // kernel keeps its readahead window and its count of a mapping's
        // misses on the open file a mapping was made through, and stops
        // reading around a fault once that count passes a hundred: made
        // through the table's one open, every mapping of the store shared
        // both, a merge's and a build's misses counted against the scans
        // of every other file, where a store of files has them a segment.
        let own = OpenOptions::new().read(true).open(&hold.one.path)?;
        // SAFETY: the contract `MmapBytes::open` takes: the bytes mapped
        // are not cut under the mapping. The extent is held by `hold` for
        // the mapping's life, and nothing is written over a file once it
        // has its name.
        let m = unsafe {
            memmap2::MmapOptions::new()
                .offset(loc.base)
                .len(len)
                .map(&own)?
        };
        Ok((m, hold, loc.base))
    }
}

impl Drop for Using {
    fn drop(&mut self) {
        let gone = {
            let mut s = lock(&self.one.s);
            self.ext.users.fetch_sub(1, Ordering::AcqRel);
            self.one.let_go(&mut s, &self.ext);
            std::mem::take(&mut s.dropping)
        };
        self.one.dropped(gone);
    }
}

impl crate::bytes::MapHeld for Using {}

// ------------------------------------------------------------ a handle --

/// A file of the table open: what the door's file (`vault::File`) is for a
/// file kept here, each operation at an offset of the file's own, made
/// through a description of the data file of the handle's own.
pub(crate) struct ExtFile {
    u: Using,
    write: bool,
    /// The position a read or a write with none of its own goes to, shared
    /// with the handles cloned of this one as a descriptor's is.
    pos: Arc<AtomicU64>,
    os: OsFile,
    /// Each write is handed to the device as it lands (`hint_writes`).
    hint: AtomicBool,
}

impl Drop for ExtFile {
    fn drop(&mut self) {
        if !self.write {
            return;
        }
        let (one, e) = (&self.u.one, &self.u.ext);
        let mut s = lock(&one.s);
        if e.writers.fetch_sub(1, Ordering::AcqRel) == 1 {
            // The last writer: what it did not use goes back.
            let mut loc = e.loc.write().unwrap_or_else(|e| e.into_inner());
            let keep = up(e.len(), GRAIN);
            if keep < loc.cap {
                let (at, n) = (loc.base + keep, loc.cap - keep);
                loc.cap = keep;
                drop(loc);
                s.space.give(at, n);
            }
        }
    }
}

/// `errno` set for a caller that reads it after a call answered -1
/// (`vault::File::pwritev`).
fn set_errno(e: &Error) {
    let n = e.raw_os_error().unwrap_or(libc::EIO);
    // SAFETY: the thread's own errno.
    unsafe {
        #[cfg(target_os = "linux")]
        {
            *libc::__errno_location() = n;
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            *libc::__error() = n;
        }
    }
}

impl ExtFile {
    /// A handle over a use of a file: a description of the data file
    /// opened for it.
    fn over(u: Using, write: bool) -> Result<ExtFile> {
        let os = OpenOptions::new()
            .read(true)
            .write(write)
            .open(&u.one.path)?;
        if write {
            u.ext.writers.fetch_add(1, Ordering::AcqRel);
        }
        Ok(ExtFile {
            u,
            write,
            pos: Arc::new(AtomicU64::new(0)),
            os,
            hint: AtomicBool::new(false),
        })
    }

    /// Whether each write of this handle is handed to the device as it
    /// lands: what a segment's writer asks of a file of its own after
    /// each piece (`vault::File::hint_writes`).
    pub(crate) fn hint_writes(&self, on: bool) {
        self.hint.store(on, Ordering::Relaxed);
    }

    /// `[at, at + len)` of the data file started on its way to the
    /// device, waiting for nothing, where this handle hints its writes: a
    /// hint the sync that follows is shorter for, and nothing on an error.
    #[inline]
    fn hinted(&self, at: u64, len: u64) {
        #[cfg(target_os = "linux")]
        if len != 0 && self.hint.load(Ordering::Relaxed) {
            use std::os::fd::AsRawFd;
            // SAFETY: a writeback over a range of an open file; the call
            // reads and writes no memory of ours.
            let _ = unsafe {
                libc::sync_file_range(
                    self.os.as_raw_fd(),
                    at as libc::off64_t,
                    len as libc::off64_t,
                    libc::SYNC_FILE_RANGE_WRITE,
                )
            };
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (at, len);
    }

    /// The descriptor every operation of this handle is made through: a
    /// description of the data file of the handle's own, so a sync through
    /// it reads the errors of its own writes.
    pub(crate) fn description(&self) -> &OsFile {
        &self.os
    }

    /// The file's bytes.
    pub(crate) fn len(&self) -> u64 {
        self.u.ext.len()
    }

    /// A number that tells this file from every other a process has open:
    /// what an inode's number is to a file of its own.
    pub(crate) fn id(&self) -> u64 {
        Arc::as_ptr(&self.u.ext) as u64
    }

    /// A second handle on the same description, its position shared:
    /// `File::try_clone`.
    pub(crate) fn try_clone(&self) -> Result<ExtFile> {
        let os = self.os.try_clone()?;
        let u = self.u.again();
        if self.write {
            u.ext.writers.fetch_add(1, Ordering::AcqRel);
        }
        Ok(ExtFile {
            u,
            write: self.write,
            pos: self.pos.clone(),
            os,
            hint: AtomicBool::new(self.hint.load(Ordering::Relaxed)),
        })
    }

    /// Another description of the same file, read-only, its position its
    /// own (`vault::File::reopen`).
    pub(crate) fn reopen(&self) -> Result<ExtFile> {
        ExtFile::over(self.u.again(), false)
    }

    /// Room for the file to reach `end`: nothing where it has it, else
    /// more where it stands if what follows is free or is the data file's
    /// end, else the file moved to room found for it, its bytes copied.
    #[cold]
    fn grow(&self, end: u64) -> Result<()> {
        let (one, e) = (&self.u.one, &self.u.ext);
        let mut s = lock(&one.s);
        // Under the table's lock, which a kill takes: a dead store is
        // given no room and moves nothing.
        if one.dead.load(Ordering::Acquire) {
            return Err(Error::other(
                "a write to a file of a store that is gone (Db::crash)",
            ));
        }
        let mut loc = e.loc.write().unwrap_or_else(|e| e.into_inner());
        if end <= loc.cap {
            return Ok(());
        }
        if loc.cap == 0 {
            // Its first room: what its writer said it will hold, or this
            // write.
            #[cfg(test)]
            let said = if one.tight.load(Ordering::Relaxed) {
                0
            } else {
                e.expect.load(Ordering::Acquire)
            };
            #[cfg(not(test))]
            let said = e.expect.load(Ordering::Acquire);
            let want = up(said.max(end), GRAIN);
            let before = s.end;
            let at = one.room(&mut s, want, align_for(want))?;
            if at + want <= before {
                one.reused.fetch_add(1, Ordering::Relaxed);
            }
            *loc = Loc {
                base: at,
                cap: want,
            };
            return Ok(());
        }
        // Half as much again, a step at the least: a file that keeps
        // growing asks a few times a doubling.
        let want = up(end.max(loc.cap + (loc.cap / 2).max(STEP)), GRAIN);
        let more = want - loc.cap;
        let tail = loc.base + loc.cap;
        if s.space.take_at(tail, more) {
            one.grown.fetch_add(1, Ordering::Relaxed);
            loc.cap = want;
            return Ok(());
        }
        // What is free behind the file may run to the data file's end
        // and fall short, or the file may end there: the data file is
        // made longer by the rest, and nothing moves. A file that ended
        // short of the data file's end by less than it asked for was
        // moved, its bytes copied, with nothing behind it.
        if tail + s.space.run_at(tail) == s.end {
            let to = up(tail + more, if more < BIG { STEP } else { BIG });
            one.reserve(s.end, to)?;
            one.growths.fetch_add(1, Ordering::Relaxed);
            let from = s.end;
            s.space.give(from, to - from);
            s.end = to;
            let took = s.space.take_at(tail, more);
            debug_assert!(took, "the data file was made longer for this");
            one.grown.fetch_add(1, Ordering::Relaxed);
            loc.cap = want;
            return Ok(());
        }
        if e.mapped.load(Ordering::Acquire) {
            return Err(Error::other(
                "a file of the store was written past its room under a mapping of it",
            ));
        }
        let at = one.room(&mut s, want, align_for(want))?;
        let len = e.len();
        let mut buf = vec![0u8; (1 << 20).min(len.max(1) as usize)];
        let mut done = 0u64;
        while done < len {
            let n = ((len - done) as usize).min(buf.len());
            one.file.read_exact_at(&mut buf[..n], loc.base + done)?;
            one.file.write_all_at(&buf[..n], at + done)?;
            done += n as u64;
        }
        // The old room goes back as any file's does: free at once for a
        // file no manifest has listed, which a file being written is.
        let named = e.named.load(Ordering::Acquire);
        one.release(&mut s, named, loc.base, loc.cap);
        *loc = Loc {
            base: at,
            cap: want,
        };
        one.moves.fetch_add(1, Ordering::Relaxed);
        let gone = std::mem::take(&mut s.dropping);
        drop(loc);
        drop(s);
        one.dropped(gone);
        Ok(())
    }

    /// A write that begins past the file's end leaves a gap, which in a
    /// file of its own reads as zeroes and here is whatever a file before
    /// it left: the gap is written with zeroes first.
    #[cold]
    fn fill(&self, from: u64, to: u64) -> Result<()> {
        let zeroes = vec![0u8; ((to - from) as usize).min(1 << 20)];
        let mut at = from;
        while at < to {
            let n = ((to - at) as usize).min(zeroes.len());
            let loc = self.placed()?;
            self.os.write_all_at(&zeroes[..n], loc.base + at)?;
            drop(loc);
            at += n as u64;
        }
        Ok(())
    }

    /// Before a write of `[at, end)`: the room, and the gap before it.
    #[inline]
    fn before_write(&self, at: u64, end: u64) -> Result<()> {
        if end > self.u.ext.loc().cap {
            self.grow(end)?;
        }
        let len = self.u.ext.len();
        if at > len {
            self.fill(len, at)?;
        }
        Ok(())
    }

    /// Where the file stands, held for a write: the file is not moved
    /// under it, and a table that has been killed (`One::kill`) refuses
    /// here, under the lock its kill waits for, so no write of a dead
    /// store's is in flight once the kill has returned.
    #[inline]
    fn placed(&self) -> Result<std::sync::RwLockReadGuard<'_, Loc>> {
        let loc = self.u.ext.loc.read().unwrap_or_else(|e| e.into_inner());
        if self.u.one.dead.load(Ordering::Acquire) {
            return Err(Error::other(
                "a write to a file of a store that is gone (Db::crash)",
            ));
        }
        Ok(loc)
    }

    #[inline]
    fn wrote(&self, end: u64) {
        self.u.ext.len.fetch_max(end, Ordering::AcqRel);
        self.u.ext.wrote.fetch_add(1, Ordering::AcqRel);
    }

    /// `FileExt::write_at`.
    pub(crate) fn write_at(&self, buf: &[u8], at: u64) -> Result<usize> {
        self.before_write(at, at + buf.len() as u64)?;
        let loc = self.placed()?;
        let n = self.os.write_at(buf, loc.base + at);
        if at < GRAIN {
            let n = n.as_ref().ok().copied();
            self.u.ext.head_wrote(at, n, std::iter::once(buf));
        }
        let n = n?;
        self.hinted(loc.base + at, n as u64);
        drop(loc);
        self.wrote(at + n as u64);
        Ok(n)
    }

    /// `FileExt::write_all_at`.
    pub(crate) fn write_all_at(&self, buf: &[u8], at: u64) -> Result<()> {
        self.before_write(at, at + buf.len() as u64)?;
        let loc = self.placed()?;
        let done = self.os.write_all_at(buf, loc.base + at);
        if at < GRAIN {
            let n = done.is_ok().then_some(buf.len());
            self.u.ext.head_wrote(at, n, std::iter::once(buf));
        }
        done?;
        self.hinted(loc.base + at, buf.len() as u64);
        drop(loc);
        self.wrote(at + buf.len() as u64);
        Ok(())
    }

    /// One `pwritev` of `iov` at `at`, as `vault::File::pwritev` answers
    /// it: the bytes written, or -1 with the error in `errno`.
    pub(crate) fn pwritev(&self, iov: &[std::io::IoSlice<'_>], at: u64) -> isize {
        use std::os::fd::AsRawFd;
        let iov = &iov[..iov.len().min(1024)];
        let total: u64 = iov.iter().map(|s| s.len() as u64).sum();
        if let Err(e) = self.before_write(at, at + total) {
            set_errno(&e);
            return -1;
        }
        let loc = match self.placed() {
            Ok(loc) => loc,
            Err(e) => {
                set_errno(&e);
                return -1;
            }
        };
        // SAFETY: `IoSlice` is ABI-compatible with `iovec` on unix, and
        // every slice it names is borrowed for the call.
        let n = unsafe {
            libc::pwritev(
                self.os.as_raw_fd(),
                iov.as_ptr() as *const libc::iovec,
                iov.len() as libc::c_int,
                (loc.base + at) as libc::off_t,
            )
        };
        if at < GRAIN && n != 0 {
            let n = usize::try_from(n).ok();
            self.u.ext.head_wrote(at, n, iov.iter().map(|s| &**s));
        }
        if n > 0 {
            self.hinted(loc.base + at, n as u64);
        }
        drop(loc);
        if n > 0 {
            self.wrote(at + n as u64);
        }
        n
    }

    /// `FileExt::read_at`, of the file's own bytes and none past them.
    pub(crate) fn read_at(&self, buf: &mut [u8], at: u64) -> Result<usize> {
        let len = self.u.ext.len();
        if at >= len {
            return Ok(0);
        }
        let n = buf.len().min((len - at) as usize);
        let loc = self.u.ext.loc.read().unwrap_or_else(|e| e.into_inner());
        self.os.read_at(&mut buf[..n], loc.base + at)
    }

    /// `FileExt::read_exact_at`.
    pub(crate) fn read_exact_at(&self, mut buf: &mut [u8], mut at: u64) -> Result<()> {
        while !buf.is_empty() {
            match self.read_at(buf, at)? {
                0 => {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        "failed to fill whole buffer",
                    ))
                }
                n => {
                    buf = &mut buf[n..];
                    at += n as u64;
                }
            }
        }
        Ok(())
    }

    /// A read at the handle's position, which moves past it.
    pub(crate) fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let at = self.pos.load(Ordering::Acquire);
        let n = self.read_at(buf, at)?;
        self.pos.store(at + n as u64, Ordering::Release);
        Ok(n)
    }

    /// A write at the handle's position, which moves past it.
    pub(crate) fn write(&self, buf: &[u8]) -> Result<usize> {
        let at = self.pos.load(Ordering::Acquire);
        let n = self.write_at(buf, at)?;
        self.pos.store(at + n as u64, Ordering::Release);
        Ok(n)
    }

    /// `Seek::seek`.
    pub(crate) fn seek(&self, to: std::io::SeekFrom) -> Result<u64> {
        let at = match to {
            std::io::SeekFrom::Start(n) => Some(n),
            std::io::SeekFrom::End(d) => self.u.ext.len().checked_add_signed(d),
            std::io::SeekFrom::Current(d) => self.pos.load(Ordering::Acquire).checked_add_signed(d),
        }
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "a seek before a file's start"))?;
        self.pos.store(at, Ordering::Release);
        Ok(at)
    }

    /// `File::set_len`: cut, or made longer with zeroes.
    pub(crate) fn set_len(&self, len: u64) -> Result<()> {
        let now = self.u.ext.len();
        if len > now {
            self.before_write(len, len)?;
        }
        if len != now && len.min(now) < GRAIN {
            self.u.ext.head_cut(len);
        }
        self.u.ext.len.store(len, Ordering::Release);
        self.u.ext.wrote.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// The file's range `[at, at + len)` as the data file's, a length of
    /// nothing reaching the file's end and nothing past its room: what an
    /// advice or a writeback over a file of the table is made over, since
    /// a length of nothing handed to the kernel would reach the end of the
    /// data file, through every other file. Linux's alone, as those calls
    /// are (`vault::File::advise`, `sync_range`).
    #[cfg(target_os = "linux")]
    pub(crate) fn range(&self, at: u64, len: u64) -> (u64, u64) {
        let loc = self.u.ext.loc();
        let end = up(self.u.ext.len(), GRAIN).min(loc.cap);
        let at = at.min(end);
        let n = if len == 0 {
            end - at
        } else {
            len.min(end - at)
        };
        (loc.base + at, n)
    }

    /// The file made durable: `File::sync_data` and `File::sync_all`. A
    /// sync of the data file through this handle's own description, which
    /// answers the errors of this file's writes, and after which a
    /// manifest may name the file. Every file of the store that rode
    /// (`ride`) before it began is durable with it.
    pub(crate) fn sync(&self) -> Result<()> {
        let w = self.u.ext.wrote.load(Ordering::Acquire);
        let b = self.u.one.sync_begun.fetch_add(1, Ordering::AcqRel) + 1;
        #[cfg(test)]
        if let Some(h) = lock(&self.u.one.mid_sync).as_ref() {
            h();
        }
        self.os.sync_data()?;
        self.u.one.sync_done.fetch_max(b, Ordering::AcqRel);
        self.u.ext.covered.fetch_max(w + 1, Ordering::AcqRel);
        self.u.one.news.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// The file's bytes are all written and it asks for no sync of its
    /// own: it is durable with the next sync of any file of the store,
    /// which is a sync of the data file. For a file a store regenerates,
    /// written beside one whose sync follows.
    pub(crate) fn ride(&self) {
        self.u.one.rides(&self.u.ext);
    }

    /// The file mapped whole, as `One::map` maps it by its name.
    pub(crate) fn map(&self) -> Result<MmapBytes> {
        self.u.map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Bytes;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("supdb-next-one-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn bytes(n: usize, salt: u64) -> Vec<u8> {
        let mut s = 0x1234_5678_9abc_def0u64 ^ salt;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                s as u8
            })
            .collect()
    }

    /// A file written under `name`, synced, its writer closed.
    fn put(one: &Arc<One>, name: &str, data: &[u8]) {
        let f = one.make(name).unwrap();
        f.write_all_at(data, 0).unwrap();
        f.sync().unwrap();
    }

    fn read(one: &Arc<One>, name: &str) -> Vec<u8> {
        let f = one.reopen(name, false).unwrap().unwrap();
        let mut out = vec![0u8; f.len() as usize];
        f.read_exact_at(&mut out, 0).unwrap();
        out
    }

    #[test]
    fn space_is_taken_low_first_and_joined_when_given_back() {
        let mut s = Space::default();
        s.give(1 << 20, 16 << 20);
        let a = s.take(4096, 4096).unwrap();
        let b = s.take(2 << 20, BIG).unwrap();
        let c = s.take(4096, 4096).unwrap();
        assert_eq!(a, 1 << 20);
        assert_eq!(b, 2 << 20, "a big file at a big file's alignment");
        assert_eq!(c, (1 << 20) + 4096, "the gap before it is used");
        s.give(a, 4096);
        s.give(c, 4096);
        s.give(b, 2 << 20);
        assert_eq!(
            s.free.len(),
            1,
            "given back, the ranges are one: {:?}",
            s.free
        );
        assert_eq!(s.bytes(), 16 << 20);
        assert!(s.take(17 << 20, 4096).is_none());
        assert!(s.take_at(5 << 20, 1 << 20));
        assert!(!s.take_at(5 << 20, 1 << 20), "taken once");
        assert!(!s.take_at(16 << 20, 2 << 20), "past the end");
        assert_eq!(s.bytes(), 15 << 20);
    }

    #[test]
    fn a_file_reads_back_under_each_of_its_names_and_through_a_mapping() {
        let d = scratch("names");
        let one = One::create(&d).unwrap();
        let data = bytes(300_000, 1);
        put(&one, "seal-1.tmp", &data);
        assert!(one.holds("seal-1.tmp"));
        one.rename("seal-1.tmp", "pcs-1.sup").unwrap();
        assert!(!one.holds("seal-1.tmp"));
        one.link("pcs-1.sup", "par-1.sup").unwrap().unwrap();
        assert!(one.link("pcs-1.sup", "par-1.sup").unwrap().is_err());
        assert_eq!(read(&one, "pcs-1.sup"), data);
        assert_eq!(read(&one, "par-1.sup"), data);
        let m = one.map("par-1.sup").unwrap().unwrap();
        assert_eq!(m.len(), data.len() as u64);
        assert_eq!(m.slice_at(0, data.len()).unwrap(), &data[..]);
        assert_eq!(one.len("pcs-1.sup"), Some(data.len() as u64));
        assert!(one.rename("nothing", "x").is_none());
        assert!(one.remove("nothing").is_none());
        assert_eq!(one.counts().files, 1, "two names, one file");
        // Both names gone and the mapping still reads: its room is held.
        one.remove("pcs-1.sup").unwrap();
        one.remove("par-1.sup").unwrap();
        let held = one.counts();
        assert_eq!(held.files, 0);
        put(&one, "seal-2.tmp", &bytes(300_000, 2));
        assert_eq!(m.slice_at(0, data.len()).unwrap(), &data[..]);
        drop(m);
        assert!(
            one.counts().free + 400_000 > held.free,
            "the room came back with the mapping"
        );
    }

    /// The resident pages of the data file over `[at, at + len)`.
    #[cfg(target_os = "linux")]
    fn resident(one: &One, at: u64, len: u64) -> usize {
        // SAFETY: a read-only mapping nothing is read through, and a
        // vector with a byte for every page of the range asked of.
        unsafe {
            let m = memmap2::MmapOptions::new()
                .offset(at)
                .len(len as usize)
                .map(&one.file)
                .unwrap();
            let mut v = vec![0u8; (len as usize).div_ceil(4096)];
            let rc = libc::mincore(
                m.as_ptr() as *mut libc::c_void,
                m.len(),
                v.as_mut_ptr().cast(),
            );
            assert_eq!(rc, 0, "mincore");
            v.iter().filter(|b| **b & 1 != 0).count()
        }
    }

    /// A file's pages leave the page cache where its room is given back,
    /// as an unlinked file's do, and before the room is another file's.
    #[cfg(target_os = "linux")]
    #[test]
    fn room_given_back_leaves_the_page_cache() {
        let d = scratch("forget");
        let one = One::create(&d).unwrap();
        let data = bytes(2 << 20, 9);
        let f = one.make("seal-1.tmp").unwrap();
        f.write_all_at(&data, 0).unwrap();
        // Written back, so the pages are clean: a dirty page is started
        // on its way and stays.
        f.sync().unwrap();
        drop(f);
        let (at, len) = one.place("seal-1.tmp").unwrap();
        assert_eq!(len, 2 << 20);
        assert!(
            resident(&one, at, len) > 256,
            "the file's pages are in the cache while it stands: {}",
            resident(&one, at, len)
        );
        // A mapping holds the room past the name, and the pages with it.
        let m = one.map("seal-1.tmp").unwrap().unwrap();
        one.remove("seal-1.tmp").unwrap();
        assert_eq!(one.counts().forgotten, 0, "held by the mapping");
        assert!(resident(&one, at, len) > 256);
        let free = one.counts().free;
        drop(m);
        let c = one.counts();
        assert_eq!(c.forgotten, 2 << 20, "{c:?}");
        assert_eq!(c.free, free + (2 << 20), "and the room is free: {c:?}");
        assert_eq!(
            resident(&one, at, len),
            0,
            "its pages are gone from the cache"
        );
    }

    #[test]
    fn a_writer_grows_where_it_stands_and_is_moved_where_it_cannot() {
        let d = scratch("grow");
        let one = One::create(&d).unwrap();
        one.tight.store(true, Ordering::Relaxed);
        let a = bytes(3 << 20, 3);
        let f = one.make("seal-1.tmp").unwrap();
        // A megabyte at a time: the room given at the first write is the
        // first write's, and each one after asks for more.
        for (i, piece) in a.chunks(1 << 20).enumerate() {
            f.write_all_at(piece, (i as u64) << 20).unwrap();
            if i == 0 {
                // Another file lands right behind the first's room.
                put(&one, "ord-1.oidx", &bytes(8192, 4));
            }
        }
        let c = one.counts();
        assert!(c.moves >= 1, "the file behind it forced a move: {c:?}");
        f.sync().unwrap();
        drop(f);
        assert_eq!(read(&one, "seal-1.tmp"), a, "its bytes moved with it");
        assert_eq!(read(&one, "ord-1.oidx"), bytes(8192, 4));
        // With nothing behind it a file grows where it stands: in a file
        // of its own, since the lowest room of this one is the hole the
        // move left, with the index behind it.
        let d = scratch("grow-alone");
        let one = One::create(&d).unwrap();
        one.tight.store(true, Ordering::Relaxed);
        let g = one.make("seal-2.tmp").unwrap();
        for (i, piece) in a.chunks(1 << 20).enumerate() {
            g.write_all_at(piece, (i as u64) << 20).unwrap();
        }
        let c = one.counts();
        assert_eq!(c.moves, 0, "{c:?}");
        assert!(c.grown > 0, "{c:?}");
        drop(g);
        assert_eq!(read(&one, "seal-2.tmp"), a);
    }

    #[test]
    fn the_data_file_is_made_longer_by_an_eighth_a_few_times_a_doubling() {
        // The step itself, at the sizes no test can write: an eighth from
        // a step to a gibibyte, where a cap of 64 MiB had a store of four
        // gibibytes ask the filesystem sixty times for its next four.
        assert_eq!(ahead(0), STEP);
        assert_eq!(ahead(8 * STEP), STEP);
        assert_eq!(ahead(512 << 20), 64 << 20);
        assert_eq!(ahead(4 << 30), 512 << 20);
        assert_eq!(ahead(8 << 30), AHEAD_MOST);
        assert_eq!(ahead(64 << 30), AHEAD_MOST);
        // And a table's: the second file's room is found by making the
        // data file longer by its length and an eighth of what stood.
        let d = scratch("ahead");
        let one = One::create(&d).unwrap();
        let f = one.make("seal-1.tmp").unwrap();
        one.expect("seal-1.tmp", 16 << 20);
        f.write_all_at(&bytes(4096, 1), 0).unwrap();
        let before = one.counts();
        let g = one.make("seal-2.tmp").unwrap();
        one.expect("seal-2.tmp", 8 << 20);
        g.write_all_at(&bytes(4096, 2), 0).unwrap();
        let c = one.counts();
        assert_eq!(c.growths, before.growths + 1, "{c:?}");
        let beyond = c.end - before.end - (8 << 20);
        assert!(
            (before.end / 8..=before.end / 8 + 2 * BIG).contains(&beyond),
            "an eighth of {} beyond what was asked for: {beyond}",
            before.end
        );
        // A third file of a mebibyte finds its room in that eighth, and
        // the filesystem is asked nothing.
        let h = one.make("seal-3.tmp").unwrap();
        one.expect("seal-3.tmp", 1 << 20);
        h.write_all_at(&bytes(4096, 3), 0).unwrap();
        assert_eq!(one.counts().growths, c.growths);
    }

    #[test]
    fn a_device_with_no_room_for_the_step_ahead_gives_what_is_asked_for() {
        let d = scratch("ahead-none");
        let one = One::create(&d).unwrap();
        put(&one, "seal-1.tmp", &bytes(4 << 20, 1));
        let before = one.counts();
        one.no_room_ahead.store(true, Ordering::Relaxed);
        let a = bytes(2 << 20, 2);
        let f = one.make("seal-2.tmp").unwrap();
        one.expect("seal-2.tmp", a.len() as u64);
        f.write_all_at(&a, 0).unwrap();
        f.sync().unwrap();
        drop(f);
        let c = one.counts();
        assert_eq!(c.growths, before.growths + 1, "{c:?}");
        assert_eq!(
            c.end,
            up(before.end, BIG) + a.len() as u64,
            "longer by the file and nothing ahead of it: {c:?}"
        );
        assert_eq!(read(&one, "seal-2.tmp"), a);
        assert_eq!(read(&one, "seal-1.tmp"), bytes(4 << 20, 1));
    }

    #[test]
    fn what_a_writer_said_it_would_hold_is_its_room_and_the_rest_comes_back() {
        let d = scratch("expect");
        let one = One::create(&d).unwrap();
        let f = one.make("compact-1.tmp").unwrap();
        one.expect("compact-1.tmp", 8 << 20);
        f.write_all_at(&bytes(1 << 20, 5), 0).unwrap();
        f.write_all_at(&bytes(1 << 20, 6), 1 << 20).unwrap();
        let c = one.counts();
        assert_eq!((c.moves, c.grown), (0, 0), "room from the start: {c:?}");
        let free = c.free;
        drop(f);
        assert_eq!(
            one.counts().free,
            free + (6 << 20),
            "the six mebibytes unused came back"
        );
    }

    #[test]
    fn a_gap_before_a_write_reads_as_zeroes_over_space_used_before() {
        let d = scratch("gap");
        let one = One::create(&d).unwrap();
        put(&one, "seal-1.tmp", &vec![0xAAu8; 64 << 10]);
        one.remove("seal-1.tmp").unwrap();
        // The next file takes the same room, and leaves a gap in it.
        let f = one.make("seal-2.tmp").unwrap();
        f.write_all_at(&[1u8; 100], 0).unwrap();
        f.write_all_at(&[2u8; 100], 40_000).unwrap();
        f.set_len(50_000).unwrap();
        drop(f);
        let got = read(&one, "seal-2.tmp");
        assert_eq!(got.len(), 50_000);
        assert!(got[..100].iter().all(|&b| b == 1));
        assert!(
            got[100..40_000].iter().all(|&b| b == 0),
            "the gap is zeroes"
        );
        assert!(got[40_000..40_100].iter().all(|&b| b == 2));
        assert!(
            got[40_100..].iter().all(|&b| b == 0),
            "the length past the last write is zeroes"
        );
        // Nothing past a file's end is read.
        let f = one.reopen("seal-2.tmp", false).unwrap().unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(f.read_at(&mut buf, 50_000).unwrap(), 0);
        assert_eq!(f.read_at(&mut buf, 49_990).unwrap(), 10);
        assert!(f.read_exact_at(&mut buf, 49_990).is_err());
    }

    #[test]
    fn room_a_manifest_named_waits_for_a_later_manifest_on_the_device() {
        let d = scratch("wait");
        let one = One::create(&d).unwrap();
        put(&one, "par-1.sup", &bytes(1 << 20, 7));
        put(&one, "seal-9.tmp", &bytes(1 << 20, 8));
        let before = one.counts().free;
        // A file no manifest listed is free the moment it goes.
        one.remove("seal-9.tmp").unwrap();
        assert_eq!(one.counts().free, before + (1 << 20));
        // One a manifest listed waits.
        let image = one.image(5, &["par-1.sup".to_string()], &[]).unwrap();
        assert_eq!(One::parse(5, &image).unwrap().len(), 1);
        one.durable(5);
        one.remove("par-1.sup").unwrap();
        let c = one.counts();
        assert_eq!((c.free, c.waiting), (before + (1 << 20), 1 << 20), "{c:?}");
        // The same manifest durable again frees nothing: it is the one
        // that names the file.
        one.durable(5);
        assert_eq!(one.counts().waiting, 1 << 20);
        // The next one does.
        one.durable(6);
        let c = one.counts();
        assert_eq!((c.free, c.waiting), (before + (2 << 20), 0), "{c:?}");
        // And a file that goes before the manifest that names it is known
        // durable waits past that manifest too: told durable, it is the
        // newest slot on the device and it names the room.
        put(&one, "par-2.sup", &bytes(1 << 20, 9));
        let free = one.counts().free;
        one.image(7, &["par-2.sup".to_string()], &[]).unwrap();
        one.remove("par-2.sup").unwrap();
        assert_eq!(one.counts().waiting, 1 << 20);
        one.durable(7);
        let c = one.counts();
        assert_eq!(
            (c.free, c.waiting),
            (free, 1 << 20),
            "the slot that names it: {c:?}"
        );
        one.durable(8);
        let c = one.counts();
        assert_eq!((c.free, c.waiting), (free + (1 << 20), 0), "{c:?}");
        assert_eq!(c.durable, 8);
        // A manifest older than the newest known on the device says
        // nothing new: the newest stands, and room that only an older one
        // listed is free the moment it goes, as it was before the older
        // one was told again.
        one.durable(7);
        assert_eq!(one.counts().durable, 8);
        put(&one, "par-3.sup", &bytes(1 << 20, 10));
        let free = one.counts().free;
        one.image(7, &["par-3.sup".to_string()], &[]).unwrap();
        one.remove("par-3.sup").unwrap();
        let c = one.counts();
        assert_eq!(
            (c.free, c.waiting),
            (free + (1 << 20), 0),
            "listed by a manifest a later one on the device replaced: {c:?}"
        );
    }

    /// A sync covers the writes made before it began and no other: a
    /// write that lands beside it may have missed it, so the file is not
    /// one a manifest may name until a sync begun after that write.
    #[test]
    fn a_write_beside_a_sync_is_not_covered_by_it() {
        let d = scratch("mid-sync");
        let one = One::create(&d).unwrap();
        let f = one.make("pcs-1.sup").unwrap();
        f.write_all_at(&bytes(50_000, 3), 0).unwrap();
        let names = ["pcs-1.sup".to_string()];
        let beside = f.try_clone().unwrap();
        *lock(&one.mid_sync) = Some(Box::new(move || {
            beside.write_all_at(&[7u8; 100], 50_000).unwrap();
        }));
        f.sync().unwrap();
        *lock(&one.mid_sync) = None;
        let e = one.image(1, &names, &[]).unwrap_err();
        assert!(e.to_string().contains("no sync has covered"), "{e}");
        f.sync().unwrap();
        one.image(1, &names, &[]).unwrap();
    }

    /// A table that has been killed takes no write: a store forgotten as
    /// a crash forgets it (`Db::crash`) leaves its threads running, and
    /// what they write would land in room the store opened after it has
    /// given to another file.
    #[test]
    fn a_killed_table_takes_no_write() {
        let d = scratch("killed");
        let one = One::create(&d).unwrap();
        let f = one.make("compact-1.tmp").unwrap();
        one.expect("compact-1.tmp", 1 << 20);
        f.write_all_at(&bytes(10_000, 4), 0).unwrap();
        one.kill();
        let e = f.write_all_at(&bytes(10_000, 5), 10_000).unwrap_err();
        assert!(e.to_string().contains("gone"), "{e}");
        // Past its room too, where the write asks the table first.
        let e = f.write_all_at(&bytes(10_000, 5), 4 << 20).unwrap_err();
        assert!(e.to_string().contains("gone"), "{e}");
        assert_eq!(f.len(), 10_000, "nothing of either landed");
    }

    /// A manifest lists a file under the checksum of its first page, taken
    /// from the writes that made the page and not from the data file
    /// (`Head`): written out of order and in pieces, across the page's end,
    /// by a vectored write, behind a gap and under a cut, the checksum is
    /// the page's as the data file holds it and nothing was read for it. A
    /// page written again after it was listed is read back once, where the
    /// file is next listed.
    #[test]
    fn a_files_head_is_taken_from_its_writes_and_read_back_only_where_they_are_not_it() {
        let d = scratch("head-writes");
        let one = One::create(&d).unwrap();
        let page = GRAIN as usize;
        let head = |name: &str, seq: u64| {
            let names = [name.to_string()];
            let rows = One::parse(seq, &one.image(seq, &names, &[]).unwrap()).unwrap();
            assert_eq!(rows.len(), 1);
            rows[0].3
        };
        let on_file = |name: &str| One::head_sum(&read(&one, name)[..]);
        // A segment's order: what lies past the page first, a piece across
        // the page's end, the page's start, and the rest of it by a write
        // of two slices.
        let data = bytes(3 * page, 21);
        let f = one.make("pcs-1.sup").unwrap();
        f.write_all_at(&data[6000..], 6000).unwrap();
        f.write_all_at(&data[3000..6000], 3000).unwrap();
        assert_eq!(f.write_at(&data[..100], 0).unwrap(), 100);
        let iov = [
            std::io::IoSlice::new(&data[100..1000]),
            std::io::IoSlice::new(&data[1000..3000]),
        ];
        assert_eq!(f.pwritev(&iov, 100), 2900);
        f.sync().unwrap();
        assert_eq!(read(&one, "pcs-1.sup"), data);
        assert_eq!(head("pcs-1.sup", 1), One::head_sum(&data[..page]));
        // A file shorter than a page: bytes behind a gap, cut under them,
        // and made longer again, which is zeroes to its length.
        let g = one.make("zpg-2.zpg").unwrap();
        g.write_all_at(&[7u8; 10], 500).unwrap();
        g.set_len(300).unwrap();
        g.set_len(2000).unwrap();
        g.sync().unwrap();
        assert_eq!(read(&one, "zpg-2.zpg"), vec![0u8; 2000]);
        assert_eq!(head("zpg-2.zpg", 2), on_file("zpg-2.zpg"));
        // And one a write reaches part of the way into.
        let h = one.make("ord-3.oidx").unwrap();
        h.write_all_at(&data[..700], 0).unwrap();
        h.sync().unwrap();
        assert_eq!(head("ord-3.oidx", 3), One::head_sum(&data[..700]));
        assert_eq!(one.counts().head_reads, 0, "nothing read for a checksum");
        // Listed again, the checksum is the one taken.
        assert_eq!(head("pcs-1.sup", 4), One::head_sum(&data[..page]));
        assert_eq!(one.counts().head_reads, 0);
        // The page written after its listing is not the page the table
        // kept: read back, once.
        f.write_all_at(&[9u8; 16], 32).unwrap();
        f.sync().unwrap();
        let mut want = data[..page].to_vec();
        want[32..48].fill(9);
        assert_eq!(head("pcs-1.sup", 5), One::head_sum(&want));
        assert_eq!(one.counts().head_reads, 1);
        assert_eq!(head("pcs-1.sup", 6), One::head_sum(&want));
        // A cut inside the page of a file already listed, the same.
        g.set_len(100).unwrap();
        g.sync().unwrap();
        assert_eq!(head("zpg-2.zpg", 7), One::head_sum(&[0u8; 100]));
        assert_eq!(one.counts().head_reads, 2);
        // A write past the page of a file already listed leaves its
        // checksum the one taken.
        f.write_all_at(&[1u8; 16], 2 * page as u64).unwrap();
        f.sync().unwrap();
        assert_eq!(head("pcs-1.sup", 8), One::head_sum(&want));
        assert_eq!(one.counts().head_reads, 2);
    }

    #[test]
    fn a_manifest_lists_what_a_sync_covered_and_nothing_else() {
        let d = scratch("image");
        let one = One::create(&d).unwrap();
        let seg = one.make("pcs-1.sup").unwrap();
        seg.write_all_at(&bytes(100_000, 9), 0).unwrap();
        let names = ["pcs-1.sup".to_string()];
        let with = ["ord-1.oidx".to_string(), "zpg-1.zpg".to_string()];
        // A segment no sync has covered is refused.
        let e = one.image(1, &names, &with).unwrap_err();
        assert!(e.to_string().contains("no sync has covered"), "{e}");
        // Its index rides the segment's sync; a form file written after
        // it rides nothing yet.
        let ord = one.make("ord-1.oidx").unwrap();
        ord.write_all_at(&bytes(5000, 10), 0).unwrap();
        ord.ride();
        seg.sync().unwrap();
        let z = one.make("zpg-1.zpg").unwrap();
        z.write_all_at(&bytes(9000, 11), 0).unwrap();
        z.ride();
        let rows = One::parse(1, &one.image(1, &names, &with).unwrap()).unwrap();
        let listed: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(listed, ["pcs-1.sup", "ord-1.oidx"]);
        // A write after the sync takes the file out again until the next.
        seg.write_all_at(&[1], 10).unwrap();
        assert!(one.image(2, &names, &with).is_err());
        seg.sync().unwrap();
        let rows = One::parse(2, &one.image(2, &names, &with).unwrap()).unwrap();
        assert_eq!(rows.len(), 3, "the form file rode the second sync");
        // A name that is no file of the table is the directory's: left out.
        let rows = One::parse(3, &one.image(3, &["pcs-7.sup".to_string()], &[]).unwrap()).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn a_table_adopted_at_an_open_is_the_files_and_the_space_between_them() {
        let d = scratch("adopt");
        let (image, a, b) = {
            let one = One::create(&d).unwrap();
            let (a, b) = (bytes(1_500_000, 12), bytes(70_000, 13));
            put(&one, "par-1.sup", &a);
            put(&one, "seal-junk.tmp", &bytes(2 << 20, 14));
            put(&one, "ord-1.oidx", &b);
            one.link("par-1.sup", "par-2.sup").unwrap().unwrap();
            let names = ["par-1.sup".to_string(), "par-2.sup".to_string()];
            (
                one.image(7, &names, &["ord-1.oidx".to_string()]).unwrap(),
                a,
                b,
            )
        };
        let one = One::open(&d).unwrap();
        let rows = One::parse(7, &image).unwrap();
        one.adopt(7, &rows).unwrap();
        assert_eq!(read(&one, "par-1.sup"), a);
        assert_eq!(read(&one, "par-2.sup"), a);
        assert_eq!(read(&one, "ord-1.oidx"), b);
        assert!(
            !one.holds("seal-junk.tmp"),
            "what no manifest listed is free space"
        );
        let c = one.counts();
        assert_eq!(c.files, 2);
        assert_eq!(
            c.free + up(a.len() as u64, GRAIN) + up(b.len() as u64, GRAIN),
            c.end - HEAD
        );
        assert_eq!(one.on_device("par-1.sup"), Some(true));
        // Removing one name of two keeps the file.
        one.remove("par-1.sup").unwrap();
        assert_eq!(read(&one, "par-2.sup"), a);
        // The file written over, as the room's next file would write it:
        // the manifest's checksum of its first page says it is not there.
        one.file
            .write_all_at(&[0x55u8; 64], one.place("par-2.sup").unwrap().0)
            .unwrap();
        assert_eq!(one.on_device("par-2.sup"), Some(false));
        // Rows that overlap, or lie outside the file, are another file's.
        let mut bad_rows = rows.clone();
        bad_rows.push(("x".into(), rows[0].1 + 4096, 10, 0));
        assert!(One::open(&d).unwrap().adopt(7, &bad_rows).is_err());
        let far = vec![("x".to_string(), 1u64 << 40, 10u64, 0u32)];
        assert!(One::open(&d).unwrap().adopt(7, &far).is_err());
        // A section cut or changed is torn.
        assert!(One::parse(7, &image[..image.len() - 1]).is_err());
        let mut flipped = image.clone();
        flipped[20] ^= 1;
        assert!(One::parse(7, &flipped).is_err());
        // A whole section of another manifest's is not this one's.
        assert!(One::parse(8, &image).is_err());
    }

    #[test]
    fn a_data_file_is_known_by_its_head() {
        let d = scratch("head");
        assert!(One::open(&d).is_err(), "no file");
        std::fs::write(d.join(NAME), vec![0u8; 8192]).unwrap();
        let e = One::open(&d).err().unwrap();
        assert!(e.to_string().contains("no supdb data file"), "{e}");
        One::create(&d).unwrap();
        One::open(&d).unwrap();
    }

    #[test]
    fn a_mapped_file_is_never_moved() {
        let d = scratch("mapped");
        let one = One::create(&d).unwrap();
        one.tight.store(true, Ordering::Relaxed);
        let f = one.make("zpg-1.zpg").unwrap();
        f.write_all_at(&bytes(8192, 15), 0).unwrap();
        put(&one, "ord-1.oidx", &bytes(8192, 16));
        let m = f.map().unwrap();
        let e = f.write_all_at(&bytes(8192, 17), 8192).unwrap_err();
        assert!(e.to_string().contains("under a mapping"), "{e}");
        assert_eq!(m.slice_at(0, 8192).unwrap(), &bytes(8192, 15)[..]);
    }

    #[test]
    fn handles_cloned_share_a_position_and_handles_reopened_do_not() {
        let d = scratch("pos");
        let one = One::create(&d).unwrap();
        let f = one.make("ord-1.oidx").unwrap();
        assert_eq!(f.write(&[1, 2, 3]).unwrap(), 3);
        let g = f.try_clone().unwrap();
        assert_eq!(g.write(&[4, 5]).unwrap(), 2);
        assert_eq!(f.seek(std::io::SeekFrom::Current(0)).unwrap(), 5);
        let r = f.reopen().unwrap();
        let mut buf = [0u8; 8];
        assert_eq!(r.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], &[1, 2, 3, 4, 5]);
        assert_eq!(f.seek(std::io::SeekFrom::End(-2)).unwrap(), 3);
        assert_eq!(g.read(&mut buf).unwrap(), 2);
        // The range an advice or a writeback is made over, where the
        // platform has those calls.
        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                f.range(0, 0).1,
                4096,
                "a length of nothing is the file's, a page whole"
            );
            assert_eq!(f.range(1 << 30, 5).1, 0, "and nothing past it");
        }
    }
}
