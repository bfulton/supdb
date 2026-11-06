//! The memory bound a bounded run measures under: a cgroup of the run's
//! own, made under the group the process was started in, with a
//! `memory.limit_in_bytes` the stores cannot cache past.
//!
//! Every banked row is in-core: the full scale's top, the rung at 1.5x the
//! machine's memory, was never reached, and the quick ladder fits the
//! page cache a hundred times over. A bound is how the out-of-core regime
//! is reached on a machine whose memory is not the store's size -- the
//! kernel reclaims the group's page cache at the limit, so a store past it
//! reads from the device, which is the regime the `ooc` scale measures.
//! The bound is the row's class as much as the memory is (`Row::class`).
//!
//! Two hierarchies, two ways in. Under cgroup v1 -- this host, with the
//! memory controller mounted read-write -- the run makes a group of its
//! own under the one it was started in, writes the limit and moves itself
//! in (`enter_at`). Under cgroup v2 alone -- GitHub's hosted runners --
//! a process cannot bound itself from inside a leaf, since a group with
//! processes in it may not hand the memory controller to a child, so
//! whoever starts the run bounds it (`systemd-run -p MemoryMax=BYTES
//! -p MemorySwapMax=0`) and the run attaches to the group it finds itself
//! in, holding that group's `memory.max` to the bound it was asked for
//! (`attach_at`): a limit of `max` is no bound, and another number is
//! another row's class. Either way a run that cannot be bounded refuses,
//! naming the file, rather than measuring an unbounded run as a bounded
//! one. The parsers are pure functions over the files' text so that a
//! host without the controller can still test them, and the attach runs
//! against a directory of plain files.

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

/// The group this process runs in while it measures. Dropping it moves
/// the process back to the group it came from and removes the group.
/// A clone is attached and not owned, as a deserialized bound is: the
/// group is one process's to leave and remove, and a plan copied into a
/// run's ledger (`ledger::Head`) names the group without holding it.
#[derive(Serialize, Deserialize)]
pub struct Bound {
    /// The group's directory under the controller's mount.
    group: PathBuf,
    /// The group this process was in before, which it goes back to.
    parent: PathBuf,
    /// The limit as the kernel reports it after the write: v1 rounds the
    /// number written down to a page, and the row records what holds.
    limit: u64,
    /// Whether this process entered the group and leaves and removes it at
    /// the drop. A bound deserialized -- in the process a bounded run
    /// gives each arm's rep (`run::one_pass_child`), which its parent
    /// spawned inside the group -- is attached and not owned: it reads the
    /// group's counts and neither leaves the group nor removes it, since
    /// the parent is still in it.
    #[serde(skip)]
    owned: bool,
    /// Whether the group is cgroup v2's, read through its files
    /// (`memory.max`, `memory.stat`'s v2 keys, `memory.peak`,
    /// `memory.events`), as against v1's. Travels with the bound to the
    /// pass's process, which reads the same group.
    #[serde(default)]
    v2: bool,
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

impl Bound {
    /// Enter a fresh group under this process's own, limited to `limit`
    /// bytes, and move the process into it. Under v1 a write to
    /// `cgroup.procs` moves the whole thread group, so this runs before
    /// any thread exists and before any allocation worth charging: the
    /// row's machine capture, the floors' buffers and the stores all come
    /// after it. The error names the file and the errno of whichever step
    /// refused -- no controller line, no controller mount, a group that
    /// cannot be made, a limit or a pid that cannot be written, a limit
    /// that reads back as something else, or a `/proc/self/cgroup` that
    /// still names the old group after the move.
    pub fn enter(limit: u64) -> io::Result<Bound> {
        let cgroup = Path::new("/proc/self/cgroup");
        let text = read_from(cgroup)?;
        let mounts = Path::new("/proc/mounts");
        let Some(own) = parse_cgroup_path(&text) else {
            // No v1 memory controller: a v2-only host, where the bound is
            // the group this process was started in (`attach_at`).
            let own = parse_unified_path(&text).ok_or_else(|| {
                io::Error::other(format!(
                    "no memory controller and no unified hierarchy in {}",
                    cgroup.display()
                ))
            })?;
            let root = parse_unified_mount(&read_from(mounts)?).ok_or_else(|| {
                io::Error::other(format!("no cgroup2 mount in {}", mounts.display()))
            })?;
            return Bound::attach_at(&root, &own, limit);
        };
        let root = parse_mount(&read_from(mounts)?).ok_or_else(|| {
            io::Error::other(format!(
                "no cgroup mount with the memory controller in {}",
                mounts.display()
            ))
        })?;
        let b = Bound::enter_at(&root, &own, limit, std::process::id())?;
        // The move, confirmed by the kernel's own statement of where this
        // process is rather than by the write having returned.
        let now = parse_cgroup_path(&read_from(cgroup)?).unwrap_or_default();
        let want = b.group.strip_prefix(&root).map_err(io::Error::other)?;
        let want = format!("/{}", want.display());
        if now != want {
            return Err(io::Error::other(format!(
                "{} names {now:?} after the move and the group is {want:?}",
                cgroup.display()
            )));
        }
        Ok(b)
    }

    /// `enter` against a controller mounted at `root`, for a process in the
    /// group `own` under it: the steps between the parsers and the
    /// confirmation, which a test drives against a directory of plain
    /// files. A group of this process's own name left by an earlier
    /// process of the same pid is taken over, since that process is gone.
    pub fn enter_at(root: &Path, own: &str, limit: u64, pid: u32) -> io::Result<Bound> {
        let parent = root.join(own.trim_start_matches('/'));
        if !parent.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("the controller has no group at {}", parent.display()),
            ));
        }
        // A stale sibling is a run that died; one with tasks still in it
        // is a run that has not, and two timing runs may not share a
        // machine, so it is reported rather than measured beside.
        let mine = format!("{PREFIX}{pid}");
        if let Ok(rd) = std::fs::read_dir(&parent) {
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with(PREFIX) || name == mine || !e.path().is_dir() {
                    continue;
                }
                if let Err(err) = std::fs::remove_dir(e.path()) {
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
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(err_at(&group, "could not make the group", e)),
        }
        let limit_file = group.join("memory.limit_in_bytes");
        write_to(&limit_file, &limit.to_string())?;
        let got = read_u64(&limit_file)?;
        // v1 keeps the limit in pages: the number read back is the number
        // written rounded down to one, and anything else is another file.
        let page = page_size();
        if got != limit / page * page {
            return Err(io::Error::other(format!(
                "{} reads back {got} after writing {limit}",
                limit_file.display()
            )));
        }
        let procs = group.join("cgroup.procs");
        write_to(&procs, &pid.to_string())?;
        Ok(Bound {
            group,
            parent,
            limit: got,
            owned: true,
            v2: false,
        })
    }

    /// The bound as cgroup v2 gives it: the group this process was started
    /// in, `own` under the unified mount at `root`, whose `memory.max` is
    /// held to `limit` -- `max` is no bound, and another number is another
    /// row's class -- and whose `memory.swap.max`, where the kernel
    /// accounts swap, is held to zero, since a group that can swap its
    /// anonymous memory out is in another regime from this host's, which
    /// has none. v2 lets no process bound itself from inside a leaf, so
    /// the wrapper that starts the run sets both (`systemd-run -p
    /// MemoryMax=BYTES -p MemorySwapMax=0`, as the `ooc-hosted` workflow
    /// does), and the group is nobody's to leave or remove here: the bound
    /// is attached, never owned.
    pub fn attach_at(root: &Path, own: &str, limit: u64) -> io::Result<Bound> {
        let group = root.join(own.trim_start_matches('/'));
        let max_file = group.join("memory.max");
        let text = read_from(&max_file)?;
        let got = text.trim();
        if got == "max" {
            return Err(io::Error::other(format!(
                "{} is \"max\": the group this process runs in has no limit, and on cgroup v2 \
                 the run is bounded by whoever starts it (systemd-run -p MemoryMax={limit} -p \
                 MemorySwapMax=0)",
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
        })
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
}

impl Bound {
    /// Whether this process entered the group, as against a bound a pass's
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
        }
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        // Back to the parent first: a group with a task in it refuses to
        // go. Neither failure can be handled here, and neither is silent.
        let procs = self.parent.join("cgroup.procs");
        if let Err(e) = write_to(&procs, &std::process::id().to_string()) {
            eprintln!("bound: leaving the group: {e}");
        }
        if let Err(e) = std::fs::remove_dir(&self.group) {
            eprintln!(
                "bound: could not remove {}: {e}; rmdir it by hand",
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

    /// A fake unified hierarchy: a directory of plain files for the group
    /// a wrapper started this process in. The attach holds the group's
    /// limit to the bound and its swap to zero, each refusal naming the
    /// file, and reads the counts through v2's files; it makes nothing and
    /// removes nothing, so the group outlives the bound.
    #[test]
    fn a_v2_group_is_attached_at_its_bound_and_refused_at_any_other() {
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
            text.contains("no limit") && text.contains(&format!("MemoryMax={limit}")),
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
        assert!(!b.is_owned(), "a v2 group is nobody's to remove");
        assert_eq!(b.group(), group);
        std::fs::write(
            group.join("memory.stat"),
            "anon 8192\nfile 4096\nfile_dirty 2\npgmajfault 5\nworkingset_refault_file 7\n",
        )
        .unwrap();
        std::fs::write(group.join("memory.peak"), "123456\n").unwrap();
        std::fs::write(group.join("memory.events"), "low 0\nhigh 0\nmax 3\noom 0\n").unwrap();
        let s = b.stat().unwrap();
        assert_eq!((s.rss, s.cache, s.dirty), (8192, 4096, 2));
        assert_eq!((s.pgmajfault, s.workingset_refault_file), (5, 7));
        assert_eq!((s.max_usage_in_bytes, s.failcnt), (123456, 3));
        // The bound a pass's process is handed reads the same files.
        let text = serde_json::to_string(&b).unwrap();
        assert!(text.contains("\"v2\":true"), "{text}");
        let handed: Bound = serde_json::from_str(&text).unwrap();
        assert!(!handed.is_owned());
        assert_eq!(handed.stat().unwrap().failcnt, 3);
        drop(handed);
        drop(b);
        assert!(group.is_dir(), "the group outlives the bound");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A fake controller: a directory of plain files. The group's
    /// `cgroup.procs` is a directory, so the write that would move the
    /// process fails whoever runs the test -- a permission bit would not
    /// stop root -- and the refusal names that file.
    #[test]
    fn a_pid_that_cannot_be_written_refuses_naming_the_file() {
        let root = std::env::temp_dir().join(format!("supdb-bench-bound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let own = "/process_api/x/claude-code-bash";
        let parent = root.join("process_api/x/claude-code-bash");
        std::fs::create_dir_all(&parent).unwrap();
        // A stale sibling with nothing in it goes; the group of this pid
        // is pre-made so that its procs file can be made unwritable.
        std::fs::create_dir(parent.join("supdb-bench-1")).unwrap();
        let pid = std::process::id();
        let group = parent.join(format!("supdb-bench-{pid}"));
        std::fs::create_dir(&group).unwrap();
        std::fs::create_dir(group.join("cgroup.procs")).unwrap();
        let err = Bound::enter_at(&root, own, 1 << 30, pid)
            .err()
            .expect("refused");
        let text = err.to_string();
        assert!(text.contains("cgroup.procs"), "{text}");
        assert!(text.contains(&group.display().to_string()), "{text}");
        assert!(
            !parent.join("supdb-bench-1").exists(),
            "the stale sibling stays"
        );
        // The limit was written and read back as itself before the refusal.
        assert_eq!(
            std::fs::read_to_string(group.join("memory.limit_in_bytes")).unwrap(),
            (1u64 << 30).to_string()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_controller_without_the_process_group_refuses_naming_it() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let err = Bound::enter_at(&root, "/not/here", 1 << 30, std::process::id())
            .err()
            .expect("refused");
        assert!(err.to_string().contains("not/here"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_stale_sibling_with_contents_is_reported_not_removed() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let parent = root.join("g");
        let stale = parent.join("supdb-bench-2");
        std::fs::create_dir_all(stale.join("inner")).unwrap();
        let err = Bound::enter_at(&root, "/g", 1 << 30, std::process::id())
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
    /// process a bounded run gives a pass was spawned inside the group and
    /// must neither leave it nor remove it at its drop, which is what
    /// `owned` gates. The group named here does not exist, and the drop
    /// must not try to leave or remove it.
    #[test]
    fn a_deserialized_bound_is_attached_and_its_drop_touches_nothing() {
        let b: Bound = serde_json::from_str(
            r#"{"group":"/no/such/group","parent":"/no/such/parent","limit":4096}"#,
        )
        .unwrap();
        assert!(!b.is_owned());
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
