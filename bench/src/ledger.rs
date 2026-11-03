//! The ledger of a run in flight: what `run` has measured so far, one
//! file a fragment, under `<out>/inflight/<utc>-<sha7>/`, so that a run
//! the kernel, a timeout or a hand interrupted is taken up from its last
//! whole rep instead of started again (`bench run --resume DIR`).
//!
//! Under a bound a pass is already independent of the passes before it:
//! a process of its own, a pool primed before its store opens, a store
//! built from nothing and removed after. So a pass's result on disk is a
//! checkpoint the run can be taken up from, and the ledger is those
//! results kept rather than read once and deleted. Every fragment is a
//! list of the row's own measurements (`row::Measurement`), and the row
//! is their merge in the order the run makes them, live or replayed,
//! through one function (`run::row_of`), so a resumed run composes the
//! row the uninterrupted one would have, and nothing is defined twice.
//!
//! The grain a bankable row resumes at is the rep, whole. The arms are
//! interleaved within a rep, which is the one comparison the suite
//! trusts, so a rep that was cut between two arms is taken again from
//! its first arm; the rep's own file (`rep-<size>-<rep>.json`, the
//! floors sampled after it, written last) is what says a rep is whole.
//! `--resume-passes` keeps a cut rep's passes instead, for a loop where
//! the interleaving is not the point, and the row says so. The floors
//! are taken again at every sitting, since the host may have moved
//! between them, and the row records where each later sitting began
//! (`row::Resumption`).
//!
//! What a resume holds the run to is in `refusal`: the binary's stamp,
//! and the machine's class, the bound in it. The plan is the ledger's
//! own, since a plan that changed is another run.

use crate::row::{Kill, MachineInfo, Measurement, Resumption};
use crate::run::Plan;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// The directory under the runs directory the ledgers live in.
pub const INFLIGHT_DIR: &str = "inflight";

/// What the ledger says about the run it is of: written at the run's
/// start and read back by a resume. The plan is kept whole, its bound
/// included as the paths it had, which the resume replaces with the
/// group it enters.
#[derive(Serialize, Deserialize)]
pub struct Head {
    pub utc: String,
    pub sha: String,
    pub rustc: String,
    pub machine: MachineInfo,
    pub plan: Plan,
    /// The sittings after the first, one per resume that ran a pass.
    #[serde(default)]
    pub resumptions: Vec<Resumption>,
}

/// Why this binary on this machine may not take the ledger's run up, or
/// nothing. The stamp is the binary's `SUPDB_SHA`, `-dirty` and all: two
/// dirty trees read the same under it and may not be the same code,
/// which is as much as the stamp can say, and a row of a dirty tree was
/// never the series' anyway. The class carries the bound (`Row::class`),
/// so a resume under another bound is refused here too.
pub fn refusal(head: &Head, sha: &str, rustc: &str, machine: &MachineInfo) -> Option<String> {
    if head.sha != sha {
        return Some(format!(
            "the ledger is of {} and this binary is {sha}; a resumed run is the same engine",
            head.sha
        ));
    }
    if head.rustc != rustc {
        return Some(format!(
            "the ledger's binary was built by {:?} and this one by {rustc:?}",
            head.rustc
        ));
    }
    let (was, now) = (head.machine.class(), machine.class());
    if was != now {
        return Some(format!(
            "the ledger's class is {was} and this machine's is {now}; a row is one class"
        ));
    }
    None
}

/// Whether a rep is whole in the ledger, cut, or not begun: the arms
/// whose pass or kill is recorded, against the rep's own file. A rep
/// with its file is whole whatever else is there, since the file is
/// written after every arm.
#[derive(Debug, PartialEq, Eq)]
pub enum RepState {
    Whole,
    /// The arms recorded so far, in the plan's order; empty when none.
    Cut(Vec<String>),
}

/// The ledger: a directory of fragments.
pub struct Ledger {
    dir: PathBuf,
}

fn io_at(path: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    // Whole or absent: a fragment half written by a process that died is
    // read as a fragment by nobody.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text + "\n").map_err(|e| io_at(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io_at(path, e))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_at(path, e)),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))
}

impl Ledger {
    /// A fresh ledger for the run `head` describes, under `out`, named by
    /// the run's utc and sha as its row will be.
    pub fn create(out: &Path, head: &Head) -> io::Result<Ledger> {
        let sha7: String = head.sha.chars().take(7).collect();
        let dir = out.join(INFLIGHT_DIR).join(format!("{}-{sha7}", head.utc));
        std::fs::create_dir_all(&dir).map_err(|e| io_at(&dir, e))?;
        let l = Ledger { dir };
        l.write_head(head)?;
        Ok(l)
    }

    /// An existing ledger, its head read; a directory with no head is
    /// not a ledger.
    pub fn open(dir: &Path) -> io::Result<(Ledger, Head)> {
        let l = Ledger {
            dir: dir.to_path_buf(),
        };
        let head = read_json::<Head>(&l.dir.join("head.json"))?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} holds no head.json; not a ledger", dir.display()),
            )
        })?;
        Ok((l, head))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn write_head(&self, head: &Head) -> io::Result<()> {
        write_json(&self.dir.join("head.json"), head)
    }

    /// The runs directory this ledger lives under: `<out>/inflight/<name>`
    /// has `<out>` two above it.
    pub fn out(&self) -> Option<&Path> {
        self.dir.parent()?.parent()
    }

    fn floors_path(&self, sitting: usize) -> PathBuf {
        self.dir.join(format!("floors-{sitting}.json"))
    }
    fn pass_path(&self, size: u64, rep: usize, arm: &str) -> PathBuf {
        self.dir.join(format!("pass-{size}-{rep}-{arm}.json"))
    }
    fn kill_path(&self, size: u64, rep: usize, arm: &str) -> PathBuf {
        self.dir.join(format!("kill-{size}-{rep}-{arm}.json"))
    }
    fn rep_path(&self, size: u64, rep: usize) -> PathBuf {
        self.dir.join(format!("rep-{size}-{rep}.json"))
    }

    /// The floors a sitting took at its start, by the sitting's number.
    pub fn write_floors(&self, sitting: usize, m: &[Measurement]) -> io::Result<()> {
        write_json(&self.floors_path(sitting), &m)
    }

    /// Every sitting's floors, in sitting order.
    pub fn read_floors(&self) -> io::Result<Vec<Vec<Measurement>>> {
        let mut out = Vec::new();
        for sitting in 0.. {
            match read_json::<Vec<Measurement>>(&self.floors_path(sitting))? {
                Some(m) => out.push(m),
                None => break,
            }
        }
        Ok(out)
    }

    /// A pass's measurements: empty for a warmup pass, which the file's
    /// presence still records as done.
    pub fn write_pass(
        &self,
        size: u64,
        rep: usize,
        arm: &str,
        m: &[Measurement],
    ) -> io::Result<()> {
        write_json(&self.pass_path(size, rep, arm), &m)
    }

    pub fn read_pass(
        &self,
        size: u64,
        rep: usize,
        arm: &str,
    ) -> io::Result<Option<Vec<Measurement>>> {
        read_json(&self.pass_path(size, rep, arm))
    }

    pub fn write_kill(&self, k: &Kill) -> io::Result<()> {
        write_json(&self.kill_path(k.size, k.rep, &k.arm), k)
    }

    pub fn read_kill(&self, size: u64, rep: usize, arm: &str) -> io::Result<Option<Kill>> {
        read_json(&self.kill_path(size, rep, arm))
    }

    /// The floors sampled after a rep, and the mark that the rep is
    /// whole: written after every arm of the rep has its pass or its
    /// kill recorded, never before.
    pub fn write_rep(&self, size: u64, rep: usize, m: &[Measurement]) -> io::Result<()> {
        write_json(&self.rep_path(size, rep), &m)
    }

    pub fn read_rep(&self, size: u64, rep: usize) -> io::Result<Option<Vec<Measurement>>> {
        read_json(&self.rep_path(size, rep))
    }

    /// Whether the rep is whole, or which of `arms` it holds so far.
    pub fn rep_state(&self, size: u64, rep: usize, arms: &[String]) -> RepState {
        if self.rep_path(size, rep).exists() {
            return RepState::Whole;
        }
        RepState::Cut(
            arms.iter()
                .filter(|a| {
                    self.pass_path(size, rep, a).exists() || self.kill_path(size, rep, a).exists()
                })
                .cloned()
                .collect(),
        )
    }

    /// Forget a cut rep: every pass and kill of it, so that it is taken
    /// again whole, from its first arm.
    pub fn forget_rep(&self, size: u64, rep: usize, arms: &[String]) -> io::Result<()> {
        for arm in arms {
            for p in [
                self.pass_path(size, rep, arm),
                self.kill_path(size, rep, arm),
            ] {
                match std::fs::remove_file(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_at(&p, e)),
                }
            }
        }
        Ok(())
    }

    /// The ledger is done with: its row is written.
    pub fn remove(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.dir).map_err(|e| io_at(&self.dir, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row::Guarantee;
    use crate::Scale;

    fn machine() -> MachineInfo {
        MachineInfo {
            arch: "x86_64".into(),
            cpu_model: "Test CPU".into(),
            cpus: 4,
            mem_total_kb: 16 << 20,
            page_size: 4096,
            cache_line: 64,
            cache_line_detected: true,
            l1d: 0,
            l2: 0,
            l3: 0,
            kernel: "k".into(),
            governor: "g".into(),
            thp: "t".into(),
            smt_on: false,
            pmu_available: false,
            aslr_disabled: false,
            virtualised: "none".into(),
            mem_limit_bytes: Some(1 << 30),
            read_ahead_kb: None,
            rotational: None,
            write_cache: None,
            device: None,
        }
    }

    fn head() -> Head {
        Head {
            utc: "20260101T000000Z".into(),
            sha: "abc1234def".into(),
            rustc: "rustc 1.0".into(),
            machine: machine(),
            plan: Plan::new(
                Scale::Ooc,
                vec!["supdb".into(), "lmdb".into()],
                3_000_000,
                4,
            ),
            resumptions: vec![],
        }
    }

    fn m(workload: &str, arm: &str, size: Option<u64>, q: &str, v: f64) -> Measurement {
        Measurement {
            workload: workload.into(),
            arm: arm.into(),
            guarantee: Guarantee::Durable,
            size,
            quantity: q.into(),
            unit: "u".into(),
            samples: vec![v],
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("supdb-bench-ledger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The resume is held to the stamp and the class, each named in the
    /// refusal, and a bound is a class.
    #[test]
    fn a_resume_is_refused_by_stamp_or_class_and_names_which() {
        let h = head();
        assert_eq!(refusal(&h, "abc1234def", "rustc 1.0", &machine()), None);
        let sha = refusal(&h, "ffff", "rustc 1.0", &machine()).unwrap();
        assert!(sha.contains("abc1234def") && sha.contains("ffff"), "{sha}");
        let rustc = refusal(&h, "abc1234def", "rustc 2.0", &machine()).unwrap();
        assert!(rustc.contains("rustc 2.0"), "{rustc}");
        let mut other = machine();
        other.mem_limit_bytes = Some(2 << 30);
        let class = refusal(&h, "abc1234def", "rustc 1.0", &other).unwrap();
        assert!(
            class.contains("/b1024m") && class.contains("/b2048m"),
            "{class}"
        );
        let mut cpus = machine();
        cpus.cpus = 8;
        assert!(refusal(&h, "abc1234def", "rustc 1.0", &cpus).is_some());
    }

    /// A rep is whole by its own file and by nothing else, a cut rep names
    /// the arms it holds in the plan's order, and forgetting it leaves
    /// nothing of it.
    #[test]
    fn a_rep_is_whole_by_its_file_and_a_cut_rep_names_what_it_holds() {
        let out = scratch("rep");
        let l = Ledger::create(&out, &head()).unwrap();
        let arms = vec!["supdb".to_string(), "lmdb".to_string()];
        assert_eq!(l.rep_state(3_000_000, 1, &arms), RepState::Cut(vec![]));
        l.write_pass(
            3_000_000,
            1,
            "lmdb",
            &[m("read", "lmdb", Some(3_000_000), "reads_per_s", 1.0)],
        )
        .unwrap();
        assert_eq!(
            l.rep_state(3_000_000, 1, &arms),
            RepState::Cut(vec!["lmdb".into()])
        );
        let k = Kill {
            arm: "supdb".into(),
            size: 3_000_000,
            rep: 1,
            after_phase: Some("load".into()),
            rss_bytes: Some(1),
            cache_bytes: None,
            utc: "now".into(),
        };
        l.write_kill(&k).unwrap();
        assert_eq!(
            l.rep_state(3_000_000, 1, &arms),
            RepState::Cut(vec!["supdb".into(), "lmdb".into()]),
            "both recorded, in the plan's order"
        );
        assert_eq!(
            l.read_kill(3_000_000, 1, "supdb").unwrap().as_ref(),
            Some(&k)
        );
        assert_eq!(l.read_pass(3_000_000, 1, "supdb").unwrap(), None);
        assert_eq!(l.read_pass(3_000_000, 1, "lmdb").unwrap().unwrap().len(), 1);
        // Every arm recorded is still a cut rep: the rep's file is the mark.
        l.write_rep(
            3_000_000,
            1,
            &[m("page-floor", "floor", None, "pages_per_s", 2.0)],
        )
        .unwrap();
        assert_eq!(l.rep_state(3_000_000, 1, &arms), RepState::Whole);
        assert_eq!(l.read_rep(3_000_000, 1).unwrap().unwrap().len(), 1);
        // Another rep is untouched by this one.
        assert_eq!(l.rep_state(3_000_000, 2, &arms), RepState::Cut(vec![]));
        l.write_pass(3_000_000, 2, "supdb", &[]).unwrap();
        l.forget_rep(3_000_000, 2, &arms).unwrap();
        assert_eq!(l.rep_state(3_000_000, 2, &arms), RepState::Cut(vec![]));
        assert_eq!(l.rep_state(3_000_000, 1, &arms), RepState::Whole);
        let _ = std::fs::remove_dir_all(&out);
    }

    /// The floors come back in sitting order, the head reads back as
    /// written with its resumptions, and a directory without a head is
    /// refused as a ledger.
    #[test]
    fn floors_and_head_round_trip_and_a_headless_directory_is_not_a_ledger() {
        let out = scratch("head");
        let mut h = head();
        let l = Ledger::create(&out, &h).unwrap();
        assert_eq!(l.out(), Some(out.as_path()));
        l.write_floors(0, &[m("wal-floor", "floor", None, "ops_per_s", 1.0)])
            .unwrap();
        l.write_floors(1, &[m("wal-floor", "floor", None, "ops_per_s", 2.0)])
            .unwrap();
        let f = l.read_floors().unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[1][0].samples, vec![2.0]);
        h.resumptions.push(Resumption {
            utc: "later".into(),
            size: 3_000_000,
            rep: 2,
        });
        l.write_head(&h).unwrap();
        let (_, back) = Ledger::open(l.dir()).unwrap();
        assert_eq!(back.resumptions, h.resumptions);
        assert_eq!(back.sha, h.sha);
        assert_eq!(back.plan.arms, h.plan.arms);
        let bare = scratch("bare");
        match Ledger::open(&bare) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}"),
            Ok(_) => panic!("a directory with no head opened as a ledger"),
        }
        l.remove().unwrap();
        assert!(!out
            .join(INFLIGHT_DIR)
            .join("20260101T000000Z-abc1234")
            .exists());
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(&bare);
    }
}
