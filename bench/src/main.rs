//! `bench run` measures and writes a row; `bench machine` prints the host as
//! a row would record it.

use std::path::PathBuf;
use supdb_bench::{engines, env, figures, gate, row, run, Scale};

const USAGE: &str = "\
usage:
  bench run --scale quick|full [--out DIR] [--arms a,b,...] [--top KEYS] [--reps N]
  bench gate ROW.json [--runs DIR]
  bench figures [--runs DIR] [--out DIR] [--scale quick|full]
  bench machine

run    measures every arm over the size ladder and writes runs/<scale>/<utc>-<sha7>.json
       --out   directory holding runs/ (default: runs)
       --arms  comma-separated subset of the arms (default: all)
       --top   the ladder's top rung in keys (default: quick 300000; full sized to 1.5x memory)
       --reps  repetitions per size and arm (default: quick 5, full 7)
gate   compares ROW to the last ten rows of its class and scale under runs/ (--runs, default runs);
       exits 1 if any quantity is worse than every one of them
figures draws every figure for the latest row of each class at --scale (default full) into
       --out (default figures), from --runs (default runs)
machine prints the machine fields as a row records them, and its derived class";

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
        eprintln!("--scale quick|full\n\n{USAGE}");
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

fn cmd_run(a: Args) -> i32 {
    let Some(scale) = a.get("--scale").and_then(Scale::parse) else {
        eprintln!("--scale quick|full is required\n\n{USAGE}");
        return 2;
    };
    let machine = env::capture();
    let arms: Vec<String> = match a.get("--arms") {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        None => engines::ARMS.iter().map(|s| s.to_string()).collect(),
    };
    if arms.is_empty() {
        eprintln!("--arms names no arm\n\n{USAGE}");
        return 2;
    }
    let top = a.num("--top").unwrap_or(match scale {
        // Measured once, then fixed: 160 s with every arm, the YCSB mixes,
        // the floors and five reps on a 4-core VM; about three minutes on a
        // GitHub runner.
        Scale::Quick => 300_000,
        Scale::Full => run::full_top(machine.mem_total_kb, 100),
    });
    let mut plan = run::Plan::new(scale, arms, top);
    if let Some(r) = a.num("--reps") {
        // Rep 0 is the warmup and is not recorded, so a plan needs at least
        // one more or it writes a row with no measurements.
        if r < 1 {
            eprintln!("--reps must be at least 1\n\n{USAGE}");
            return 2;
        }
        plan.reps = r as usize;
    }
    let out = PathBuf::from(a.get("--out").unwrap_or("runs"));

    eprintln!(
        "bench run: scale {} on {} ({} keys top, {} reps, arms {})",
        scale.as_str(),
        machine.cpu_model,
        top,
        plan.reps,
        plan.arms.join(",")
    );
    let mut log = |s: &str| eprintln!("{s}");
    match run::run(&plan, machine, &mut log) {
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
