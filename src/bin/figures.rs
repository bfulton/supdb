//! Turn committed results into publication-quality figures.
//!
//! Reads `results/*.json` and writes standalone SVGs to `figures/`, plus an
//! index page that gathers them. Nothing here re-runs the engine: figures are
//! derived from the recorded measurements, so a reviewer can check that a
//! published chart follows from the data without trusting this program.

use std::path::{Path, PathBuf};
use supdb::bench::plot::{Bars, Chart, Series};
use supdb::bench::{jparse, J};

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let arg = |n: &str, d: &str| -> String {
        argv.iter()
            .position(|a| a == n)
            .and_then(|i| argv.get(i + 1))
            .cloned()
            .unwrap_or_else(|| d.into())
    };
    let results = PathBuf::from(arg("--results", "results"));
    let outdir = PathBuf::from(arg("--out", "figures"));
    let profile = arg("--profile", "ci");
    std::fs::create_dir_all(&outdir)?;

    let mut made: Vec<(String, String)> = Vec::new();
    let mut emit = |name: &str, title: &str, svg: String| -> std::io::Result<()> {
        let p = outdir.join(format!("{name}.svg"));
        std::fs::write(&p, svg)?;
        println!("# wrote {}", p.display());
        made.push((name.to_string(), title.to_string()));
        Ok(())
    };

    if let Some(d) = load(&results, "ext-ycsb", &profile) {
        emit(
            "ext-ycsb",
            "YCSB core workloads across the field",
            fig_ycsb(&d),
        )?;
    }
    if let Some(d) = load(&results, "ext-kv", &profile) {
        emit(
            "ext-kv",
            "Load, read and scan across the field",
            fig_extkv(&d),
        )?;
    }
    if let Some(d) = load(&results, "f1-outofcore", &profile) {
        emit(
            "f1-outofcore",
            "Read latency once the dataset outgrows memory",
            fig_outofcore(&d),
        )?;
    }

    let idx = outdir.join("index.html");
    std::fs::write(&idx, index_html(&made, &profile))?;
    println!("# wrote {}", idx.display());
    if made.is_empty() {
        println!(
            "# no results at profile '{profile}'; run `internal all --profile {profile}` first"
        );
    }
    Ok(())
}

fn load(dir: &Path, exp: &str, profile: &str) -> Option<J> {
    let text = std::fs::read_to_string(dir.join(format!("{exp}.{profile}.json"))).ok()?;
    jparse::parse(&text).ok()
}

/// Figure 6. Warm against cold, as distributions rather than two numbers.
fn fig_outofcore(d: &J) -> String {
    let curve = |k: &str| -> Vec<(f64, f64)> {
        d.path(&format!("series.{k}.cdf"))
            .map(|s| s.items())
            .unwrap_or(&[])
            .iter()
            .filter_map(|p| {
                let pc = p.num("p")?;
                if pc >= 100.0 || pc <= 0.0 {
                    return None;
                }
                Some((1.0 / (1.0 - pc / 100.0), p.num("ms")?.max(1e-6)))
            })
            .collect()
    };
    let mut c = Chart::new(
        "Read latency once the dataset outgrows memory",
        "1 / (1 - percentile)",
        "read latency (ms)",
    )
    .subtitle("mmap with no madvise: no readahead control, no async I/O, no eviction policy")
    .log_x()
    .log_y()
    .add(Series::new("resident", curve("resident")))
    .add(Series::new("out of core", curve("cold")));
    let b = curve("ballasted");
    if !b.is_empty() {
        c = c.add(Series::new("cache squeezed", b));
    }
    let honest = d
        .num("series.cache_control.drop_caches_succeeded")
        .unwrap_or(0.0)
        > 0.0
        || d.path("series.cache_control.drop_caches_succeeded")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    c.caption(if honest {
        "Page cache dropped between phases. The gap is what the engine pays when it has to \
         reach storage, which no published number measures."
    } else {
        "WARNING: drop_caches did not succeed on this run, so the 'cold' series was served from \
         page cache and is not a cold measurement. Recorded rather than hidden."
    })
    .to_svg()
}

/// External figure: YCSB throughput, one bar per engine within each workload.
fn fig_ycsb(d: &J) -> String {
    let rows = d.path("series.workloads").map(|s| s.items()).unwrap_or(&[]);
    let mut groups: Vec<String> = Vec::new();
    let mut engines: Vec<String> = Vec::new();
    for r in rows {
        if let Some(w) = r.path("workload").and_then(|v| v.as_str()) {
            // Keep the YCSB letter; the descriptive tail does not fit an axis.
            let short = w.split('-').next().unwrap_or(w).to_string();
            if !groups.contains(&short) {
                groups.push(short);
            }
        }
        if let Some(e) = r.path("engine").and_then(|v| v.as_str()) {
            if !engines.contains(&e.to_string()) {
                engines.push(e.to_string());
            }
        }
    }
    let mut b = Bars::new(
        "YCSB core workloads: Supdb against the field",
        "throughput (ops/s)",
    )
    .subtitle("A 50/50 update-heavy, B 95/5, C read-only, D read-latest, E short scans, F read-modify-write")
    .log_y()
    .groups(groups.clone());
    for e in &engines {
        let vals: Vec<f64> = groups
            .iter()
            .map(|g| {
                rows.iter()
                    .find(|r| {
                        r.path("engine").and_then(|v| v.as_str()) == Some(e.as_str())
                            && r.path("workload")
                                .and_then(|v| v.as_str())
                                .is_some_and(|w| w.starts_with(g.as_str()))
                    })
                    .and_then(|r| r.num("ops_per_s"))
                    .unwrap_or(0.0)
            })
            .collect();
        b = b.add(e, vals);
    }
    b.caption(
        "Log scale. Every engine runs the same workload definitions, key distribution and batch \
         size. Supdb provides one of six guarantees the others provide five or six of -- durable \
         commit, transactions, checksums, reopen-for-write, read-your-writes, ordered scan -- so \
         this compares promises as well as implementations.",
    )
    .to_svg()
}

/// External figure: the redb benchmark shape across the field.
fn fig_extkv(d: &J) -> String {
    let rows = d.path("series.engines").map(|s| s.items()).unwrap_or(&[]);
    let groups: Vec<String> = vec![
        "bulk load".into(),
        "random read".into(),
        "range scan".into(),
    ];
    let mut b = Bars::new(
        "Load, read and scan: Supdb against the field",
        "operations/s",
    )
    .subtitle("workload shape follows redb's own benchmark; all engines native, no JNI")
    .log_y()
    .groups(groups);
    for r in rows {
        let name = r.path("engine").and_then(|v| v.as_str()).unwrap_or("?");
        b = b.add(
            name,
            vec![
                r.num("load_ops_per_s").unwrap_or(0.0),
                r.num("read_ops_per_s").unwrap_or(0.0),
                r.num("scan_entries_per_s").unwrap_or(0.0),
            ],
        );
    }
    b.caption(
        "Log scale. The design document reports Supdb ahead of LMDB on warm reads, measured \
         through a Java harness with an adapter it separately found to allocate per value and \
         open a transaction per lookup. Measured natively, the ordering reverses.",
    )
    .to_svg()
}

fn index_html(made: &[(String, String)], profile: &str) -> String {
    let mut s = String::from(
        "<!doctype html><meta charset=utf8><title>Supdb figures</title>\
         <style>:root{--bg:#f6f7fa;--fg:#151a24;--fg2:#525d74;--card:#fff;--rule:#d9dde8}\
         @media(prefers-color-scheme:dark){:root{--bg:#0d1017;--fg:#e3e8f2;--fg2:#9aa5bc;--card:#141924;--rule:#28303f}}\
         body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.6 'IBM Plex Sans',system-ui,sans-serif;padding:40px 24px}\
         main{max-width:900px;margin:0 auto;display:flex;flex-direction:column;gap:28px}\
         h1{font-size:26px;margin:0}p.sub{color:var(--fg2);margin:0}\
         figure{margin:0;background:var(--card);border:1px solid var(--rule);border-radius:6px;padding:18px;overflow-x:auto}\
         figcaption{font-size:12.5px;color:var(--fg2);margin-top:8px}\
         img{max-width:100%;height:auto;display:block}</style><main>",
    );
    s.push_str(&format!(
        "<div><h1>Supdb internal benchmarks</h1><p class=sub>Figures generated from <code>results/*.{profile}.json</code>. \
         Profile <strong>{profile}</strong>{}.</p></div>",
        if profile == "full" { "" } else { " &mdash; not citable evidence" }
    ));
    for (name, title) in made {
        s.push_str(&format!(
            "<figure><img src=\"{name}.svg\" alt=\"{title}\"><figcaption>{title}</figcaption></figure>"
        ));
    }
    s.push_str("</main>");
    s
}
