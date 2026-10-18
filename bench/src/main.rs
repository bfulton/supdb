//! `bench run` measures and writes a row; `bench machine` prints the host as
//! a row would record it.

use std::path::PathBuf;
use supdb_bench::bound::Bound;
use supdb_bench::{engines, env, figures, gate, row, run, Scale};

const USAGE: &str = "\
usage:
  bench run --scale quick|full [--out DIR] [--arms a,b,...] [--top KEYS] [--bottom KEYS] [--reps N]
  bench run --scale ooc --bound BYTES [--ops-div N] [the options above]
  bench ab --arms A,B [--size KEYS] [--reps N] [--len ENTRIES] [--pairs TEXT] [--bound BYTES] [--passes a,b,...]
  bench gate ROW.json [--runs DIR]
  bench figures [--runs DIR] [--out DIR] [--scale quick|full|ooc]
  bench machine

run    measures every arm over the size ladder and writes runs/<scale>/<utc>-<sha7>.json
       --out   directory holding runs/ (default: runs)
       --arms  comma-separated subset of the arms (default: all; at ooc the bounded roster)
       --top   the ladder's top rung in keys (default: quick 300000; full sized to 1.5x memory;
               ooc 10000000)
       --bottom the ladder's bottom rung in keys (default: the whole ladder from 10000; ooc 3000000)
       --reps  repetitions per size and arm (default: quick 5, full 7, ooc 3)
       --bound the memory the run may use, in bytes, enforced by a cgroup of the run's own:
               required at ooc, refused at quick and full. The process is moved into the
               group before anything is measured, the limit as read back goes into the row's
               class, and a bound too small for the floors is refused before the floors
       --ops-div what the read passes' operation counts are divided by at ooc (default 30);
               loads and bursts are never divided, and no divided count goes below 1000
ab     two arms in one process, alternating and paired rep by rep, for choosing
       between them rather than for the series: the order swaps every other rep
       so neither stands first more often, and the verdict is a sign test over
       the pairs. Reports the counts an engine keeps beside the timings.
       --size  keys (default 100000)   --reps  pairs after the warmup (default 12)
       --len   entries a scan takes (default 100). Swept, it separates what a
               scan pays once from what it pays per entry.
       --pairs every pair, rep by rep, of each quantity whose name holds TEXT.
       --bound as for run: the pairing under a memory bound, with the counts per
               operation a bounded row records paired like any other quantity.
       --passes a comma-separated subset of load,read,read-threaded,scan,scan-threaded,
               mixes,scan-mixed,lag (default: all), for a question one phase answers
               at a rung where the whole pass is an hour.
       An arm against itself (--arms supdb,supdb) is the control: what a
       verdict looks like when nothing differs.
gate   compares ROW to the last ten rows of its class and scale under runs/ (--runs, default runs);
       exits 1 if any quantity is worse than every one of them
figures draws every figure for the latest row of each class at --scale (default full) into
       --out (default figures), from --runs (default runs)
machine prints the machine fields as a row records them, and its derived class";

/// The bound an argument names, checked and not yet entered: `None` with
/// no `--bound`, the bytes otherwise, or the refusal. A bound below
/// `MIN_BOUND_BYTES` is refused, since the floors cannot run under it,
/// and it is the known-red input `scripts/ooc.sh --prove-red` proves its
/// gate on; one that is not a whole number of mebibytes is refused too,
/// since the row's class names the bound in exact mebibytes
/// (`Row::class`). Checked before anything is written, so a bad number
/// never costs the scan floor's file.
fn bound_arg(a: &Args) -> Result<Option<u64>, String> {
    let Some(text) = a.get("--bound") else {
        return Ok(None);
    };
    let Some(bytes) = a.num("--bound") else {
        return Err(format!("--bound takes a number of bytes, not {text:?}"));
    };
    if bytes < run::MIN_BOUND_BYTES {
        return Err(format!(
            "refused: --bound {bytes} is below the {} bytes the floors alone need",
            run::MIN_BOUND_BYTES
        ));
    }
    if bytes % (1 << 20) != 0 {
        return Err(format!(
            "--bound {bytes} is not a whole number of mebibytes"
        ));
    }
    Ok(Some(bytes))
}

/// The bound entered: the group, or the refusal, which has the shape of
/// the ENOSPC refusal in `run` -- before anything is measured, and never
/// a row (or a pairing) without the bound in it. The counters a bounded
/// phase's counts are read from are probed once here, so a count that
/// cannot be read refuses by name before the floors rather than
/// recording a zero.
fn enter_bound(bytes: u64) -> Result<Bound, String> {
    let b = Bound::enter(bytes).map_err(|e| format!("refused: could not enter the bound: {e}"))?;
    // The group's path on stderr, for the wrapper that removes it if the
    // run is killed inside it (`scripts/ooc.sh`).
    eprintln!(
        "bench: bound {} bytes in group {}",
        b.limit(),
        b.group().display()
    );
    run::probe_counts(&b).map_err(|e| format!("refused: {e}"))?;
    Ok(b)
}

struct Args(Vec<String>);
impl Args {
    fn get(&self, n: &str) -> Option<&str> {
        self.0
            .iter()
            .position(|a| a == n)
            .and_then(|i| self.0.get(i + 1))
            .map(|s| s.as_str())
    }
    fn num(&self, n: &str) -> Option<u64> {
        self.get(n).and_then(|v| v.replace('_', "").parse().ok())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("run") => cmd_run(Args(args[1..].to_vec())),
        Some("ab") => cmd_ab(Args(args[1..].to_vec())),
        Some("gate") => cmd_gate(Args(args[1..].to_vec())),
        Some("figures") => cmd_figures(Args(args[1..].to_vec())),
        Some("machine") => cmd_machine(),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

fn cmd_machine() -> i32 {
    let m = env::capture();
    match serde_json::to_string_pretty(&m) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    }
    let probe = row::Row {
        utc: String::new(),
        sha: String::new(),
        rustc: String::new(),
        scale: Scale::Quick,
        machine: m,
        measurements: vec![],
    };
    println!("class: {}", probe.class());
    0
}

fn cmd_figures(a: Args) -> i32 {
    let runs = PathBuf::from(a.get("--runs").unwrap_or("runs"));
    let out = PathBuf::from(a.get("--out").unwrap_or("figures"));
    let scale = a
        .get("--scale")
        .map(Scale::parse)
        .unwrap_or(Some(Scale::Full));
    let Some(scale) = scale else {
        eprintln!("--scale quick|full|ooc\n\n{USAGE}");
        return 2;
    };
    match figures::render_all(&runs, &out, scale) {
        Ok(w) if w.is_empty() => {
            println!(
                "no {} rows under {}; nothing drawn",
                scale.as_str(),
                runs.display()
            );
            0
        }
        Ok(w) => {
            for r in &w {
                println!("{}  {}", r.path.display(), r.title);
            }
            0
        }
        Err(e) => {
            eprintln!("figures: {e}");
            1
        }
    }
}

fn cmd_gate(a: Args) -> i32 {
    let Some(path) = a.0.first().filter(|s| !s.starts_with("--")) else {
        eprintln!("gate needs the row to check\n\n{USAGE}");
        return 2;
    };
    let runs = PathBuf::from(a.get("--runs").unwrap_or("runs"));
    let row = match row::Row::read(std::path::Path::new(path)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            return 2;
        }
    };
    match gate::gate(&row, &runs) {
        Ok(rep) => {
            print!("{}", rep.render());
            i32::from(rep.regressed())
        }
        Err(e) => {
            eprintln!("gate: {e}");
            2
        }
    }
}

fn cmd_ab(a: Args) -> i32 {
    let arms: Vec<String> = a
        .get("--arms")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let [a0, b0] = &arms[..] else {
        eprintln!("--arms takes exactly two, as A,B\n\n{USAGE}");
        return 2;
    };
    for arm in [a0, b0] {
        if engines::guarantee(arm).is_none() {
            eprintln!("no arm named {arm}\n\n{USAGE}");
            return 2;
        }
    }
    let size = a.num("--size").unwrap_or(100_000);
    let reps = a.num("--reps").unwrap_or(12).max(1) as usize;
    let passes = match a.get("--passes") {
        Some(list) => match run::Passes::parse(list) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}\n\n{USAGE}");
                return 2;
            }
        },
        None => run::Passes::all(),
    };
    // The bound first, before the machine capture and the plan's threads:
    // a cgroup v1 move takes the whole thread group, and the pairing's
    // every allocation is to be the group's.
    let bound = match bound_arg(&a).and_then(|b| b.map(enter_bound).transpose()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bench ab: {e}");
            return 2;
        }
    };
    let mut plan = run::Plan::new(
        Scale::Quick,
        vec![a0.clone(), b0.clone()],
        size,
        env::capture().cpus,
    );
    plan.bound = bound;
    plan.passes = passes;
    if let Some(l) = a.num("--len") {
        plan.scan_len = (l as usize).max(1);
    }
    eprintln!(
        "bench ab: {a0} against {b0} at {size} keys, scans of {}, {reps} pairs after a warmup{}{}",
        plan.scan_len,
        match plan.bound_bytes() {
            Some(b) => format!(", under a bound of {b} bytes"),
            None => String::new(),
        },
        if plan.passes == run::Passes::all() {
            String::new()
        } else {
            format!(", passes {}", a.get("--passes").unwrap_or_default())
        }
    );
    let mut log = std::io::stderr();
    let got = match run::ab((a0, b0), size, reps, &plan, &mut log) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("bench ab: {e}");
            return 1;
        }
    };
    println!(
        "\n{:<34} {:>13} {:>13} {:>8} {:>9} {:>8}",
        "quantity", a0, b0, "B/A", "B above A", "sign p"
    );
    let held = run::holm(&got.iter().map(|q| q.p).collect::<Vec<_>>(), 0.05);
    for (q, held) in got.iter().zip(&held) {
        // A ratio is only the size of the thing; the sign test is whether
        // there is a thing. Marked where a coin would give this or worse
        // less than one time in twenty, and marked twice where that holds
        // for the table as a whole: an arm against itself marks one or
        // two of its three dozen quantities once.
        let ratio = if q.a != 0.0 { q.b / q.a } else { f64::NAN };
        println!(
            "{:<34} {:>13.4} {:>13.4} {:>7.3}x {:>5}/{:<3} {:>8.3}{}",
            q.quantity,
            q.a,
            q.b,
            ratio,
            q.up,
            q.n,
            q.p,
            match (held, q.p < 0.05) {
                (true, _) => "  **",
                (false, true) => "  *",
                _ => "",
            }
        );
    }
    let marked = got.iter().filter(|q| q.p < 0.05).count();
    println!(
        "\n{} quantities: {marked} marked *, where a coin marks about {:.1}; {} marked **, \
         which holds for the table as a whole (Holm)",
        got.len(),
        got.len() as f64 * 0.05,
        held.iter().filter(|h| **h).count()
    );
    if let Some(text) = a.get("--pairs") {
        for q in got.iter().filter(|q| q.quantity.contains(text)) {
            println!("\n{} by rep (A first on even reps):", q.quantity);
            for (i, (x, y)) in q.pairs.iter().enumerate() {
                let r = if *x != 0.0 { y / x } else { f64::NAN };
                println!("  rep {:>3}  {:>15.2} {:>15.2}  {:>7.3}x", i + 1, x, y, r);
            }
        }
    }
    0
}

fn cmd_run(a: Args) -> i32 {
    let Some(scale) = a.get("--scale").and_then(Scale::parse) else {
        eprintln!("--scale quick|full|ooc is required\n\n{USAGE}");
        return 2;
    };
    // The bound is the ooc scale's and nothing else's: a quick or full row
    // under one would land in its class with the bound in it, and the
    // bounded scale without one would measure the machine's memory and
    // call it a bound.
    let bounded = scale == Scale::Ooc;
    match (bounded, a.get("--bound")) {
        (true, None) => {
            eprintln!("--bound BYTES is required at --scale ooc\n\n{USAGE}");
            return 2;
        }
        (false, Some(_)) => {
            eprintln!("--bound is for --scale ooc alone\n\n{USAGE}");
            return 2;
        }
        _ => {}
    }
    if !bounded && a.get("--ops-div").is_some() {
        eprintln!("--ops-div is for --scale ooc alone\n\n{USAGE}");
        return 2;
    }
    let ops_div = match a.get("--ops-div") {
        Some(_) => match a.num("--ops-div") {
            Some(d) if d >= 1 => d,
            _ => {
                eprintln!("--ops-div takes a divisor of at least 1\n\n{USAGE}");
                return 2;
            }
        },
        None => run::OOC_OPS_DIV,
    };
    // The bound's number, checked before anything is written: a bound
    // that is refused costs no scan floor's file, and the floor file is
    // written and synced before the bound is entered.
    let bound_bytes = match bound_arg(&a) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bench run: {e}");
            return 1;
        }
    };
    let arms: Vec<String> = match a.get("--arms") {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        None if bounded => engines::OOC_ARMS.iter().map(|s| s.to_string()).collect(),
        None => engines::ARMS.iter().map(|s| s.to_string()).collect(),
    };
    if arms.is_empty() {
        eprintln!("--arms names no arm\n\n{USAGE}");
        return 2;
    }
    // The top before the bound, since the scan floor's file is sized by
    // it and written before the bound is entered; the full scale's top is
    // sized from the machine, read here once and again under the bound
    // below, where the row's capture is.
    let top = a.num("--top").unwrap_or(match scale {
        // Measured once, then fixed: 160 s with every arm, the YCSB mixes,
        // the floors and five reps on a 4-core VM; about three minutes on a
        // GitHub runner.
        Scale::Quick => 300_000,
        Scale::Full => run::full_top(env::capture().mem_total_kb, 100),
        // The two rungs either side of a bound of a gigabyte or two at
        // the suite's hundred-and-sixteen bytes a record.
        Scale::Ooc => 10_000_000,
    });
    let bottom = match a.num("--bottom") {
        Some(b) => {
            // Refused rather than run, because a bottom above the top
            // leaves an empty ladder, and a row with no measurements in it
            // is a run that reports having measured.
            if supdb_bench::ladder_from(b, top).is_empty() {
                eprintln!(
                    "--bottom {b} is above the top rung {top}, so the ladder is empty\n\n{USAGE}"
                );
                return 2;
            }
            Some(b)
        }
        None if bounded => Some(3_000_000),
        None => None,
    };
    let reps = match a.num("--reps") {
        // Rep 0 is the warmup and is not recorded, so a plan needs at least
        // one more or it writes a row with no measurements.
        Some(r) if r < 1 => {
            eprintln!("--reps must be at least 1\n\n{USAGE}");
            return 2;
        }
        r => r,
    };
    let out = PathBuf::from(a.get("--out").unwrap_or("runs"));

    // The root and, under a bound, the scan floor's file first: the file
    // is the top rung's bytes, written and synced outside the group and
    // dropped from the cache, so the group's first charge is the floors'
    // own reads of it and not a gigabyte of dirty pages the bound would
    // have to write back to make room for them.
    let root = match run::prepare_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bench run: {e}");
            return 1;
        }
    };
    if bounded {
        let bytes = run::scan_floor_bytes(top, run::VALUE_SIZE);
        let path = match run::write_scan_floor(&root, bytes) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("bench run: the scan floor's file: {e}");
                return 1;
            }
        };
        // Dropped here and not by the writer: the unbounded floor keeps
        // its pages, and the advice refuses off Linux.
        if let Err(e) = run::drop_cache(&path) {
            eprintln!("bench run: the scan floor's file: {e}");
            return 1;
        }
    }
    // Then the bound: before the machine capture, the plan's threads and
    // every allocation the row will charge to it. Its refusal has the
    // ENOSPC refusal's shape -- before the floors, with no row written --
    // so a bounded row is never written without its bound in it, and the
    // fresh root goes with the refusal, as `run` removes it at its end.
    let bound = match bound_bytes.map(enter_bound).transpose() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bench run: {e}");
            let _ = std::fs::remove_dir_all(&root);
            return 1;
        }
    };
    let mut machine = env::capture();
    machine.mem_limit_bytes = bound.as_ref().map(Bound::limit);
    let mut plan = run::Plan::new(scale, arms, top, machine.cpus);
    plan.bound = bound;
    plan.ops_div = if bounded { ops_div } else { 1 };
    if let Some(b) = bottom {
        plan.bottom = b;
    }
    if let Some(r) = reps {
        plan.reps = r as usize;
    }

    // The rungs, not just the top: with a bottom set, the top alone does not
    // say what was measured, and this line is the only record of the plan a
    // run that never finishes leaves behind.
    let rungs = supdb_bench::ladder_from(plan.bottom, top);
    eprintln!(
        "bench run: scale {} on {} ({} reps, rungs {}, arms {}{})",
        scale.as_str(),
        machine.cpu_model,
        plan.reps,
        rungs
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(","),
        plan.arms.join(","),
        match machine.mem_limit_bytes {
            Some(b) => format!(", bound {b} bytes, ops divided by {}", plan.ops_div),
            None => String::new(),
        }
    );
    let mut log = |s: &str| eprintln!("{s}");
    match run::run(&plan, machine, &mut log, Some(&out), &root) {
        Ok(row) => match row.write(&out) {
            Ok(p) => {
                println!("{}", p.display());
                0
            }
            Err(e) => {
                eprintln!("could not write the row: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("run failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bound that is not a whole number of mebibytes is refused naming
    /// it, since the class names the bound in exact mebibytes; a whole one
    /// passes, and one below the floors' need is refused as such first.
    #[test]
    fn a_bound_not_in_whole_mebibytes_is_refused() {
        let args = |b: &str| Args(vec!["--bound".to_string(), b.to_string()]);
        assert_eq!(
            bound_arg(&args("1610612737")).unwrap_err(),
            "--bound 1610612737 is not a whole number of mebibytes"
        );
        assert_eq!(bound_arg(&args("1610612736")).unwrap(), Some(1610612736));
        let low = bound_arg(&args("67108864")).unwrap_err();
        assert!(
            low.starts_with("refused: --bound 67108864 is below"),
            "{low}"
        );
        let text = bound_arg(&args("64MiB")).unwrap_err();
        assert!(text.contains("takes a number"), "{text}");
        assert_eq!(bound_arg(&Args(vec![])).unwrap(), None);
    }
}
