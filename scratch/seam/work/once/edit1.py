import sys
p = '/home/user/supdb/src/db/pager.rs'
s = open(p).read()


def rep(old, new, count=1):
    global s
    assert s.count(old) == count, (old, s.count(old))
    s = s.replace(old, new)


rep("""    breaths: AtomicU64,
    adopted_bytes: AtomicU64,
}""", """    breaths: AtomicU64,
    adopted_bytes: AtomicU64,
    // SCRATCH(once)
    direct_reads: AtomicU64,
    direct_bytes: AtomicU64,
    direct_us: AtomicU64,
    run_chunks: AtomicU64,
    nowait_hits: AtomicU64,
    limit_lo: AtomicU64,
    limit_hi: AtomicU64,
}""")
rep("""    pub breaths: u64,
}""", """    pub breaths: u64,
    // SCRATCH(once): preads of the envelope past the page cache, their
    // bytes and wall time, chunks filled by a pread of more than one,
    // runs served by RWF_NOWAIT, and the dynamic budget's range.
    pub direct_reads: u64,
    pub direct_bytes: u64,
    pub direct_us: u64,
    pub run_chunks: u64,
    pub nowait_hits: u64,
    pub limit_lo: u64,
    pub limit_hi: u64,
}""")
rep("""pub(crate) struct Pager {
    /// Where the cache is `crowded`: its budget and the slack past it,
    /// worked out once, since every read of the store asks.
    hard: u64,
    /// Where it is `swamped`.
    swamp: u64,
    limit: AtomicU64,""", """pub(crate) struct Pager {
    // SCRATCH(once): `hard` and `swamp` derived from the current limit;
    // a unit's bytes, kept so the derivation asks no page size.
    unit_bytes: u64,
    /// SCRATCH(once): 0 today, 1 a miss read past the page cache
    /// (O_DIRECT), 2 RWF_NOWAIT first and O_DIRECT on EAGAIN.
    once: u8,
    /// SCRATCH(once): the budget recomputed as this percentage of
    /// `zfile::read_budget().budget`, every 100 ms; 0 fixed.
    dyn_pct: u8,
    trace: bool,
    limit: AtomicU64,""")
rep("""    pub(crate) fn new(limit: u64) -> Arc<Pager> {
        let unit = (unit_chunks() * CHUNK) as u64;
        let p = Arc::new(Pager {
            hard: limit.saturating_add((limit / 16).max(4 * unit)),
            swamp: limit.saturating_add((limit / 2).max(6 * unit)),
            told""", """    pub(crate) fn new(limit: u64) -> Arc<Pager> {
        Pager::new_with(limit, 0, 0)
    }

    /// SCRATCH(once): `new` with the miss path and the budget's mode.
    pub(crate) fn new_with(limit: u64, once: u8, dyn_pct: u8) -> Arc<Pager> {
        let unit = (unit_chunks() * CHUNK) as u64;
        let once = match (once, std::env::var("SUPDB_ONCE_FORCE").ok().as_deref()) {
            (0, Some("d")) => 1,
            (0, Some("dn")) => 2,
            (o, _) => o,
        };
        let p = Arc::new(Pager {
            unit_bytes: unit,
            once,
            dyn_pct,
            trace: std::env::var_os("SUPDB_PAGER_TRACE").is_some_and(|v| v == "1"),
            told""")
rep("""            counts: Counts::default(),
        });
        let weak = Arc::downgrade(&p);""", """            counts: Counts::default(),
        });
        p.counts.limit_lo.store(limit, Ordering::Relaxed);
        p.counts.limit_hi.store(limit, Ordering::Relaxed);
        let weak = Arc::downgrade(&p);""")
rep("""            breaths: r(&c.breaths),
        }""", """            breaths: r(&c.breaths),
            direct_reads: r(&c.direct_reads),
            direct_bytes: r(&c.direct_bytes),
            direct_us: r(&c.direct_us),
            run_chunks: r(&c.run_chunks),
            nowait_hits: r(&c.nowait_hits),
            limit_lo: r(&c.limit_lo),
            limit_hi: r(&c.limit_hi),
        }""")
rep("""        self.could_give() > self.hard
    }""", """        let l = self.budget();
        self.could_give() > l.saturating_add((l / 16).max(4 * self.unit_bytes))
    }""")
rep("""        self.could_give() > self.swamp
    }""", """        let l = self.budget();
        self.could_give() > l.saturating_add((l / 2).max(6 * self.unit_bytes))
    }""")
rep("""    fn run(me: Weak<Pager>) {
        loop {
            let Some(p) = me.upgrade() else {
                return;
            };
            if p.stop.load(Ordering::SeqCst) {
                return;
            }""", """    fn run(me: Weak<Pager>) {
        let mut last_dyn = std::time::Instant::now() - Duration::from_secs(1);
        let mut last_trace = std::time::Instant::now();
        loop {
            let Some(p) = me.upgrade() else {
                return;
            };
            if p.stop.load(Ordering::SeqCst) {
                return;
            }
            // SCRATCH(once): the budget followed, and the trace.
            #[cfg(target_os = "linux")]
            if p.dyn_pct > 0 && last_dyn.elapsed() >= Duration::from_millis(100) {
                last_dyn = std::time::Instant::now();
                if let Some(b) = super::zfile::read_budget() {
                    let l = (b.budget / 100 * u64::from(p.dyn_pct)).max(64 << 20);
                    p.limit.store(l, Ordering::Relaxed);
                    p.counts.limit_lo.fetch_min(l, Ordering::Relaxed);
                    p.counts.limit_hi.fetch_max(l, Ordering::Relaxed);
                }
            }
            if p.trace && last_trace.elapsed() >= Duration::from_secs(1) {
                last_trace = std::time::Instant::now();
                let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                // SAFETY: a clock read into a local.
                unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
                let c = p.counts();
                eprintln!(
                    "pagertrace {}.{:09} held {} limit {} unbacked {} fills {} evicted {} direct {} runchunks {} nowait {}",
                    ts.tv_sec, ts.tv_nsec, c.held, c.budget,
                    p.unbacked.load(Ordering::Relaxed), c.fills, c.evicted_units,
                    c.direct_reads, c.run_chunks, c.nowait_hits
                );
            }""")
rep("""    static PINS: Cell<u32> = const { Cell::new(0) };
}""", """    static PINS: Cell<u32> = const { Cell::new(0) };
    /// SCRATCH(once): holds this thread has, for the run a fill reads
    /// ahead under one.
    static HOLDS: Cell<u32> = const { Cell::new(0) };
    /// SCRATCH(once): the chunks of a file a reader on this thread is
    /// about to read: (the file's address, first, last).
    static HINT: Cell<(usize, usize, usize)> = const { Cell::new((0, 0, 0)) };
    /// SCRATCH(once): a page-aligned buffer for one run.
    static RUNBUF: std::cell::RefCell<Option<RunBuf>> = const { std::cell::RefCell::new(None) };
}

/// SCRATCH(once): the most chunks one pread reads.
const RUN_MAX: usize = 32;

/// SCRATCH(once): RUN_MAX pages, aligned to a page, for O_DIRECT.
struct RunBuf(*mut u8);

impl RunBuf {
    fn layout() -> std::alloc::Layout {
        std::alloc::Layout::from_size_align(RUN_MAX << 12, 4096).expect("a run's layout")
    }
    fn new() -> RunBuf {
        // SAFETY: a layout of nonzero size.
        let p = unsafe { std::alloc::alloc_zeroed(RunBuf::layout()) };
        assert!(!p.is_null(), "a run buffer");
        RunBuf(p)
    }
}

impl Drop for RunBuf {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout.
        unsafe { std::alloc::dealloc(self.0, RunBuf::layout()) };
    }
}""")
rep("""        h.pin();
        debug_pinned(true);
        h""", """        h.pin();
        debug_pinned(true);
        let _ = HOLDS.try_with(|c| c.set(c.get() + 1));
        h""")
rep("""impl Drop for Hold {
    fn drop(&mut self) {
        debug_pinned(false);""", """impl Drop for Hold {
    fn drop(&mut self) {
        debug_pinned(false);
        let _ = HOLDS.try_with(|c| c.set(c.get().saturating_sub(1)));""")
rep("""    path: std::path::PathBuf,
}

impl Back {""", """    path: std::path::PathBuf,
    /// SCRATCH(once): the envelope opened past the page cache, at the
    /// first fill that reads it so, and opened buffered for the probe.
    direct: OnceLock<File>,
    buffered: OnceLock<File>,
}

impl Back {""")
rep("""            aead,
            path: path.to_path_buf(),
        })""", """            aead,
            path: path.to_path_buf(),
            direct: OnceLock::new(),
            buffered: OnceLock::new(),
        })""")

# fill -> fill_from
rep("""    /// Chunk `c` decrypted into its place. The caller moved it to `BUSY`.
    fn fill(&self, c: usize) -> Result<()> {
        let back = self.back.get().ok_or_else(|| {""", """    /// Chunk `c` decrypted into its place. The caller moved it to `BUSY`.
    fn fill(&self, c: usize) -> Result<()> {
        let back = self.back()?;
        let n = self.chunk_len(c);
        let at = back.hdr.stored_off(c as u64) as usize;
        let stored = back.map.get(at..at + n + TAG).ok_or_else(|| {
            Error::new(
                ErrorKind::UnexpectedEof,
                format!(
                    "{}: its envelope is shorter than its header says",
                    self.label
                ),
            )
        })?;
        self.fill_from(c, stored)
    }

    fn back(&self) -> Result<&Back> {
        self.back.get().ok_or_else(|| {""")
rep("""                    self.label
                ),
            )
        })?;
        let n = self.chunk_len(c);
        let at = back.hdr.stored_off(c as u64) as usize;
        let stored = back.map.get(at..at + n + TAG).ok_or_else(|| {
            Error::new(
                ErrorKind::UnexpectedEof,
                format!(
                    "{}: its envelope is shorter than its header says",
                    self.label
                ),
            )
        })?;
        // SAFETY""", """                    self.label
                ),
            )
        })
    }

    /// SCRATCH(once): chunk `c` decrypted into its place from its stored
    /// bytes, its plaintext and then its tag.
    fn fill_from(&self, c: usize, stored: &[u8]) -> Result<()> {
        let back = self.back()?;
        let n = self.chunk_len(c);
        if stored.len() < n + TAG {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                format!("{}: chunk {c}'s stored bytes are short", self.label),
            ));
        }
        let stored = &stored[..n + TAG];
        // SAFETY""")

# slow EMPTY arm
rep("""                    if !self.rescue(unit) {
                        state.store(EMPTY, Ordering::SeqCst);
                        self.gone(unit);
                        continue;
                    }
                    match self.fill(c) {""", """                    if self.pager.once != 0 {
                        match self.fill_run(c, unit) {
                            Some(ok) => return ok,
                            None => continue,
                        }
                    }
                    if !self.rescue(unit) {
                        state.store(EMPTY, Ordering::SeqCst);
                        self.gone(unit);
                        continue;
                    }
                    match self.fill(c) {""")

# fill_run inserted before pause_after_rescue
rep("""    /// A test's: a reader that has taken a unit back held there, before it
    /// says its chunk is lent, until the test lets it go.""", """    /// SCRATCH(once): chunk `c`, which this thread moved to `BUSY`, and
    /// the run after it this thread is about to read, filled from one
    /// pread of the envelope past the page cache. `None` where the unit's
    /// pages were going and the caller looks again.
    #[cold]
    fn fill_run(&self, c: usize, unit: &AtomicU8) -> Option<bool> {
        let uc = self.unit_chunks;
        let unit_last = ((c / uc + 1) * uc - 1).min(self.state.len() - 1);
        let me = self as *const Paged as usize;
        let (hp, ha, hb) = HINT.with(|h| h.get());
        let mut lim = if hp == me && ha <= c && c <= hb {
            hb
        } else if HOLDS.with(|h| h.get()) > 0 {
            c + RUN_MAX - 1
        } else {
            c
        };
        lim = lim.min(unit_last).min(c + RUN_MAX - 1);
        let mut last = c;
        while last < lim
            && self.state[last + 1]
                .compare_exchange(EMPTY, BUSY, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            last += 1;
        }
        let put_back = |from: usize| {
            for s in &self.state[from..=last] {
                s.store(EMPTY, Ordering::SeqCst);
            }
        };
        if !self.rescue(unit) {
            put_back(c);
            self.gone(unit);
            return None;
        }
        let res = self.read_run(c, last);
        let got = match res {
            Ok(n) => n,
            Err(e) => {
                put_back(c);
                self.pager.poison(&e.to_string());
                return Some(false);
            }
        };
        let filled = RUNBUF.with(|b| {
            let b = b.borrow();
            let buf = b.as_ref().expect("a run buffer read into");
            // SAFETY: `got` bytes of the buffer were read into.
            let buf = unsafe { std::slice::from_raw_parts(buf.0, got) };
            for i in c..=last {
                let off = (i - c) << 12;
                let r = buf
                    .get(off..)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::UnexpectedEof,
                            format!("{}: a run read short", self.label),
                        )
                    })
                    .and_then(|s| self.fill_from(i, s));
                if let Err(e) = r {
                    return Err((i, e));
                }
                self.present.fetch_add(1, Ordering::Relaxed);
                self.state[i].store(COLD, Ordering::SeqCst);
                Counts::bump(&self.pager.counts.fills);
                self.pager.add(self.chunk_len(i) as u64);
            }
            Ok(())
        });
        if last > c {
            self.pager
                .counts
                .run_chunks
                .fetch_add((last - c + 1) as u64, Ordering::Relaxed);
        }
        match filled {
            Ok(()) => Some(true),
            Err((i, e)) => {
                put_back(i);
                self.pager.poison(&e.to_string());
                Some(i > c)
            }
        }
    }

    /// SCRATCH(once): chunks `c..=last`'s stored pages read into the
    /// thread's run buffer: RWF_NOWAIT first under DN, past the page cache
    /// otherwise. The bytes read.
    fn read_run(&self, c: usize, last: usize) -> Result<usize> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let back = self.back()?;
        let at = back.hdr.stored_off(c as u64);
        let want = (last - c + 1) << 12;
        let need = ((last - c) << 12) + self.chunk_len(last) + TAG;
        let open = |flags: i32| -> Result<File> {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags)
                .open(&back.path)
        };
        RUNBUF.with(|b| {
            let mut b = b.borrow_mut();
            let buf = b.get_or_insert_with(RunBuf::new).0;
            if self.pager.once == 2 {
                if back.buffered.get().is_none() {
                    let _ = back.buffered.set(open(0)?);
                }
                let fd = back.buffered.get().expect("opened").as_raw_fd();
                let iov = libc::iovec {
                    iov_base: buf.cast(),
                    iov_len: want,
                };
                // SAFETY: one iovec over the thread's buffer of RUN_MAX pages.
                let r = unsafe { libc::preadv2(fd, &iov, 1, at as i64, libc::RWF_NOWAIT) };
                if r >= 0 && r as usize >= need {
                    Counts::bump(&self.pager.counts.nowait_hits);
                    return Ok(r as usize);
                }
            }
            if back.direct.get().is_none() {
                // A filesystem that refuses O_DIRECT (a tmpfs) is read buffered.
                let f = open(libc::O_DIRECT).or_else(|_| open(0))?;
                let _ = back.direct.set(f);
            }
            let fd = back.direct.get().expect("opened").as_raw_fd();
            let t = std::time::Instant::now();
            let mut got = 0usize;
            while got < need {
                // SAFETY: the rest of the thread's buffer, aligned at a page.
                let r = unsafe {
                    libc::pread(fd, buf.add(got).cast(), want - got, (at + got as u64) as i64)
                };
                if r < 0 {
                    let e = Error::last_os_error();
                    if e.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                if r == 0 {
                    return Err(Error::new(
                        ErrorKind::UnexpectedEof,
                        format!("{}: its envelope is shorter than its header says", self.label),
                    ));
                }
                got += r as usize;
                Counts::bump(&self.pager.counts.direct_reads);
            }
            let c = &self.pager.counts;
            c.direct_bytes.fetch_add(got as u64, Ordering::Relaxed);
            c.direct_us
                .fetch_add(t.elapsed().as_micros() as u64, Ordering::Relaxed);
            Ok(got)
        })
    }

    /// A test's: a reader that has taken a unit back held there, before it
    /// says its chunk is lent, until the test lets it go.""")

# ensure multi-chunk hint
rep("""        let b = ((off + len as u64 - 1) / CHUNK as u64) as usize;
        if b >= self.state.len() {
            return false;
        }
        (a..=b).all(""", """        let b = ((off + len as u64 - 1) / CHUNK as u64) as usize;
        if b >= self.state.len() {
            return false;
        }
        if self.pager.once != 0 {
            HINT.with(|h| h.set((self as *const Paged as usize, a, b)));
        }
        (a..=b).all(""")
# willneed
rep("""        if (a..=b).all(|c| matches!(self.state[c].load(Ordering::Relaxed), HOT | COLD)) {
            return 0;
        }
        let mut at = from;""", """        if (a..=b).all(|c| matches!(self.state[c].load(Ordering::Relaxed), HOT | COLD)) {
            return 0;
        }
        if self.pager.once != 0 {
            // SCRATCH(once): the range is the run a fill reads, not an
            // advice that fills the page cache with ciphertext.
            HINT.with(|h| h.set((self as *const Paged as usize, a, b)));
            return 0;
        }
        let mut at = from;""")
open(p, 'w').write(s)
print("ok")
