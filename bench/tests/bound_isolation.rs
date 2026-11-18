//! A pass's bound holds the pass's own process and nothing of the run's
//! (`bound::Bound::pass`): a pairing whose own process holds a hundred
//! megabytes of ballast (`run::PARENT_BALLAST`) records each pass's group
//! at its start (`run::AUDIT_WORKLOAD`) under the few megabytes a pass in
//! a group of its own begins with, and above nothing, which is the record
//! that the pass ran in its group at all. Where this host has no cgroup v1
//! memory controller to write -- cgroup v2 alone, the hosted runners --
//! the test says so and checks nothing, since a v2 group is bounded by
//! whoever starts the run; the v2 layouts are held by `bound`'s own tests.

use std::process::Command;

fn v1_memory_controller() -> bool {
    std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|t| {
        t.lines().any(|l| {
            l.split(':')
                .nth(1)
                .is_some_and(|c| c.split(',').any(|c| c == "memory"))
        })
    })
}

#[test]
fn a_pass_bound_sees_nothing_the_run_holds() {
    if !v1_memory_controller() {
        eprintln!("no cgroup v1 memory controller here; the per-pass bound is not exercised");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("supdb-next-boundiso-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let ballast = 100u64 << 20;
    let out = Command::new(env!("CARGO_BIN_EXE_bench"))
        .args([
            "ab",
            "--arms",
            "supdb,supdb",
            "--size",
            "20000",
            "--reps",
            "1",
            "--passes",
            "load",
            "--bound",
            &(256u64 << 20).to_string(),
        ])
        .env("TMPDIR", &tmp)
        .env("SUPDB_BENCH_PARENT_BALLAST", ballast.to_string())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "bench ab: {}\n{stdout}\n{stderr}",
        out.status
    );
    assert!(
        stderr.contains("each pass in a group of its own"),
        "the pairing bounded each pass on its own: {stderr}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("bound-start group_bytes"))
        .unwrap_or_else(|| panic!("no audit line in the table:\n{stdout}"));
    let f: Vec<f64> = line
        .split_whitespace()
        .skip(2)
        .take(2)
        .map(|x| x.parse().unwrap())
        .collect();
    for b in &f {
        assert!(
            *b > 0.0 && *b < (32u64 << 20) as f64,
            "a pass began with {b} bytes charged to its bound, with {ballast} held by the run: {line}"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
