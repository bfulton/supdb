//! The recovery workloads run in the suite and write their quantities into
//! a row: `recover` and `recover-shuffled`, the open of a store its writer
//! was killed over and the first read through it, for the engine and for a
//! comparator of each kind. The writer is this crate's own binary, `bench
//! crash-load SPEC`, so the test runs the workloads as `run` does.
//!
//! What the workload holds the reopened store to -- a prefix of whole
//! batches and no staged tail, and for a durable arm every batch -- fails
//! the run, so a row here is a row whose stores passed it; what a buffered
//! arm's process took with it is a quantity of the row, and so is what the
//! arm that holds its log in the process lost, which is the point of it.

use std::path::Path;
use std::process::Command;

fn bench() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bench"))
}

/// The samples of every quantity of `workload` in the one row under `out`,
/// by (arm, quantity).
fn samples(out: &Path, workload: &str) -> std::collections::BTreeMap<(String, String), Vec<f64>> {
    let dir = out.join("quick");
    let mut rows: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no rows under {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(rows.len(), 1, "one row under {}: {rows:?}", dir.display());
    let text = std::fs::read_to_string(rows.remove(0)).unwrap();
    let row: serde_json::Value = serde_json::from_str(&text).unwrap();
    row["measurements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["workload"] == workload)
        .map(|m| {
            (
                (
                    m["arm"].as_str().unwrap().to_string(),
                    m["quantity"].as_str().unwrap().to_string(),
                ),
                m["samples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn a_row_carries_both_recoveries_for_every_arm() {
    let root = std::env::temp_dir().join(format!("supdb-next-recover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // Both guarantees, the engine and a comparator of each kind. A durable
    // arm that lost a batch it acknowledged fails the run, so a row at all
    // is every durable store whole.
    let durable = ["supdb", "lmdb", "rocksdb-tuned"];
    let buffered = ["supdb-ingest", "lmdb-nosync", "rocksdb-tuned-nosync"];
    // And the arm of the third guarantee that reopens a store: the log
    // held in the process until a sync. `faster`, beside it in the roster,
    // runs no recovery workload.
    let held = "supdb-held";
    let mut arms = [durable, buffered].concat();
    arms.push(held);
    let arms = arms.join(",");
    let status = bench()
        .args([
            "run", "--scale", "quick", "--top", "10000", "--reps", "1", "--arms", &arms, "--out",
        ])
        .arg(&root)
        .env("TMPDIR", &root)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "bench run with the recovery workloads: {status}"
    );
    for workload in ["recover", "recover-shuffled"] {
        let got = samples(&root, workload);
        // One sample a quantity, the rep after the warmup, as every
        // quantity of the row.
        let one = |arm: &str, quantity: &str| -> Option<f64> {
            let s = got.get(&(arm.to_string(), quantity.to_string()))?;
            assert_eq!(s.len(), 1, "{workload} {quantity} for {arm}: {s:?}");
            Some(s[0])
        };
        for arm in durable.iter().chain(&buffered).chain([&held]) {
            let open = one(arm, "open_ms")
                .unwrap_or_else(|| panic!("no {workload} open_ms for {arm}: {got:?}"));
            assert!(
                open.is_finite() && open > 0.0,
                "{workload} open_ms for {arm}: {open}"
            );
        }
        for arm in durable {
            let read = one(arm, "first_read_us")
                .unwrap_or_else(|| panic!("no {workload} first_read_us for {arm}: {got:?}"));
            assert!(
                read.is_finite() && read > 0.0,
                "{workload} first read for {arm}: {read}"
            );
            assert_eq!(
                one(arm, "unrecovered_batches"),
                None,
                "a durable arm records no loss: it fails on one"
            );
        }
        for arm in buffered {
            let lost = one(arm, "unrecovered_batches")
                .unwrap_or_else(|| panic!("no {workload} unrecovered_batches for {arm}: {got:?}"));
            // Every engine hands every commit to the kernel, which outlives
            // the process, whatever it left unsynced: a batch gone is an
            // engine that held acknowledged commits in its own buffers --
            // supdb's ordered ingest did, up to a megabyte of them -- or a
            // workload that killed the wrong thing or read the wrong store.
            assert_eq!(lost, 0.0, "{workload}: {arm} lost acknowledged batches");
            // The first read is of the last key committed before the kill,
            // which is there.
            assert!(
                one(arm, "first_read_us").is_some(),
                "{workload}: no first read for {arm}"
            );
        }
        // The held log's arm is the one whose loss is its guarantee: the
        // run did not fail on it, the row says how much it was, and the
        // first read is recorded exactly where the last key came back. Its
        // shuffled load goes through the log, and at this rung nothing
        // seals, spills or rotates before the kill, so the process took
        // the load with it: an arm that lost nothing there wrote its log
        // at its commits, which is `supdb-ingest` under another name and
        // no peer of FASTER's. Its ordered load goes to a run's own file,
        // which this says nothing of.
        let lost = one(held, "unrecovered_batches")
            .unwrap_or_else(|| panic!("no {workload} unrecovered_batches for {held}: {got:?}"));
        assert!(
            lost.is_finite() && lost >= 0.0,
            "{workload}: {held} lost {lost}"
        );
        assert_eq!(
            one(held, "first_read_us").is_some(),
            lost == 0.0,
            "{workload}: {held} lost {lost} batches and its first read of the last key"
        );
        if workload == "recover-shuffled" {
            assert!(
                lost > 0.0,
                "{workload}: {held} was killed holding its log and lost nothing"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_crash_load_with_no_spec_or_no_store_is_refused() {
    let status = bench().arg("crash-load").status().unwrap();
    assert_eq!(status.code(), Some(2), "crash-load with no spec: {status}");
    let status = bench()
        .args([
            "crash-load",
            "/nonexistent/supdb-next-no-such-crash-spec.json",
        ])
        .status()
        .unwrap();
    assert_eq!(
        status.code(),
        Some(2),
        "crash-load with a missing spec: {status}"
    );
}
