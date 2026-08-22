//! Turn committed results into publication-quality figures.
//!
//! Reads `results/*.json` and writes standalone SVGs to `figures/`, plus an
//! index page that gathers them. Nothing here re-runs the engine: figures are
//! derived from the recorded measurements, so a reviewer can check that a
//! published chart follows from the data without trusting this program.

use std::path::{Path, PathBuf};
use supdb::bench::plot::{Chart, Series};
use supdb::bench::{jparse, J};

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let arg = |n: &str, d: &str| -> String {
        argv.iter().position(|a| a == n).and_then(|i| argv.get(i + 1)).cloned().unwrap_or_else(|| d.into())
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

    if let Some(d) = load(&results, "f2-open", &profile) {
        emit("f2-open-cost", "Reader open cost against key count", fig_open_cost(&d))?;
        emit("f2-amortization", "Cost per read against reads per process", fig_amortization(&d))?;
    }
    if let Some(d) = load(&results, "f6-threads", &profile) {
        emit("f6-thread-scaling", "Write throughput against writer threads", fig_threads(&d))?;
    }
    if let Some(d) = load(&results, "f4-durability", &profile) {
        emit("f4-durability", "Throughput against the data-loss window", fig_durability(&d))?;
    }
    if let Some(d) = load(&results, "f5-latency", &profile) {
        emit("f5-latency-cdf", "Append latency distribution", fig_latency(&d))?;
    }
    if let Some(d) = load(&results, "f1-outofcore", &profile) {
        emit("f1-outofcore", "Read latency, warm against cold", fig_outofcore(&d))?;
    }

    let idx = outdir.join("index.html");
    std::fs::write(&idx, index_html(&made, &profile))?;
    println!("# wrote {}", idx.display());
    if made.is_empty() {
        println!("# no results at profile '{profile}'; run `internal all --profile {profile}` first");
    }
    Ok(())
}

fn load(dir: &Path, exp: &str, profile: &str) -> Option<J> {
    let text = std::fs::read_to_string(dir.join(format!("{exp}.{profile}.json"))).ok()?;
    jparse::parse(&text).ok()
}

/// Figure 1. Open cost against key count, with a linear reference.
///
/// The claim under test is that open is independent of key count, as it would
/// be for an index read through a shared mapping. The reference line is what
/// exactly-linear growth looks like from the first measured point.
fn fig_open_cost(d: &J) -> String {
    let pts: Vec<(f64, f64)> = d
        .path("series.scaling")
        .map(|s| s.items())
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| Some((p.num("keys")?, p.num("open_ms")?)))
        .collect();
    let band: Vec<(f64, f64, f64)> = d
        .path("series.scaling")
        .map(|s| s.items())
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            Some((p.num("keys")?, p.num("open_samples.ci95_lo")?, p.num("open_samples.ci95_hi")?))
        })
        .collect();
    let linear: Vec<(f64, f64)> = match (pts.first(), pts.last()) {
        (Some((x0, y0)), Some((x1, _))) => vec![(*x0, *y0), (*x1, y0 * (x1 / x0))],
        _ => vec![],
    };
    Chart::new("Reader open cost scales with key count", "keys in the store", "open (ms)")
        .subtitle("Reader::build materialises the whole key index per process")
        .log_x()
        .log_y()
        .add(Series::new("measured", pts).with_band(band))
        .add(Series::new("linear", linear).as_reference())
        .caption(
            "Median of interleaved repetitions; band is the 95% bootstrap interval of the median. \
             A shared mmap-able index would be a flat line. Every read benchmark in the design \
             document calls Reader::open before starting its timer, so this cost appears nowhere.",
        )
        .to_svg()
}

/// Figure 2. The amortization curve, one series per store size.
fn fig_amortization(d: &J) -> String {
    let mut c = Chart::new(
        "A short-lived reader process pays for the whole index",
        "reads performed by the process",
        "cost per read (us)",
    )
    .subtitle("total wall time including process spawn and index build, divided by reads")
    .log_x()
    .log_y();
    for p in d.path("series.scaling").map(|s| s.items()).unwrap_or(&[]) {
        let keys = p.num("keys").unwrap_or(0.0);
        let pts: Vec<(f64, f64)> = p
            .path("process_curve")
            .map(|s| s.items())
            .unwrap_or(&[])
            .iter()
            .filter_map(|q| Some((q.num("reads")?, q.num("us_per_read_incl_open")?)))
            .collect();
        if !pts.is_empty() {
            c = c.add(Series::new(&format!("{} keys", human(keys)), pts));
        }
    }
    // The asymptote every curve is falling towards.
    if let Some(p) = d.path("series.scaling").and_then(|s| s.items().last()) {
        if let (Some(sr), Some(first), Some(last)) = (
            p.num("steady_read_us"),
            p.path("process_curve").and_then(|c| c.at(0)).and_then(|q| q.num("reads")),
            p.path("process_curve").and_then(|c| c.items().last()).and_then(|q| q.num("reads")),
        ) {
            c = c.add(Series::new("steady state", vec![(first, sr), (last, sr)]).as_reference());
        }
    }
    c.caption(
        "Below the break-even point the open dominates and the published read throughput does \
         not describe the workload. Many short-lived reader processes was uppend's founding \
         premise, and is the regime this design is worst at.",
    )
    .to_svg()
}

/// Figure 3. Thread scaling against ideal.
fn fig_threads(d: &J) -> String {
    let rows = d.path("series.scaling").map(|s| s.items()).unwrap_or(&[]);
    let pts: Vec<(f64, f64)> =
        rows.iter().filter_map(|p| Some((p.num("threads")?, p.num("ops_per_s")?))).collect();
    let band: Vec<(f64, f64, f64)> = rows
        .iter()
        .filter_map(|p| Some((p.num("threads")?, p.num("samples.ci95_lo")?, p.num("samples.ci95_hi")?)))
        .collect();
    let ideal: Vec<(f64, f64)> = match pts.first() {
        Some((_, y0)) => pts.iter().map(|(x, _)| (*x, y0 * x)).collect(),
        None => vec![],
    };
    Chart::new("Write throughput does not scale with writer threads", "writer threads", "appends/s")
        .subtitle("the appender mutex is taken inside seal_shard's per-extent loop")
        .add(Series::new("measured", pts).with_band(band))
        .add(Series::new("linear", ideal).as_reference())
        .caption(
            "Median of interleaved repetitions; band is the 95% bootstrap interval. Every \
             db_bench comparison in the design document is single-threaded, which is the \
             configuration in which this does not appear.",
        )
        .to_svg()
}

/// Figure 4. The durability curve.
fn fig_durability(d: &J) -> String {
    let rows = d.path("series.durability_curve").map(|s| s.items()).unwrap_or(&[]);
    let pts: Vec<(f64, f64)> = rows
        .iter()
        .filter_map(|p| Some((p.num("ops_at_risk")?.max(1.0), p.num("ops_per_s")?)))
        .collect();
    let band: Vec<(f64, f64, f64)> = rows
        .iter()
        .filter_map(|p| {
            Some((p.num("ops_at_risk")?.max(1.0), p.num("samples.ci95_lo")?, p.num("samples.ci95_hi")?))
        })
        .collect();
    Chart::new(
        "Throughput is bought with the data a crash would destroy",
        "operations at risk between checkpoints",
        "appends/s",
    )
    .subtitle("checkpoint() rewrites the entire key index, not what changed")
    .log_x()
    .add(Series::new("Supdb", pts).with_band(band))
    .caption(
        "The rightmost point takes no checkpoint before close, so the whole run is at risk -- \
         the configuration every published fillrandom number was measured in. A usable engine \
         needs a point on the left of this curve, which requires an incremental checkpoint.",
    )
    .to_svg()
}

/// Figure 5. The distribution behind the mean.
fn fig_latency(d: &J) -> String {
    let pts: Vec<(f64, f64)> = d
        .path("series.append_cdf")
        .map(|s| s.items())
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| {
            let pc = p.num("p")?;
            // A percentile axis is read in "nines"; 100 is not plottable there.
            if pc >= 100.0 || pc <= 0.0 {
                return None;
            }
            Some((1.0 / (1.0 - pc / 100.0), p.num("ms")?.max(1e-6)))
        })
        .collect();
    let mean = d.num("series.append_latency.mean_ms").unwrap_or(0.0);
    let meanline: Vec<(f64, f64)> = match (pts.first(), pts.last()) {
        (Some((x0, _)), Some((x1, _))) if mean > 0.0 => vec![(*x0, mean), (*x1, mean)],
        _ => vec![],
    };
    Chart::new("The mean is not a summary of append cost", "1 / (1 - percentile)", "append latency (ms)")
        .subtitle("x = 10 is p90, 100 is p99, 1000 is p99.9")
        .log_x()
        .log_y()
        .add(Series::new("append", pts))
        .add(Series::new("mean", meanline).as_reference())
        .caption(
            "Latency measured per call. The distance between the curve and the dashed mean is \
             the cost inline merging and whole-index checkpoints impose on an unlucky caller, \
             and is invisible in every number the design document publishes.",
        )
        .to_svg()
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
        "Read latency once the page cache is gone",
        "1 / (1 - percentile)",
        "read latency (ms)",
    )
    .subtitle("mmap with no madvise: no readahead control, no async I/O, no eviction policy")
    .log_x()
    .log_y()
    .add(Series::new("warm", curve("warm")))
    .add(Series::new("cold", curve("cold")));
    let b = curve("ballasted");
    if !b.is_empty() {
        c = c.add(Series::new("cache squeezed", b));
    }
    let honest = d.num("series.cache_control.drop_caches_succeeded").unwrap_or(0.0) > 0.0
        || d.path("series.cache_control.drop_caches_succeeded").and_then(|v| v.as_bool()).unwrap_or(false);
    c.caption(if honest {
        "Page cache dropped between phases. The gap is what the engine pays when it has to \
         reach storage, which no published number measures."
    } else {
        "WARNING: drop_caches did not succeed on this run, so the 'cold' series was served from \
         page cache and is not a cold measurement. Recorded rather than hidden."
    })
    .to_svg()
}

fn human(v: f64) -> String {
    if v >= 1e6 {
        format!("{:.0}M", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.0}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
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
