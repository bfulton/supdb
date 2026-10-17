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
//! Only cgroup v1 is handled, because this host is v1 with the memory
//! controller mounted read-write and v2 mounted with nothing delegated;
//! on a v2-only host `enter` refuses, naming the file it could not find,
//! rather than measuring an unbounded run as a bounded one. The parsers
//! are pure functions over the files' text so that a host without a
//! controller can still test them.

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

/// `memory.stat`'s text into the fields above. The file carries each key
/// twice, plain and as `total_`; the plain one is this group's own, which
/// is what the run is.
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

/// The group this process runs in while it measures. Dropping it moves
/// the process back to the group it came from and removes the group.
pub struct Bound {
    /// The group's directory under the controller's mount.
    group: PathBuf,
    /// The group this process was in before, which it goes back to.
    parent: PathBuf,
    /// The limit as the kernel reports it after the write: v1 rounds the
    /// number written down to a page, and the row records what holds.
    limit: u64,
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
        let own = parse_cgroup_path(&read_from(cgroup)?).ok_or_else(|| {
            io::Error::other(format!(
                "no memory controller in {}: a cgroup v2-only host, which this bound does not \
                 drive",
                cgroup.display()
            ))
        })?;
        let mounts = Path::new("/proc/mounts");
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

    /// The group's counters now. Read before the first operation of a
    /// phase and after its last, never between, as every counter the
    /// suite reads around a phase is.
    pub fn stat(&self) -> BoundStat {
        let text = std::fs::read_to_string(self.group.join("memory.stat")).unwrap_or_default();
        let mut s = parse_stat(&text);
        s.max_usage_in_bytes =
            read_u64(&self.group.join("memory.max_usage_in_bytes")).unwrap_or(0);
        s.failcnt = read_u64(&self.group.join("memory.failcnt")).unwrap_or(0);
        s
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
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

    /// A fake controller: a directory of plain files. The group's
    /// `cgroup.procs` is a directory, so the write that would move the
    /// process fails whoever runs the test -- a permission bit would not
    /// stop root -- and the refusal names that file.
    #[test]
    fn a_pid_that_cannot_be_written_refuses_naming_the_file() {
        let root =
            std::env::temp_dir().join(format!("supdb-bench-bound-{}", std::process::id()));
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
        let root = std::env::temp_dir()
            .join(format!("supdb-bench-bound-stale-{}", std::process::id()));
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
