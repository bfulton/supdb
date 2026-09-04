//! Check committed results against committed claims.
//!
//! This is the mechanism that keeps the rigorous edge from regressing. Every
//! statement the project makes about itself is written down in `claims.json`
//! with the state it is expected to be in, and this program checks the
//! recorded measurements against it. CI runs it, so a change that alters the
//! engine's behaviour cannot land while the documentation still says otherwise.
//!
//! It is deliberately symmetric. A finding that was expected to fail and now
//! passes is reported just as loudly as the reverse -- because either the
//! engine improved and the claim is stale, or the experiment stopped testing
//! anything. Both need a human. A "not exercised" finding where the claim
//! expected a real result is also a failure: an untested hazard must never
//! read as a green build.

use std::path::{Path, PathBuf};
use supdb::bench::{jparse, Status, J};

struct Outcome {
    failures: Vec<String>,
    checked: usize,
    skipped: Vec<String>,
}

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let arg = |n: &str, d: &str| -> String {
        argv.iter()
            .position(|a| a == n)
            .and_then(|i| argv.get(i + 1))
            .cloned()
            .unwrap_or_else(|| d.into())
    };
    let claims_path = PathBuf::from(arg("--claims", "claims.json"));
    let results = PathBuf::from(arg("--results", "results"));
    // Which profile's results to check. CI checks `ci`; a release checks `full`.
    let profile = arg("--profile", "ci");
    let strict = argv.iter().any(|a| a == "--strict");

    let text = std::fs::read_to_string(&claims_path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("cannot read {}: {e}", claims_path.display()),
        )
    })?;
    let claims = jparse::parse(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let mut out = Outcome {
        failures: Vec::new(),
        checked: 0,
        skipped: Vec::new(),
    };

    println!(
        "verifying claims in {} against {}/*.{}.json\n",
        claims_path.display(),
        results.display(),
        profile
    );

    check_findings(&claims, &results, &profile, &mut out);
    check_metrics(&claims, &results, &profile, &mut out);
    check_unregistered(&claims, &results, &profile, &mut out);

    println!("\n{} claim(s) checked", out.checked);
    if !out.skipped.is_empty() {
        println!(
            "{} skipped (no result file at this profile, or a precondition the host cannot meet):",
            out.skipped.len()
        );
        for s in &out.skipped {
            println!("  - {s}");
        }
        if strict {
            println!("\n--strict: a skipped claim is a failure");
            out.failures
                .extend(out.skipped.iter().map(|s| format!("skipped: {s}")));
        }
    }
    if out.failures.is_empty() {
        println!("\nOK: every claim matches the recorded results.");
        Ok(())
    } else {
        println!(
            "\n{} CLAIM(S) DO NOT MATCH THE RESULTS:",
            out.failures.len()
        );
        for f in &out.failures {
            println!("  x {f}");
        }
        println!(
            "\nEither the engine changed and the claim is stale, or the experiment stopped\n\
             testing what it says it tests. Both need a decision, not a re-run."
        );
        std::process::exit(1);
    }
}

fn load(results: &Path, experiment: &str, profile: &str) -> Option<J> {
    let p = results.join(format!("{experiment}.{profile}.json"));
    let text = std::fs::read_to_string(p).ok()?;
    jparse::parse(&text).ok()
}

fn check_findings(claims: &J, results: &Path, profile: &str, out: &mut Outcome) {
    let Some(list) = claims.path("findings") else {
        return;
    };
    for c in list.items() {
        let exp = c.path("experiment").and_then(|v| v.as_str()).unwrap_or("");
        let id = c.path("id").and_then(|v| v.as_str()).unwrap_or("");
        let want = c.path("expect").and_then(|v| v.as_str()).unwrap_or("holds");
        // A claim may pin itself to one profile. An out-of-core finding cannot
        // hold at `ci`, where the dataset is 64MB on a 16GB machine, and a
        // claim that ignored that would either fail every CI run or have to be
        // written so loosely it checked nothing.
        if let Some(want_profile) = c.path("profile").and_then(|v| v.as_str()) {
            if want_profile != profile {
                continue;
            }
        }
        let label = format!("{exp}/{id}");

        let Some(doc) = load(results, exp, profile) else {
            out.skipped.push(label);
            continue;
        };
        // Architecture pins for the same reason profile pins exist. Cache line
        // size and page size differ across targets -- Graviton is 64B/4KiB like
        // x86, Apple Silicon is 128B/16KiB -- so a layout threshold calibrated
        // on one is not a claim about the other, and checking it there would
        // fail for a reason that says nothing about the engine.
        if let Some(want_arch) = c.path("arch").and_then(|v| v.as_str()) {
            if doc.path("env.arch").and_then(|v| v.as_str()) != Some(want_arch) {
                continue;
            }
        }
        out.checked += 1;

        let found = doc
            .path("findings")
            .map(|f| f.items())
            .unwrap_or(&[])
            .iter()
            .find(|f| f.path("id").and_then(|v| v.as_str()) == Some(id));

        let Some(f) = found else {
            out.failures.push(format!(
                "{label}: claimed but the experiment recorded no such finding"
            ));
            continue;
        };
        let got = f.path("status").and_then(|v| v.as_str()).unwrap_or("");
        let detail = f.path("detail").and_then(|v| v.as_str()).unwrap_or("");

        let want_status = Status::from_str(want);
        let got_status = Status::from_str(got);

        if got_status == Some(Status::NotExercised) && want_status != Some(Status::NotExercised) {
            // A claim may name a capability of the *host* that its experiment
            // needs -- `drop_caches` wants root, which a hosted CI runner does
            // not have. Where the run says it could not reach the condition,
            // the claim is skipped rather than failed: failing it would report
            // a fact about the machine as a fact about the engine, and the
            // gate would be red everywhere the capability is missing. Rule 3
            // makes the finding `not_exercised`; this is the other half.
            if let Some(needs) = c.path("needs").and_then(|v| v.as_str()) {
                out.checked -= 1;
                out.skipped
                    .push(format!("{label} (needs {needs} on this host)"));
                continue;
            }
            out.failures.push(format!(
                "{label}: claim expects '{want}' but the run did not exercise it -- {detail}"
            ));
            continue;
        }
        if got == want {
            let mark = if got == "fails" {
                "known-failing"
            } else {
                "ok"
            };
            println!("  [{mark}] {label}: {}", short(detail));
        } else {
            out.failures.push(format!(
                "{label}: expected '{want}', recorded '{got}' -- {detail}"
            ));
        }
    }
}

/// The other direction: a finding a run reported that no claim registers.
///
/// `check_findings` walks claims and looks for results, which cannot see a
/// finding nobody claimed -- and an unclaimed finding is the exact thing this
/// file exists to prevent, a measurement with no recorded expected state. It
/// hid two different faults at once: findings the suites emit and nobody ever
/// adjudicated, and findings left in a committed result by an experiment that
/// has since stopped emitting them, which is a result file describing an
/// engine that no longer exists.
///
/// Registration is what is checked, not adjudication at this profile: a claim
/// pinned to `full` or to one architecture still registers its finding
/// everywhere, so pins are ignored here.
fn check_unregistered(claims: &J, results: &Path, profile: &str, out: &mut Outcome) {
    let mut registered: Vec<(String, String)> = Vec::new();
    if let Some(list) = claims.path("findings") {
        for c in list.items() {
            let exp = c.path("experiment").and_then(|v| v.as_str()).unwrap_or("");
            let id = c.path("id").and_then(|v| v.as_str()).unwrap_or("");
            registered.push((exp.to_string(), id.to_string()));
        }
    }
    let suffix = format!(".{profile}.json");
    let Ok(dir) = std::fs::read_dir(results) else {
        return;
    };
    let mut files: Vec<String> = dir
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(&suffix))
        .collect();
    files.sort();
    for name in files {
        let exp = &name[..name.len() - suffix.len()];
        let Ok(text) = std::fs::read_to_string(results.join(&name)) else {
            continue;
        };
        let Ok(doc) = jparse::parse(&text) else {
            continue;
        };
        for f in doc.path("findings").map(|f| f.items()).unwrap_or(&[]) {
            let id = f.path("id").and_then(|v| v.as_str()).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            if !registered
                .iter()
                .any(|(e, i)| e == exp && i == id)
            {
                let status = f.path("status").and_then(|v| v.as_str()).unwrap_or("");
                out.failures.push(format!(
                    "{exp}/{id}: the run recorded this finding ('{status}') and no claim registers it"
                ));
            }
        }
    }
}

fn check_metrics(claims: &J, results: &Path, profile: &str, out: &mut Outcome) {
    let Some(list) = claims.path("metrics") else {
        return;
    };
    for c in list.items() {
        let exp = c.path("experiment").and_then(|v| v.as_str()).unwrap_or("");
        let path = c.path("path").and_then(|v| v.as_str()).unwrap_or("");
        // Metrics pin to a profile for the same reason findings do: a
        // throughput floor calibrated at ci scale is meaningless at full,
        // where a single 66-second checkpoint dominates the run.
        if let Some(want_profile) = c.path("profile").and_then(|v| v.as_str()) {
            if want_profile != profile {
                continue;
            }
        }
        let label = format!("{exp}:{path}");

        let Some(doc) = load(results, exp, profile) else {
            out.skipped.push(label);
            continue;
        };
        if let Some(want_arch) = c.path("arch").and_then(|v| v.as_str()) {
            if doc.path("env.arch").and_then(|v| v.as_str()) != Some(want_arch) {
                continue;
            }
        }
        out.checked += 1;

        let Some(v) = doc.num(path) else {
            out.failures
                .push(format!("{label}: path not present in the result"));
            continue;
        };
        if let Some(min) = c.num("min") {
            if v < min {
                out.failures
                    .push(format!("{label}: {v:.3} is below the floor {min:.3}"));
                continue;
            }
        }
        if let Some(max) = c.num("max") {
            if v > max {
                out.failures
                    .push(format!("{label}: {v:.3} is above the ceiling {max:.3}"));
                continue;
            }
        }
        println!("  [ok] {label} = {v:.3}");
    }
}

fn short(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 110 {
        format!("{}...", one.chars().take(107).collect::<String>())
    } else {
        one
    }
}
