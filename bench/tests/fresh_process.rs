//! A pass in a process of its own answers the row the pass in this process
//! answers (`Plan::fresh_process`): the same quantities, each with the
//! same number of samples, from the smallest rung of the quick scale, one
//! rep after the warmup. The child is this crate's own binary, `bench
//! one-pass SPEC`, so the test runs it as `run` does.

use std::path::Path;
use std::process::Command;

fn bench() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bench"))
}

/// The row a run wrote under `out`: (workload, arm, size, quantity) to its
/// sample count.
fn row_keys(
    out: &Path,
) -> std::collections::BTreeMap<(String, String, Option<u64>, String), usize> {
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
        .map(|m| {
            (
                (
                    m["workload"].as_str().unwrap().to_string(),
                    m["arm"].as_str().unwrap().to_string(),
                    m["size"].as_u64(),
                    m["quantity"].as_str().unwrap().to_string(),
                ),
                m["samples"].as_array().unwrap().len(),
            )
        })
        .collect()
}

fn run_quick(out: &Path, mode: &str) {
    std::fs::create_dir_all(out).unwrap();
    let status = bench()
        .args([
            "run", "--scale", "quick", "--top", "10000", "--reps", "1", "--arms", "supdb", mode,
            "--out",
        ])
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "bench run {mode}: {status}");
}

#[test]
fn a_pass_in_its_own_process_answers_the_row_the_pass_in_this_one_does() {
    let root = std::env::temp_dir().join(format!("supdb-next-freshproc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let fresh = root.join("fresh");
    let one = root.join("one");
    run_quick(&fresh, "--fresh-process");
    run_quick(&one, "--one-process");
    let a = row_keys(&fresh);
    let b = row_keys(&one);
    assert!(
        a.keys()
            .any(|(_, arm, size, _)| arm == "supdb" && *size == Some(10000)),
        "the arm's quantities are in the row: {a:?}"
    );
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "the same quantities either way"
    );
    for (k, n) in &a {
        assert_eq!(
            *n, b[k],
            "the same sample count either way: {k:?} has {n} and {}",
            b[k]
        );
        if k.1 == "supdb" {
            assert_eq!(
                *n, 1,
                "one sample an arm quantity after the warmup: {k:?} has {n}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The child command refuses to run without its spec, or with one it
/// cannot read, with a status `run` tells apart from the bound's kill.
#[test]
fn the_pass_command_without_its_spec_is_refused() {
    let status = bench().arg("one-pass").status().unwrap();
    assert_eq!(status.code(), Some(2));
    let status = bench()
        .args(["one-pass", "/nonexistent/supdb-next-no-such-spec.json"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
