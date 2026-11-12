//! The memory bound a bounded run measures under: a cgroup made for each
//! pass, with a limit the store cannot cache past, holding the pass's own
//! process and nothing else.
//!
//! Every banked row is in-core: the full scale's top, the rung at 1.5x the
//! machine's memory, was never reached, and the quick ladder fits the
//! page cache a hundred times over. A bound is how the out-of-core regime
//! is reached on a machine whose memory is not the store's size -- the
//! kernel reclaims the group's page cache at the limit, so a store past it
//! reads from the device, which is the regime the `ooc` scale measures.
//! The bound is the row's class as much as the memory is (`Row::class`).
//!
//! The bound is the pass's: the engine and the workload driving it, in the
//! process `run` and `ab` give each pass (`run::pass_in_child`), which
//! joins a group of its own before it runs anything and is the only
//! process in it. The run's process -- its floors, its ledger, its samples
//! -- stays out, so that what it holds comes out of no pass's bound, and
//! each pass's group is made for it and removed after it, so that no pass
//! starts with what an earlier one left charged; each pass records what
//! its group held at its start (`Bound::usage`), which is the check that
//! it was so. Under cgroup v1 -- this host, with the memory controller
//! mounted read-write -- the run makes a group of its own under the one it
//! was started in, never enters it, and makes the pass groups in it
//! (`create_at`). Under cgroup v2 -- GitHub's hosted runners -- a group may
//! hand the memory controller to the groups under it only while no process
//! is in it, so the run is started in a delegated, unlimited service
//! (`systemd-run -p Delegate=yes`), moves itself into a leaf of it and
//! makes the pass groups beside that leaf (`delegate_at`). A v2 group that
//! is not delegated and has the limit on it is the one shape where the
//! passes share the run's group (`attach_at`, `Mode::Shared`), held to the
//! bound the run was asked for: there the run's process holds as little as
//! it can, and each pass's start says what it held. Either way a run that
//! cannot be bounded refuses, naming the file, rather than measuring an
//! unbounded run as a bounded one. The parsers are pure functions over the
//! files' text so that a host without the controller can still test them,
//! and the groups' steps run against a directory of plain files.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// The group's name under the parent, with the pid so that a run that
/// died leaves a name its successor recognises as stale and removes.
const PREFIX: &str = "supdb-bench-";

/// The memory controller's path for this process from `/proc/self/cgroup`
/// text: the v1 line `N:memory:/path`, or one naming `memory` among
/// several controllers (`N:cpu,memory:/path`). A v2-only host has only
/// the `0::/path` line, which names no controller, so it is `None` here:
/// the run must not take the unified hierarchy for a memory controller it
/// cannot write a limit into.
pub fn parse_cgroup_path(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut it = line.splitn(3, ':');
        let (Some(_id), Some(controllers), Some(path)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if controllers.split(',').any(|c| c == "memory") {
            return Some(path.to_string());
        }
    }
    None
}

/// Where the v1 memory controller is mounted, from `/proc/mounts` text:
/// the first mount of type `cgroup` whose options name `memory`. The
/// mount point is the second field; a path with a space in it is escaped
/// there as `\040`, which no cgroup mount has, so the field is taken as
/// is.
pub fn parse_mount(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 || f[2] != "cgroup" {
            continue;
        }
        if f[3].split(',').any(|o| o == "memory") {
            return Some(PathBuf::from(f[1]));
        }
    }
    None
}

/// The unified hierarchy's path for this process from `/proc/self/cgroup`
/// text: the `0::/path` line, which every host with cgroup2 mounted has,
/// v2-only or hybrid. It is the group to attach to only where no v1
/// memory controller line exists (`Bound::enter`).
pub fn parse_unified_path(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("0::").map(str::to_string))
}

/// Where the unified hierarchy is mounted, from `/proc/mounts` text: the
/// first mount of type `cgroup2`.
pub fn parse_unified_mount(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 3 && f[2] == "cgroup2" {
            return Some(PathBuf::from(f[1]));
        }
    }
    None
}

/// What the group's `memory.stat` and its two counters say, as read.
/// Every field is the file's own name, so a reader of a row's stderr can
/// look each up in the kernel's documentation under that name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoundStat {
    pub cache: u64,
    pub rss: u64,
    pub mapped_file: u64,
    pub dirty: u64,
    pub writeback: u64,
    pub pgfault: u64,
    pub pgmajfault: u64,
    /// File pages the group faulted back in after reclaiming them: the
    /// count that says the bound was reached, and the one a bounded row
    /// records per operation (`refaults_per_op`).
    pub workingset_refault_file: u64,
    pub inactive_file: u64,
    pub active_file: u64,
    pub max_usage_in_bytes: u64,
    pub failcnt: u64,
}

/// v1's `memory.stat` text into the fields above. The file carries each
/// key twice, plain and as `total_`; the plain one is this group's own,
/// which is what the run is.
pub fn parse_stat(text: &str) -> BoundStat {
    let mut s = BoundStat::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        let v: u64 = v.parse().unwrap_or(0);
        match k {
            "cache" => s.cache = v,
            "rss" => s.rss = v,
            "mapped_file" => s.mapped_file = v,
            "dirty" => s.dirty = v,
            "writeback" => s.writeback = v,
            "pgfault" => s.pgfault = v,
            "pgmajfault" => s.pgmajfault = v,
            "workingset_refault_file" => s.workingset_refault_file = v,
            "inactive_file" => s.inactive_file = v,
            "active_file" => s.active_file = v,
            _ => {}
        }
    }
    s
}

/// v2's `memory.stat` text into the same fields, under the names v1 gave
/// them: `anon` is `rss`, `file` is `cache`, and `file_mapped`,
/// `file_dirty` and `file_writeback` are the three without the prefix;
/// the faults, the refaults and the two file lists share their names.
pub fn parse_stat_v2(text: &str) -> BoundStat {
    let mut s = BoundStat::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(k), Some(v)) = (it.next(), it.next()) else {
            continue;
        };
        let v: u64 = v.parse().unwrap_or(0);
        match k {
            "file" => s.cache = v,
            "anon" => s.rss = v,
            "file_mapped" => s.mapped_file = v,
            "file_dirty" => s.dirty = v,
            "file_writeback" => s.writeback = v,
            "pgfault" => s.pgfault = v,
            "pgmajfault" => s.pgmajfault = v,
            "workingset_refault_file" => s.workingset_refault_file = v,
            "inactive_file" => s.inactive_file = v,
            "active_file" => s.active_file = v,
            _ => {}
        }
    }
    s
}

/// v2's `memory.events` text: the `max` line, the times the group's
/// memory reached its limit and was reclaimed, which is what v1's
/// `failcnt` counts.
pub fn parse_events_max(text: &str) -> u64 {
    text.lines()
        .find_map(|line| {
            let mut it = line.split_whitespace();
            match (it.next(), it.next()) {
                (Some("max"), Some(v)) => v.parse().ok(),
                _ => None,
            }
        })
        .unwrap_or(0)
}

/// How a run's passes are bounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Each pass's process in a group of its own, made for the pass with the
    /// limit on it, joined by the pass's process before it runs anything
    /// (`PassGroup::join_before_exec`), and removed after it; the run's own
    /// process in none of them, so nothing it holds -- the floors, the
    /// ledger, the samples -- comes out of a pass's bound, and no pass
    /// starts with what an earlier one left charged.
    #[default]
    PerPass,
    /// The run and its passes in the one group a wrapper bounded: the
    /// cgroup v2 group of a `systemd-run` service that was not delegated
    /// (`attach_at`), where no process may make a group of its own. The
    /// run's process is inside the bound beside every pass, so it holds as
    /// little as it can there (`run::start_floors`), and each pass records
    /// what the group held at its start (`Bound::usage`).
    Shared,
}

/// The bound a run measures under, or the one group a pass measures in.
/// The run's: a group under the one the process was started in, which the
/// process never enters and in which each pass is given a group of its own
/// (`Bound::pass`); dropping it removes the groups it made. A pass's, as
/// its spec hands it to the pass's process (`PassGroup::bound`): attached
/// and not owned, read for the pass's counts and its start, never removed
/// by the reader. A clone is attached and not owned, as a deserialized
/// bound is.
#[derive(Serialize, Deserialize)]
pub struct Bound {
    /// The run's group, or the pass's, under the controller's mount.
    group: PathBuf,
    /// The group the process was started in, under which the run's group
    /// was made (v1) or which is the run's group (v2).
    parent: PathBuf,
    /// The limit as the kernel reports it after the write: v1 rounds the
    /// number written down to a page, and the row records what holds.
    limit: u64,
    /// Whether this process made the group and removes it at the drop.
    #[serde(skip)]
    owned: bool,
    /// Whether the group is cgroup v2's, read through its files
    /// (`memory.max`, `memory.current`, `memory.stat`'s v2 keys,
    /// `memory.peak`, `memory.events`), as against v1's.
    #[serde(default)]
    v2: bool,
    /// How the run's passes are bounded (`Mode`).
    #[serde(default)]
    mode: Mode,
}

/// The group one pass's process runs in (`Bound::pass`): made with the
/// limit on it and removed at the drop, after the process it held has
/// exited; or, under `Mode::Shared`, the run's own group, which neither is
/// made nor removed here.
pub struct PassGroup {
    group: PathBuf,
    /// The file the pass's process writes its own pid to before it runs
    /// anything, as a NUL-terminated path for the write between fork and
    /// exec; none where the process inherits the run's group.
    procs: Option<std::ffi::CString>,
    owned: bool,
    limit: u64,
    v2: bool,
    mode: Mode,
}

fn err_at(path: &Path, what: &str, e: io::Error) -> io::Error {
    io::Error::new(
        e.kind(),
        format!(
            "{what} {}: {e}{}",
            path.display(),
            e.raw_os_error()
                .map(|n| format!(" (errno {n})"))
                .unwrap_or_default()
        ),
    )
}

fn write_to(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, text).map_err(|e| err_at(path, "could not write", e))
}

fn read_from(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path).map_err(|e| err_at(path, "could not read", e))
}

fn read_u64(path: &Path) -> io::Result<u64> {
    let text = read_from(path)?;
    text.trim().parse().map_err(|_| {
        io::Error::other(format!(
            "could not parse {} as a number: {:?}",
            path.display(),
            text.trim()
        ))
    })
}

/// A group and every group under it, removed from the bottom up: a cgroup
/// directory goes only once the groups under it have, and one with a
/// process in it refuses with `EBUSY`, which is reported, never forced.
/// Only directories are walked; the files in a cgroup directory are the
/// kernel's and go with it.
fn remove_tree(group: &Path) -> io::Result<()> {
    if let Ok(rd) = std::fs::read_dir(group) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                remove_tree(&e.path())?;
            }
        }
    }
    std::fs::remove_dir(group)
}

impl Bound {
    /// The run's bound, `limit` bytes for each of its passes. Under cgroup
    /// v1 (this host) a group of the run's own is made under the one this
    /// process was started in, and the process stays out of it
    /// (`create_at`). Under cgroup v2 the group this process was started in
    /// is the run's: delegated and unlimited -- `systemd-run -p
    /// Delegate=yes`, as the `ooc-hosted` workflow starts it -- the process
    /// moves itself into a leaf of it so that the group may hand the memory
    /// controller to the passes' groups beside it (`delegate_at`); limited
    /// to the bound and not delegated, the passes share it with this process
    /// (`attach_at`, `Mode::Shared`). A first pass group is made and
    /// removed here, so that a limit the kernel will not take refuses
    /// before anything is measured. The error names the file and the errno
    /// of whichever step refused.
    pub fn enter(limit: u64) -> io::Result<Bound> {
        let cgroup = Path::new("/proc/self/cgroup");
        let text = read_from(cgroup)?;
        let mounts = Path::new("/proc/mounts");
        let b = match parse_cgroup_path(&text) {
            Some(own) => {
                let root = parse_mount(&read_from(mounts)?).ok_or_else(|| {
                    io::Error::other(format!(
                        "no cgroup mount with the memory controller in {}",
                        mounts.display()
                    ))
                })?;
                Bound::create_at(&root, &own, limit, std::process::id())?
            }
            None => {
                let own = parse_unified_path(&text).ok_or_else(|| {
                    io::Error::other(format!(
                        "no memory controller and no unified hierarchy in {}",
                        cgroup.display()
                    ))
                })?;
                let root = parse_unified_mount(&read_from(mounts)?).ok_or_else(|| {
                    io::Error::other(format!("no cgroup2 mount in {}", mounts.display()))
                })?;
                let group = root.join(own.trim_start_matches('/'));
                let max = read_from(&group.join("memory.max"))?;
                if max.trim() == "max" {
                    Bound::delegate_at(&root, &own, limit)?
                } else {
                    Bound::attach_at(&root, &own, limit)?
                }
            }
        };
        drop(b.pass(0)?);
        Ok(b)
    }

    /// `enter` under v1, against a controller mounted at `root`, for a
    /// process in the group `own` under it: the run's group made under
    /// `own`, with no limit of its own and no process in it. Groups of
    /// earlier runs that died are removed with the pass groups under them;
    /// one that still has a process in it is a run that has not died, and
    /// two timing runs may not share a machine, so it is reported rather
    /// than measured beside. A test drives this against a directory of
    /// plain files.
    pub fn create_at(root: &Path, own: &str, limit: u64, pid: u32) -> io::Result<Bound> {
        let parent = root.join(own.trim_start_matches('/'));
        if !parent.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("the controller has no group at {}", parent.display()),
            ));
        }
        let mine = format!("{PREFIX}{pid}");
        if let Ok(rd) = std::fs::read_dir(&parent) {
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with(PREFIX) || name == mine || !e.path().is_dir() {
                    continue;
                }
                if let Err(err) = remove_tree(&e.path()) {
                    return Err(err_at(
                        &e.path(),
                        "a stale group refuses removal (tasks still in it?):",
                        err,
                    ));
                }
            }
        }
        let group = parent.join(&mine);
        match std::fs::create_dir(&group) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // An earlier run of this pid that died: its pass groups go.
                if let Ok(rd) = std::fs::read_dir(&group) {
                    for e in rd.flatten() {
                        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            remove_tree(&e.path()).map_err(|err| {
                                err_at(&e.path(), "a stale pass group refuses removal:", err)
                            })?;
                        }
                    }
                }
            }
            Err(e) => return Err(err_at(&group, "could not make the group", e)),
        }
        Ok(Bound {
            group,
            parent,
            limit: limit / page_size() * page_size(),
            owned: true,
            v2: false,
            mode: Mode::PerPass,
        })
    }

    /// `enter` under v2 in a delegated, unlimited group, `own` under the
    /// unified mount at `root`: every process in the group is moved into a
    /// leaf of it, `harness`, since v2 lets a group hand a controller to
    /// the groups under it only while no process is in it; the memory
    /// controller is then handed down, and each pass is given a leaf beside
    /// `harness` with the limit on it (`pass`). Nothing here is removed at
    /// the drop: the group and its leaves are the service's, which go with
    /// it.
    pub fn delegate_at(root: &Path, own: &str, limit: u64) -> io::Result<Bound> {
        let group = root.join(own.trim_start_matches('/'));
        let harness = group.join("harness");
        match std::fs::create_dir(&harness) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && harness.is_dir() => {}
            Err(e) => {
                return Err(err_at(
                    &harness,
                    "could not make the harness's leaf (is the group delegated? systemd-run -p \
                     Delegate=yes):",
                    e,
                ))
            }
        }
        let procs = read_from(&group.join("cgroup.procs"))?;
        for pid in procs.split_whitespace() {
            write_to(&harness.join("cgroup.procs"), pid)?;
        }
        write_to(&group.join("cgroup.subtree_control"), "+memory")?;
        Ok(Bound {
            parent: group.clone(),
            group,
            limit,
            owned: false,
            v2: true,
            mode: Mode::PerPass,
        })
    }

    /// The bound as cgroup v2 gives it without delegation: the group this
    /// process was started in, `own` under the unified mount at `root`,
    /// whose `memory.max` is held to `limit` -- `max` is no bound, and
    /// another number is another row's class -- and whose `memory.swap.max`,
    /// where the kernel accounts swap, is held to zero, since a group that
    /// can swap its anonymous memory out is in another regime from this
    /// host's, which has none. The wrapper that starts the run sets both
    /// (`systemd-run -p MemoryMax=BYTES -p MemorySwapMax=0`), and the passes
    /// share the group with this process (`Mode::Shared`): the bound is
    /// attached, never owned.
    pub fn attach_at(root: &Path, own: &str, limit: u64) -> io::Result<Bound> {
        let group = root.join(own.trim_start_matches('/'));
        let max_file = group.join("memory.max");
        let text = read_from(&max_file)?;
        let got = text.trim();
        if got == "max" {
            return Err(io::Error::other(format!(
                "{} is \"max\": the group this process runs in has no limit, and on cgroup v2 \
                 the run is bounded by whoever starts it (systemd-run -p MemoryMax={limit} -p \
                 MemorySwapMax=0, or -p Delegate=yes for a group of each pass's own)",
                max_file.display()
            )));
        }
        let got: u64 = got.parse().map_err(|_| {
            io::Error::other(format!(
                "could not parse {} as a number: {got:?}",
                max_file.display()
            ))
        })?;
        if got != limit {
            return Err(io::Error::other(format!(
                "{} is {got} and --bound is {limit}: the group's limit and the row's bound have \
                 to be one number",
                max_file.display()
            )));
        }
        let swap_file = group.join("memory.swap.max");
        if let Ok(swap) = std::fs::read_to_string(&swap_file) {
            if swap.trim() != "0" {
                return Err(io::Error::other(format!(
                    "{} is {:?}: a group that can swap is not this bound (systemd-run -p \
                     MemorySwapMax=0)",
                    swap_file.display(),
                    swap.trim()
                )));
            }
        }
        Ok(Bound {
            parent: group.clone(),
            group,
            limit: got,
            owned: false,
            v2: true,
            mode: Mode::Shared,
        })
    }

    /// The group pass `n` runs in. `Mode::PerPass`: a group of its own under
    /// the run's, `pass-<n>`, made here with the limit on it (and, under v2,
    /// no swap and the whole group killed together at the limit), and
    /// removed at the `PassGroup`'s drop. `Mode::Shared`: the run's group.
    pub fn pass(&self, n: u64) -> io::Result<PassGroup> {
        if self.mode == Mode::Shared {
            return Ok(PassGroup {
                group: self.group.clone(),
                procs: None,
                owned: false,
                limit: self.limit,
                v2: self.v2,
                mode: Mode::Shared,
            });
        }
        let group = self.group.join(format!("pass-{n}"));
        if group.is_dir() {
            remove_tree(&group)
                .map_err(|e| err_at(&group, "a stale pass group refuses removal:", e))?;
        }
        std::fs::create_dir(&group)
            .map_err(|e| err_at(&group, "could not make the pass's group", e))?;
        // A guard: a step below that refuses removes the group again.
        let mut made = PassGroup {
            procs: None,
            group: group.clone(),
            owned: true,
            limit: self.limit,
            v2: self.v2,
            mode: Mode::PerPass,
        };
        let limit = if self.v2 {
            let max_file = group.join("memory.max");
            write_to(&max_file, &self.limit.to_string())?;
            let got = read_u64(&max_file)?;
            if got != self.limit {
                return Err(io::Error::other(format!(
                    "{} reads back {got} after writing {}",
                    max_file.display(),
                    self.limit
                )));
            }
            let swap = group.join("memory.swap.max");
            if swap.exists() {
                write_to(&swap, "0")?;
            }
            let oom = group.join("memory.oom.group");
            if oom.exists() {
                write_to(&oom, "1")?;
            }
            got
        } else {
            let limit_file = group.join("memory.limit_in_bytes");
            write_to(&limit_file, &self.limit.to_string())?;
            let got = read_u64(&limit_file)?;
            // v1 keeps the limit in pages: the number read back is the number
            // written rounded down to one, and anything else is another file.
            let page = page_size();
            if got != self.limit / page * page {
                return Err(io::Error::other(format!(
                    "{} reads back {got} after writing {}",
                    limit_file.display(),
                    self.limit
                )));
            }
            got
        };
        let procs =
            std::ffi::CString::new(group.join("cgroup.procs").as_os_str().as_encoded_bytes())
                .map_err(io::Error::other)?;
        made.procs = Some(procs);
        made.limit = limit;
        Ok(made)
    }

    /// The limit as read back from the group.
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// The group's directory, for the stderr line a wrapper reads the
    /// group's name from.
    pub fn group(&self) -> &Path {
        &self.group
    }

    /// How the run's passes are bounded.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The bytes the kernel charges the group now (`memory.usage_in_bytes`,
    /// v2's `memory.current`): read by a pass's process at its start, before
    /// the store opens, as the record of what the bound held before the
    /// pass did anything (`run::one_pass_child`).
    pub fn usage(&self) -> io::Result<u64> {
        if self.v2 {
            read_u64(&self.group.join("memory.current"))
        } else {
            read_u64(&self.group.join("memory.usage_in_bytes"))
        }
    }

    /// The group's counters now, or the error naming the file that could
    /// not be read -- never a zero in its place, which would read as a
    /// store that never left the cache. Read before the first operation
    /// of a phase and after its last, never between, as every counter the
    /// suite reads around a phase is.
    pub fn stat(&self) -> io::Result<BoundStat> {
        if self.v2 {
            let mut s = parse_stat_v2(&read_from(&self.group.join("memory.stat"))?);
            s.max_usage_in_bytes = read_u64(&self.group.join("memory.peak"))?;
            s.failcnt = parse_events_max(&read_from(&self.group.join("memory.events"))?);
            return Ok(s);
        }
        let mut s = parse_stat(&read_from(&self.group.join("memory.stat"))?);
        s.max_usage_in_bytes = read_u64(&self.group.join("memory.max_usage_in_bytes"))?;
        s.failcnt = read_u64(&self.group.join("memory.failcnt"))?;
        Ok(s)
    }

    /// Whether this process made the group, as against a bound a pass's
    /// own process was handed (`owned`).
    pub fn is_owned(&self) -> bool {
        self.owned
    }
}

impl Clone for Bound {
    fn clone(&self) -> Bound {
        Bound {
            group: self.group.clone(),
            parent: self.parent.clone(),
            limit: self.limit,
            owned: false,
            v2: self.v2,
            mode: self.mode,
        }
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        // The run's group and any pass group a cut pass left in it. This
        // process was never in any of them, so nothing has to leave first.
        if let Err(e) = remove_tree(&self.group) {
            eprintln!(
                "bound: could not remove {}: {e}; rmdir its pass groups and it by hand",
                self.group.display()
            );
        }
    }
}

impl PassGroup {
    /// The bound the pass's process is handed in its spec: its own group,
    /// attached, for its counts and its start.
    pub fn bound(&self) -> Bound {
        Bound {
            group: self.group.clone(),
            parent: self.group.clone(),
            limit: self.limit,
            owned: false,
            v2: self.v2,
            mode: self.mode,
        }
    }

    /// The group's directory.
    pub fn group(&self) -> &Path {
        &self.group
    }

    /// The group's counters now (`Bound::stat`), for the kill's line after
    /// the pass's process is gone.
    pub fn stat(&self) -> io::Result<BoundStat> {
        self.bound().stat()
    }

    /// Have `cmd`'s process join this group before it runs anything: its
    /// own pid written to the group's `cgroup.procs` between the fork and
    /// the exec, so that not one of its pages is charged anywhere else. A
    /// write that fails fails the spawn, with the errno. Nothing is done
    /// where the process inherits the run's group (`Mode::Shared`).
    #[cfg(unix)]
    pub fn join_before_exec(&self, cmd: &mut std::process::Command) {
        use std::os::unix::process::CommandExt as _;
        let Some(procs) = self.procs.clone() else {
            return;
        };
        // SAFETY: the closure runs in the forked child before the exec and
        // calls only async-signal-safe functions -- getpid, open, write and
        // close -- over a path made before the fork and a buffer on its own
        // stack; it allocates nothing.
        unsafe {
            cmd.pre_exec(move || {
                let mut buf = [0u8; 20];
                let mut n = libc::getpid() as u64;
                let mut i = buf.len();
                loop {
                    i -= 1;
                    buf[i] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
                let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let len = buf.len() - i;
                let w = libc::write(fd, buf.as_ptr().add(i).cast(), len);
                let e = io::Error::last_os_error();
                libc::close(fd);
                if w != len as isize {
                    return Err(e);
                }
                Ok(())
            });
        }
    }
}

impl Drop for PassGroup {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        // The pass's process has exited and been waited for; a group whose
        // last process is just gone can refuse for a moment under v2 while
        // the kernel depopulates it, so the removal is tried a few times
        // before it is reported.
        let mut last = None;
        for _ in 0..50 {
            match std::fs::remove_dir(&self.group) {
                Ok(()) => return,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                Err(e) => last = Some(e),
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Some(e) = last {
            eprintln!(
                "bound: could not remove the pass group {}: {e}; rmdir it by hand",
                self.group.display()
            );
        }
    }
}

fn page_size() -> u64 {
    // SAFETY: a plain query of the system's page size.
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(p).unwrap_or(4096).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = "9:name=systemd:/\n\
8:pids:/\n\
7:blkio:/\n\
4:memory:/process_api/01a14a61-c5c5-710c-a531-2d6bd1173a1a/claude-code-bash\n\
3:cpuset:/\n\
2:cpuacct:/process_api/01a14a61-c5c5-710c-a531-2d6bd1173a1a/cc-shell\n\
1:cpu:/\n\
0::/\n";

    const MOUNTS: &str = "tmpfs /sys/fs/cgroup tmpfs rw,relatime,mode=755 0 0\n\
cgroup /sys/fs/cgroup/cpu cgroup rw,relatime,cpu 0 0\n\
cgroup /sys/fs/cgroup/memory cgroup rw,relatime,memory 0 0\n\
cgroup /sys/fs/cgroup/devices cgroup rw,relatime,devices 0 0\n\
cgroup2 /sys/fs/cgroup/unified cgroup2 rw,relatime 0 0\n";

    #[test]
    fn the_memory_controllers_path_is_read_off_its_own_line() {
        assert_eq!(
            parse_cgroup_path(V1).as_deref(),
            Some("/process_api/01a14a61-c5c5-710c-a531-2d6bd1173a1a/claude-code-bash")
        );
        // Several controllers on one line, memory among them.
        assert_eq!(
            parse_cgroup_path("3:cpu,memory:/a/b\n0::/\n").as_deref(),
            Some("/a/b")
        );
        // A v2-only host names no controller, and a line naming a
        // controller whose name merely contains the word is not it.
        assert_eq!(parse_cgroup_path("0::/user.slice\n"), None);
        assert_eq!(parse_cgroup_path("2:memory_x:/a\n0::/\n"), None);
        assert_eq!(parse_cgroup_path(""), None);
    }

    const V2: &str = "0::/system.slice/run-u1234.service\n";

    const MOUNTS_V2: &str = "sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0\n\
cgroup2 /sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec,relatime,nsdelegate,memory_recursiveprot 0 0\n";

    #[test]
    fn the_unified_path_is_the_zero_line_and_its_mount_the_cgroup2_one() {
        assert_eq!(
            parse_unified_path(V2).as_deref(),
            Some("/system.slice/run-u1234.service")
        );
        // A hybrid host has the line too, at the root; `enter` takes the
        // v1 controller first wherever one is named.
        assert_eq!(parse_unified_path(V1).as_deref(), Some("/"));
        assert_eq!(parse_cgroup_path(V2), None);
        assert_eq!(parse_unified_path(""), None);
        assert_eq!(
            parse_unified_mount(MOUNTS_V2),
            Some(PathBuf::from("/sys/fs/cgroup"))
        );
        assert_eq!(
            parse_unified_mount(MOUNTS),
            Some(PathBuf::from("/sys/fs/cgroup/unified"))
        );
        assert_eq!(parse_unified_mount(""), None);
    }

    #[test]
    fn the_controllers_mount_is_the_cgroup_mount_with_memory_in_its_options() {
        assert_eq!(
            parse_mount(MOUNTS),
            Some(PathBuf::from("/sys/fs/cgroup/memory"))
        );
        // The unified mount is type cgroup2 and names no controller.
        assert_eq!(
            parse_mount("cgroup2 /sys/fs/cgroup/unified cgroup2 rw,relatime 0 0\n"),
            None
        );
        assert_eq!(parse_mount(""), None);
    }

    #[test]
    fn stat_takes_the_groups_own_keys_and_not_the_totals() {
        let s = parse_stat(
            "cache 4096\nrss 8192\nmapped_file 1\ndirty 2\nwriteback 3\npgfault 40\n\
             pgmajfault 5\nworkingset_refault_file 7\ninactive_file 9\nactive_file 11\n\
             total_cache 999999\ntotal_pgmajfault 999\n",
        );
        assert_eq!(s.cache, 4096);
        assert_eq!(s.rss, 8192);
        assert_eq!(s.pgmajfault, 5);
        assert_eq!(s.workingset_refault_file, 7);
        assert_eq!(s.inactive_file, 9);
        assert_eq!(s.active_file, 11);
        assert_eq!(s.max_usage_in_bytes, 0);
    }

    #[test]
    fn the_v2_stat_is_read_under_v1s_names_and_the_events_max_is_the_failcnt() {
        let s = parse_stat_v2(
            "anon 8192\nfile 4096\nkernel 77\nfile_mapped 1\nfile_dirty 2\nfile_writeback 3\n\
             pgfault 40\npgmajfault 5\nworkingset_refault_anon 6\nworkingset_refault_file 7\n\
             inactive_file 9\nactive_file 11\n",
        );
        assert_eq!(s.rss, 8192);
        assert_eq!(s.cache, 4096);
        assert_eq!(s.mapped_file, 1);
        assert_eq!(s.dirty, 2);
        assert_eq!(s.writeback, 3);
        assert_eq!(s.pgfault, 40);
        assert_eq!(s.pgmajfault, 5);
        assert_eq!(s.workingset_refault_file, 7);
        assert_eq!(s.inactive_file, 9);
        assert_eq!(s.active_file, 11);
        assert_eq!(
            parse_events_max("low 0\nhigh 2\nmax 13\noom 0\noom_kill 0\noom_group_kill 0\n"),
            13
        );
        assert_eq!(parse_events_max("low 0\nhigh 2\n"), 0);
        assert_eq!(parse_events_max(""), 0);
    }

    /// The files a test made in a fake group, removed so that the group's
    /// own removal -- an `rmdir`, as a cgroup directory's is -- can go.
    fn clear_files(dir: &Path) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.file_type().unwrap().is_file() {
                std::fs::remove_file(e.path()).unwrap();
            }
        }
    }

    /// A fake unified hierarchy: a directory of plain files for the group a
    /// wrapper started this process in, not delegated. The attach holds the
    /// group's limit to the bound and its swap to zero, each refusal naming
    /// the file, reads the counts through v2's files, and gives every pass
    /// the run's own group; it makes nothing and removes nothing, so the
    /// group outlives the bound.
    #[test]
    fn a_v2_group_that_is_not_delegated_is_shared_at_its_bound_and_refused_at_any_other() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-v2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let own = "/system.slice/run-u1234.service";
        let group = root.join("system.slice/run-u1234.service");
        std::fs::create_dir_all(&group).unwrap();
        let limit = 1u64 << 30;
        let refusal = |why: &str| {
            let err = Bound::attach_at(&root, own, limit)
                .err()
                .unwrap_or_else(|| panic!("attached with {why}"));
            err.to_string()
        };
        let text = refusal("no memory.max");
        assert!(text.contains("memory.max"), "{text}");
        std::fs::write(group.join("memory.max"), "max\n").unwrap();
        let text = refusal("memory.max of max");
        assert!(
            text.contains("no limit")
                && text.contains(&format!("MemoryMax={limit}"))
                && text.contains("Delegate=yes"),
            "{text}"
        );
        std::fs::write(group.join("memory.max"), format!("{}\n", limit * 2)).unwrap();
        let text = refusal("another limit");
        assert!(
            text.contains(&(limit * 2).to_string()) && text.contains(&limit.to_string()),
            "{text}"
        );
        std::fs::write(group.join("memory.max"), format!("{limit}\n")).unwrap();
        std::fs::write(group.join("memory.swap.max"), "max\n").unwrap();
        let text = refusal("swap allowed");
        assert!(
            text.contains("memory.swap.max") && text.contains("MemorySwapMax=0"),
            "{text}"
        );
        std::fs::write(group.join("memory.swap.max"), "0\n").unwrap();
        let b = Bound::attach_at(&root, own, limit).unwrap();
        assert_eq!(b.limit(), limit);
        assert_eq!(b.mode(), Mode::Shared);
        assert!(!b.is_owned(), "a v2 group is nobody's to remove");
        assert_eq!(b.group(), group);
        std::fs::write(
            group.join("memory.stat"),
            "anon 8192\nfile 4096\nfile_dirty 2\npgmajfault 5\nworkingset_refault_file 7\n",
        )
        .unwrap();
        std::fs::write(group.join("memory.peak"), "123456\n").unwrap();
        std::fs::write(group.join("memory.events"), "low 0\nhigh 0\nmax 3\noom 0\n").unwrap();
        std::fs::write(group.join("memory.current"), "9437184\n").unwrap();
        let s = b.stat().unwrap();
        assert_eq!((s.rss, s.cache, s.dirty), (8192, 4096, 2));
        assert_eq!((s.pgmajfault, s.workingset_refault_file), (5, 7));
        assert_eq!((s.max_usage_in_bytes, s.failcnt), (123456, 3));
        // Every pass in the run's own group, made by nobody.
        let p = b.pass(7).unwrap();
        assert_eq!(p.group(), group);
        let handed = p.bound();
        assert_eq!(handed.usage().unwrap(), 9437184);
        drop(p);
        // The bound a pass's process is handed reads the same files.
        let text = serde_json::to_string(&handed).unwrap();
        assert!(
            text.contains("\"v2\":true") && text.contains("Shared"),
            "{text}"
        );
        let handed: Bound = serde_json::from_str(&text).unwrap();
        assert!(!handed.is_owned());
        assert_eq!(handed.stat().unwrap().failcnt, 3);
        drop(handed);
        drop(b);
        assert!(group.is_dir(), "the group outlives the bound");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A delegated, unlimited v2 group: every process in it moves to a
    /// `harness` leaf, the memory controller is handed down, and each pass
    /// gets a leaf beside `harness` with the limit on it, no swap and the
    /// group killed together at the limit -- the files a kernel with swap
    /// accounting and the group kill has -- removed at its drop.
    #[test]
    fn a_delegated_v2_group_puts_the_run_in_a_leaf_and_each_pass_in_one_of_its_own() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-v2d-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let own = "/system.slice/run-u99.service";
        let group = root.join("system.slice/run-u99.service");
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(group.join("memory.max"), "max\n").unwrap();
        std::fs::write(group.join("cgroup.procs"), "4242\n").unwrap();
        let limit = 1u64 << 30;
        let b = Bound::delegate_at(&root, own, limit).unwrap();
        assert_eq!(b.mode(), Mode::PerPass);
        assert!(!b.is_owned(), "the service's group is the service's");
        assert_eq!(
            std::fs::read_to_string(group.join("harness/cgroup.procs")).unwrap(),
            "4242",
            "the run's processes moved into the harness's leaf"
        );
        assert_eq!(
            std::fs::read_to_string(group.join("cgroup.subtree_control")).unwrap(),
            "+memory"
        );
        let pass_dir = group.join("pass-3");
        let p = b.pass(3).unwrap();
        assert_eq!(p.group(), pass_dir);
        assert_eq!(
            std::fs::read_to_string(pass_dir.join("memory.max")).unwrap(),
            limit.to_string()
        );
        assert!(
            !pass_dir.join("memory.swap.max").exists(),
            "written only where the kernel has it"
        );
        clear_files(&pass_dir);
        drop(p);
        assert!(!pass_dir.exists(), "the pass's group goes with it");
        // A stale pass group of the same number that will not go -- a file
        // stands for a process in it -- refuses, naming it.
        std::fs::create_dir(&pass_dir).unwrap();
        std::fs::write(pass_dir.join("cgroup.procs"), "77\n").unwrap();
        let err = b.pass(3).err().expect("refused");
        assert!(err.to_string().contains("stale pass group"), "{err}");
        clear_files(&pass_dir);
        std::fs::remove_dir(&pass_dir).unwrap();
        // A group that is not delegated refuses the leaf, naming why.
        let undelegated = root.join("system.slice/run-u100.service");
        std::fs::create_dir_all(&undelegated).unwrap();
        std::fs::write(undelegated.join("harness"), "a file where the leaf goes").unwrap();
        let err = Bound::delegate_at(&root, "/system.slice/run-u100.service", limit)
            .err()
            .expect("refused");
        assert!(err.to_string().contains("Delegate=yes"), "{err}");
        drop(b);
        assert!(group.is_dir(), "the service's group outlives the bound");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Under v1 the run's group is made with no process and no limit in it:
    /// the run stays where it was started, and each pass gets a group of
    /// its own under the run's with the limit on it, removed at its drop.
    /// The run's group goes at the bound's drop with any pass group a cut
    /// pass left in it.
    #[test]
    fn a_v1_run_group_holds_no_process_and_each_pass_gets_its_own() {
        let root = std::env::temp_dir().join(format!("supdb-bench-bound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let own = "/process_api/x/claude-code-bash";
        let parent = root.join("process_api/x/claude-code-bash");
        std::fs::create_dir_all(&parent).unwrap();
        // A stale sibling with an empty pass group in it, which a run that
        // died leaves, goes with it.
        std::fs::create_dir_all(parent.join("supdb-bench-1/pass-4")).unwrap();
        let pid = std::process::id();
        let limit = 1u64 << 30;
        let b = Bound::create_at(&root, own, limit, pid).unwrap();
        let group = parent.join(format!("supdb-bench-{pid}"));
        assert_eq!(b.group(), group);
        assert!(b.is_owned());
        assert_eq!(b.mode(), Mode::PerPass);
        assert!(
            !parent.join("supdb-bench-1").exists(),
            "the stale sibling went"
        );
        assert!(
            !group.join("cgroup.procs").exists() && !group.join("memory.limit_in_bytes").exists(),
            "no process and no limit in the run's group"
        );
        let p = b.pass(1).unwrap();
        let pass_dir = group.join("pass-1");
        assert_eq!(p.group(), pass_dir);
        assert_eq!(
            std::fs::read_to_string(pass_dir.join("memory.limit_in_bytes")).unwrap(),
            limit.to_string()
        );
        assert!(
            !p.bound().is_owned(),
            "the pass's process is handed it attached"
        );
        clear_files(&pass_dir);
        drop(p);
        assert!(!pass_dir.exists());
        // A pass group the kernel will not make refuses, naming it.
        std::fs::write(group.join("pass-2"), "a file where the group goes").unwrap();
        let err = b.pass(2).err().expect("refused");
        assert!(err.to_string().contains("pass-2"), "{err}");
        std::fs::remove_file(group.join("pass-2")).unwrap();
        // A pass group a cut pass left goes with the run's group.
        std::fs::create_dir(group.join("pass-9")).unwrap();
        drop(b);
        assert!(!group.exists(), "the run's group and its leftovers went");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The pass's process joins its group before it runs anything: a real
    /// spawn, with the group's `cgroup.procs` a plain file, leaves the
    /// child's own pid in it, written between the fork and the exec.
    #[cfg(unix)]
    #[test]
    fn a_pass_process_writes_its_own_pid_into_its_group_before_the_exec() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-join-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let parent = root.join("g");
        std::fs::create_dir_all(&parent).unwrap();
        let b = Bound::create_at(&root, "/g", 1 << 30, std::process::id()).unwrap();
        let p = b.pass(5).unwrap();
        // The kernel's file, which a plain directory has to be given.
        std::fs::write(p.group().join("cgroup.procs"), "").unwrap();
        let mut cmd = std::process::Command::new("true");
        p.join_before_exec(&mut cmd);
        let child = cmd.spawn().unwrap();
        let pid = child.id();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert_eq!(
            std::fs::read_to_string(p.group().join("cgroup.procs")).unwrap(),
            pid.to_string()
        );
        // A group whose procs file cannot be opened fails the spawn.
        clear_files(p.group());
        std::fs::create_dir(p.group().join("cgroup.procs")).unwrap();
        let mut cmd = std::process::Command::new("true");
        p.join_before_exec(&mut cmd);
        assert!(
            cmd.spawn().is_err(),
            "a pass that cannot join its group does not run"
        );
        std::fs::remove_dir(p.group().join("cgroup.procs")).unwrap();
        drop(p);
        drop(b);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_controller_without_the_process_group_refuses_naming_it() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let err = Bound::create_at(&root, "/not/here", 1 << 30, std::process::id())
            .err()
            .expect("refused");
        assert!(err.to_string().contains("not/here"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A stale sibling that will not go -- a file in it stands for a
    /// process a cgroup directory would refuse its removal for -- is a run
    /// still alive, reported and not measured beside.
    #[test]
    fn a_stale_sibling_with_contents_is_reported_not_removed() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let parent = root.join("g");
        let stale = parent.join("supdb-bench-2");
        std::fs::create_dir_all(stale.join("pass-1")).unwrap();
        std::fs::write(stale.join("pass-1/tasks"), "77\n").unwrap();
        let err = Bound::create_at(&root, "/g", 1 << 30, std::process::id())
            .err()
            .expect("refused");
        let text = err.to_string();
        assert!(text.contains("supdb-bench-2"), "{text}");
        assert!(text.contains("stale"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod attached {
    use super::Bound;

    /// A bound read back from a pass's spec is attached, not owned: the
    /// pass's process reads its group and must not remove it at its drop,
    /// which is what `owned` gates; a spec written before the bound had a
    /// mode reads as a group of the pass's own. The group named here does
    /// not exist, and the drop must not try to remove it.
    #[test]
    fn a_deserialized_bound_is_attached_and_its_drop_touches_nothing() {
        let b: Bound = serde_json::from_str(
            r#"{"group":"/no/such/group","parent":"/no/such/parent","limit":4096}"#,
        )
        .unwrap();
        assert!(!b.is_owned());
        assert_eq!(b.mode(), super::Mode::PerPass);
        assert_eq!(b.limit(), 4096);
        assert_eq!(b.group(), std::path::Path::new("/no/such/group"));
        let text = serde_json::to_string(&b).unwrap();
        assert!(
            !text.contains("owned"),
            "owned is this process's own and never travels: {text}"
        );
        drop(b);
    }
}
