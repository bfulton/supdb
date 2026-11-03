//! A run cut short is taken up from its ledger (`bench run --resume`), a
//! cut rep is taken again whole, and a pass killed under the bound is a
//! cell of the row and not the end of the run. The runs are this crate's
//! own binary at the smallest rung of the quick scale, each pass in a
//! process of its own, the cut and the kill chosen through the runner's
//! seams (`SUPDB_BENCH_STOP_AFTER`, `SUPDB_BENCH_KILL_AFTER`) rather than
//! waited for. Every run gets a temp directory of its own, since a run
//! sweeps every stale working directory under its temp directory first.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bench() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bench"))
}

/// A directory of this test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-resume-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `bench run` at ten thousand keys over `arms`, each pass in a process
/// of its own, under `env`, with its stores under `tmp`.
fn run(
    out: &Path,
    tmp: &Path,
    arms: &str,
    reps: &str,
    env: &[(&str, &str)],
    extra: &[&str],
) -> Output {
    std::fs::create_dir_all(out).unwrap();
    let mut c = bench();
    c.args([
        "run",
        "--scale",
        "quick",
        "--top",
        "10000",
        "--reps",
        reps,
        "--arms",
        arms,
        "--fresh-process",
        "--out",
    ])
    .arg(out)
    .args(extra)
    .env("TMPDIR", tmp);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn resume(dir: &Path, tmp: &Path, extra: &[&str]) -> Output {
    bench()
        .args(["run", "--resume"])
        .arg(dir)
        .args(extra)
        .env("TMPDIR", tmp)
        .output()
        .unwrap()
}

fn text(o: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&o.stdout).to_string(),
        String::from_utf8_lossy(&o.stderr).to_string(),
    )
}

/// The one row under `dir`, parsed.
fn row_in(dir: &Path) -> serde_json::Value {
    let mut rows: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("no rows under {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    assert_eq!(rows.len(), 1, "one row under {}: {rows:?}", dir.display());
    serde_json::from_str(&std::fs::read_to_string(rows.remove(0)).unwrap()).unwrap()
}

/// (workload, arm, quantity) to its sample count.
fn counts(row: &serde_json::Value) -> BTreeMap<(String, String, String), usize> {
    row["measurements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                (
                    m["workload"].as_str().unwrap().to_string(),
                    m["arm"].as_str().unwrap().to_string(),
                    m["quantity"].as_str().unwrap().to_string(),
                ),
                m["samples"].as_array().unwrap().len(),
            )
        })
        .collect()
}

/// The one ledger under `out/inflight`.
fn ledger_in(out: &Path) -> PathBuf {
    let dir = out.join("inflight");
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no ledger under {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(v.len(), 1, "one ledger under {}: {v:?}", dir.display());
    v.remove(0)
}

/// A run cut after its first rep is taken up at its second, and the row
/// it composes is the uninterrupted run's for every arm quantity, with
/// the floors of both sittings and the sitting recorded; the ledger is
/// gone once the row is written.
#[test]
fn a_run_cut_between_reps_is_taken_up_at_the_next_and_composes_the_row() {
    let base = scratch("between");
    let (out, tmp) = (base.join("out"), base.join("tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cut = run(
        &out,
        &tmp,
        "supdb",
        "2",
        &[("SUPDB_BENCH_STOP_AFTER", "10000-1")],
        &[],
    );
    let (_, err) = text(&cut);
    assert_eq!(cut.status.code(), Some(1), "the cut run fails: {err}");
    assert!(
        err.contains("SUPDB_BENCH_STOP_AFTER: stopped after rep 1 of 10000"),
        "{err}"
    );
    let ledger = ledger_in(&out);
    for f in [
        "head.json",
        "floors-0.json",
        "pass-10000-1-supdb.json",
        "rep-10000-1.json",
    ] {
        assert!(ledger.join(f).exists(), "{f} in the ledger");
    }
    assert!(!ledger.join("pass-10000-2-supdb.json").exists());
    assert!(
        !out.join("quick").exists(),
        "no row banked from a run of no whole rung"
    );

    let taken = resume(&ledger, &tmp, &[]);
    let (_, err) = text(&taken);
    assert!(taken.status.success(), "the resume: {err}");
    assert!(
        err.contains("taken up at rep 2 of 10000, sitting 2"),
        "{err}"
    );
    assert!(
        err.contains("rep 2  supdb"),
        "rep 2 ran in the resumed run: {err}"
    );
    assert!(
        !err.contains("rep 1  supdb"),
        "rep 1 was replayed, not run: {err}"
    );
    assert!(!ledger.exists(), "the ledger goes with the row");
    let row = row_in(&out.join("quick"));
    let r = row["resumptions"]
        .as_array()
        .expect("the sitting is recorded");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["size"], 10000);
    assert_eq!(r[0]["rep"], 2);
    assert!(row.get("killed").is_none());

    // Against the same run in one sitting.
    let plain = base.join("plain");
    let one = run(&plain, &tmp, "supdb", "2", &[], &[]);
    assert!(one.status.success(), "{}", text(&one).1);
    let p = row_in(&plain.join("quick"));
    assert!(
        p.get("resumptions").is_none(),
        "a run in one sitting records none"
    );
    let (a, b) = (counts(&row), counts(&p));
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "the same quantities"
    );
    for (k, n) in &a {
        if k.1 == "floor" {
            // Two sittings of floors, each a warmup and two reps, against one.
            if k.0 == "page-floor" {
                assert_eq!(
                    (*n, b[k]),
                    (6, 4),
                    "{k:?}: the start floors twice, the reps' floors once"
                );
            } else {
                assert_eq!((*n, b[k]), (4, 2), "{k:?}: floors of both sittings");
            }
        } else {
            assert_eq!((*n, b[k]), (2, 2), "{k:?}: two reps either way");
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// A rep cut between two arms is taken again whole from its first arm,
/// since the arms are interleaved within a rep; with `--resume-passes`
/// the passes it holds are kept and only the rest run.
#[test]
fn a_rep_cut_between_arms_is_taken_again_whole_unless_passes_are_kept() {
    let base = scratch("within");
    let (out, tmp) = (base.join("out"), base.join("tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cut = run(
        &out,
        &tmp,
        "supdb,lmdb",
        "1",
        &[("SUPDB_BENCH_STOP_AFTER", "10000-1-supdb")],
        &[],
    );
    let (_, err) = text(&cut);
    assert_eq!(cut.status.code(), Some(1), "{err}");
    assert!(
        err.contains("stopped after the pass of supdb at rep 1 of 10000"),
        "{err}"
    );
    let ledger = ledger_in(&out);
    assert!(ledger.join("pass-10000-1-supdb.json").exists());
    assert!(!ledger.join("pass-10000-1-lmdb.json").exists());
    assert!(!ledger.join("rep-10000-1.json").exists(), "the rep is cut");

    // Kept: lmdb runs, supdb is replayed.
    let kept_base = scratch("kept");
    let kept_out = kept_base.join("out");
    std::fs::create_dir_all(&kept_out).unwrap();
    copy_dir(&out, &kept_out);
    let kept = resume(&ledger_in(&kept_out), &tmp, &["--resume-passes"]);
    let (_, err) = text(&kept);
    assert!(kept.status.success(), "{err}");
    assert!(err.contains("rep 1  lmdb"), "lmdb ran: {err}");
    assert!(!err.contains("rep 1  supdb"), "supdb was kept: {err}");
    assert!(!err.contains("taken again whole"), "{err}");
    let row = row_in(&kept_out.join("quick"));
    let c = counts(&row);
    assert_eq!(c[&("read".into(), "supdb".into(), "reads_per_s".into())], 1);
    assert_eq!(c[&("read".into(), "lmdb".into(), "reads_per_s".into())], 1);

    // Whole: both arms run again, in the plan's order.
    let whole = resume(&ledger, &tmp, &[]);
    let (_, err) = text(&whole);
    assert!(whole.status.success(), "{err}");
    assert!(
        err.contains("rep 1 at 10000 was cut after supdb; taken again whole"),
        "{err}"
    );
    let supdb_at = err.find("rep 1  supdb").expect("supdb ran again");
    let lmdb_at = err.find("rep 1  lmdb").expect("lmdb ran");
    assert!(supdb_at < lmdb_at, "supdb first, as the plan orders them");
    let row = row_in(&out.join("quick"));
    let c = counts(&row);
    assert_eq!(c[&("read".into(), "supdb".into(), "reads_per_s".into())], 1);
    assert_eq!(c[&("read".into(), "lmdb".into(), "reads_per_s".into())], 1);
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&kept_base);
}

/// A resume is held to the ledger's binary and refuses a plan of its own.
#[test]
fn a_resume_refuses_another_binary_and_a_plan_of_its_own() {
    let base = scratch("refuse");
    let (out, tmp) = (base.join("out"), base.join("tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let cut = run(
        &out,
        &tmp,
        "supdb",
        "1",
        &[("SUPDB_BENCH_STOP_AFTER", "10000-1-supdb")],
        &[],
    );
    assert_eq!(cut.status.code(), Some(1), "{}", text(&cut).1);
    let ledger = ledger_in(&out);
    let with_arms = resume(&ledger, &tmp, &["--arms", "lmdb"]);
    assert_eq!(with_arms.status.code(), Some(2), "{}", text(&with_arms).1);
    assert!(text(&with_arms)
        .1
        .contains("takes its plan from the ledger"));
    let fewer = resume(&ledger, &tmp, &["--reps", "0"]);
    assert_eq!(fewer.status.code(), Some(2), "{}", text(&fewer).1);
    let other_out = resume(&ledger, &tmp, &["--out", "elsewhere"]);
    assert_eq!(other_out.status.code(), Some(2), "{}", text(&other_out).1);
    // Another binary: the head's stamp, changed by hand.
    let head = ledger.join("head.json");
    let mut h: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&head).unwrap()).unwrap();
    h["sha"] = serde_json::Value::String("0000000deadbeef".into());
    std::fs::write(&head, serde_json::to_string_pretty(&h).unwrap()).unwrap();
    let other = resume(&ledger, &tmp, &[]);
    let (_, err) = text(&other);
    assert_eq!(other.status.code(), Some(1), "{err}");
    assert!(
        err.contains("refused") && err.contains("0000000deadbeef"),
        "{err}"
    );
    assert!(ledger.exists(), "a refused resume leaves the ledger");
    let _ = std::fs::remove_dir_all(&base);
}

/// A pass killed in its own process is a cell of the row: the run goes
/// on to the next arm, the row carries the kill with the phase it fell
/// after, it is written under `killed/` and not beside the series, and
/// the command exits with the kill's status.
#[test]
fn a_killed_pass_is_a_cell_of_the_row_and_the_run_goes_on() {
    let base = scratch("killed");
    let (out, tmp) = (base.join("out"), base.join("tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let o = run(
        &out,
        &tmp,
        "supdb,lmdb",
        "1",
        &[("SUPDB_BENCH_KILL_AFTER", "supdb:load")],
        &[],
    );
    let (stdout, err) = text(&o);
    assert_eq!(o.status.code(), Some(137), "{err}");
    assert!(
        err.contains("supdb           KILLED under the bound after load"),
        "{err}"
    );
    assert!(
        err.contains("rep 1  lmdb"),
        "the run went on to lmdb: {err}"
    );
    assert!(
        err.contains("killed: supdb at rep 1 of 10000 keys after load"),
        "{err}"
    );
    assert!(
        stdout.contains("killed/quick/"),
        "the row's path names killed/: {stdout}"
    );
    assert!(
        !out.join("quick").exists(),
        "nothing beside the series: {err}"
    );
    let row = row_in(&out.join("killed").join("quick"));
    let k = row["killed"].as_array().expect("the kill is in the row");
    assert_eq!(k.len(), 1);
    assert_eq!(k[0]["arm"], "supdb");
    assert_eq!(k[0]["size"], 10000);
    assert_eq!(k[0]["rep"], 1);
    assert_eq!(k[0]["after_phase"], "load");
    let c = counts(&row);
    assert!(c.keys().any(|k| k.1 == "lmdb"), "lmdb's cells are kept");
    assert!(!c.keys().any(|k| k.1 == "supdb"), "the killed arm has none");
    assert!(!out.join("inflight").join("x").exists());
    assert!(
        std::fs::read_dir(out.join("inflight"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "the ledger goes with the row"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A pairing under fresh processes loses a killed rep's pair and goes
/// on, and says so with the kill's status.
#[test]
fn a_pairing_loses_a_killed_reps_pair_and_goes_on() {
    let base = scratch("ab");
    let tmp = base.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let o = bench()
        .args([
            "ab",
            "--arms",
            "supdb,lmdb",
            "--size",
            "10000",
            "--reps",
            "2",
            "--passes",
            "load,read",
            "--fresh-process",
        ])
        .env("TMPDIR", &tmp)
        .env("SUPDB_BENCH_KILL_AFTER", "supdb:load:1")
        .output()
        .unwrap();
    let (stdout, err) = text(&o);
    assert_eq!(o.status.code(), Some(137), "{err}");
    assert!(
        err.contains("rep 1/2  supdb KILLED under the bound after load; the rep's pair is lost"),
        "{err}"
    );
    assert!(
        err.contains("1 of 2 reps killed under the bound: rep 1 supdb after load"),
        "{err}"
    );
    assert!(
        stdout.contains("read reads_per_s"),
        "the table is the pairs that ran: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}
