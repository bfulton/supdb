//! Figures: one program draws every figure from `runs/`, so a figure that
//! disagrees with the data is a bug here and not a stale file.
//!
//! The rules are DESIGN.md's, which are Doumont's: one message per figure
//! and the message is the title; a curve per arm over the size ladder,
//! never a bar; two axes and nothing else; direct labels, no legend; the
//! CI as a light band; one typeface, two sizes; black on white.
//!
//! Colour was computed, not eyeballed. An all-grey ladder failed the
//! normal-vision separation check between its two lightest greys (ΔE 13.4,
//! floor 15). Ink for the default, one accent for the shipping option, and
//! one grey for the comparators passes it (17.1) and the colour-blind check
//! (14.3) with every mark at 3:1 against the surface; the two comparators
//! share the grey and differ by dash and by their label. The validator also
//! reports that the palette is not a saturated categorical one, which is
//! true and intended.

use crate::gate::higher_is_better;
use crate::row::{Guarantee, Row};
use crate::run::{FLOOR_ARM, KEY_SIZE, VALUE_SIZE};
use crate::stats::{Samples, CI_CONF, CI_RESAMPLES};
use crate::Scale;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

const INK: &str = "#1b1b1b";
const ACCENT: &str = "#2b6cb0";
const GREY: &str = "#8c8c8c";
const RULE: &str = "#c8c8c8";
const MUTED: &str = "#6b6b6b";
const FONT: &str = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif";

const W: f64 = 760.0;
const H: f64 = 420.0;
const LEFT: f64 = 72.0;
const RIGHT: f64 = 150.0;
const TOP: f64 = 88.0;
const BOTTOM: f64 = 48.0;

/// How an arm is drawn: colour and dash. The default in ink, the shipping
/// option in the accent, comparators in grey with the second dashed.
fn style(arm: &str) -> (&'static str, &'static str) {
    match arm {
        "supdb" | "supdb-ingest" => (INK, ""),
        "supdb-noadvice" => (ACCENT, ""),
        "lmdb" | "lmdb-nosync" => (GREY, ""),
        "rocksdb-tuned" | "rocksdb-nosync" => (GREY, "7,4"),
        _ => (GREY, "2,3"),
    }
}

/// The comparator's name as a reader knows it.
fn pretty(arm: &str) -> &'static str {
    match arm {
        "supdb" => "supdb",
        "supdb-noadvice" => "supdb (no advice)",
        "supdb-ingest" => "supdb (buffered)",
        "lmdb" => "LMDB",
        "lmdb-nosync" => "LMDB (nosync)",
        "rocksdb-tuned" => "RocksDB",
        "rocksdb-nosync" => "RocksDB (nosync)",
        _ => "?",
    }
}

fn workload_noun(w: &str) -> &'static str {
    match w {
        "load" => "Ordered load",
        "load-shuffled" => "Shuffled load",
        "read" => "Point reads",
        "scan" => "Ordered scans",
        "ycsb-A" => "YCSB-A, update-heavy (50/50 read/update, zipfian)",
        "ycsb-B" => "YCSB-B, read-mostly (95/5 read/update, zipfian)",
        "ycsb-C" => "YCSB-C, read-only (zipfian)",
        "ycsb-D" => "YCSB-D, read-latest (95/5 read/insert)",
        "ycsb-E" => "YCSB-E, short ranges (95/5 scan/insert, zipfian)",
        "ycsb-F" => "YCSB-F, read-modify-write (50/50, zipfian)",
        _ => "",
    }
}

/// The floors a run recorded, as medians: records per second for one
/// durable framed append, bytes per second for one mapped sequential walk.
#[derive(Default, Clone, Copy)]
struct Floors {
    wal_ops_s: Option<f64>,
    scan_bytes_s: Option<f64>,
}

/// Which floor a figure is read against, in the figure's own unit, and
/// its label. The one-barrier floor bounds a durable load; the mmap floor
/// bounds a scan, converted from bytes to the suite's fixed entry size.
fn floor_for(
    workload: &str,
    quantity: &str,
    guarantee: Guarantee,
    floors: Floors,
) -> Option<(f64, &'static str)> {
    match (workload, quantity, guarantee) {
        ("load" | "load-shuffled", "ops_per_s", Guarantee::Durable) => {
            floors.wal_ops_s.map(|f| (f, "one-barrier floor"))
        }
        ("scan", "entries_per_s", _) => floors
            .scan_bytes_s
            .map(|f| (f / (KEY_SIZE + VALUE_SIZE) as f64, "mmap floor")),
        _ => None,
    }
}

struct Point {
    size: u64,
    median: f64,
    lo: f64,
    hi: f64,
}

struct Curve {
    arm: String,
    points: Vec<Point>,
}

/// (workload, quantity, unit, guarantee) -> arm -> points.
type Groups = BTreeMap<(String, String, String, Guarantee), BTreeMap<String, Vec<Point>>>;

pub struct Rendered {
    pub path: PathBuf,
    pub title: String,
}

/// Render every figure for the latest row of `scale` in each class under
/// `runs/`, into `out/<class-slug>/`. Returns what was written.
pub fn render_all(runs: &Path, out: &Path, scale: Scale) -> io::Result<Vec<Rendered>> {
    let dir = runs.join(scale.as_str());
    let mut latest: BTreeMap<String, Row> = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let r = Row::read(&p)?;
            let c = r.class();
            match latest.get(&c) {
                Some(have) if have.utc >= r.utc => {}
                _ => {
                    latest.insert(c, r);
                }
            }
        }
    }
    let mut written = Vec::new();
    for (class, row) in &latest {
        let sub = out.join(class.replace('/', "_"));
        std::fs::create_dir_all(&sub)?;
        written.extend(render_row(row, &sub)?);
    }
    if !written.is_empty() {
        write_index(out, &written)?;
    }
    Ok(written)
}

fn render_row(row: &Row, out: &Path) -> io::Result<Vec<Rendered>> {
    let mut groups: Groups = BTreeMap::new();
    let mut floors = Floors::default();
    for m in &row.measurements {
        let Some(size) = m.size else {
            if m.arm == FLOOR_ARM {
                let med = Samples::new(m.samples.clone()).median();
                match m.workload.as_str() {
                    "wal-floor" => floors.wal_ops_s = Some(med),
                    "scan-floor" => floors.scan_bytes_s = Some(med),
                    _ => {}
                }
            }
            continue;
        };
        let s = Samples::new(m.samples.clone());
        let (lo, hi) = s.median_ci(CI_CONF, CI_RESAMPLES);
        groups
            .entry((
                m.workload.clone(),
                m.quantity.clone(),
                m.unit.clone(),
                m.guarantee,
            ))
            .or_default()
            .entry(m.arm.clone())
            .or_default()
            .push(Point {
                size,
                median: s.median(),
                lo,
                hi,
            });
    }
    let mut written = Vec::new();
    for ((workload, quantity, unit, guarantee), arms) in groups {
        // Arm order: the default first so it is drawn last (on top).
        let order = crate::engines::ARMS;
        let mut curves: Vec<Curve> = order
            .iter()
            .filter_map(|a| arms.get(*a).map(|pts| (a.to_string(), pts)))
            .map(|(arm, pts)| {
                let mut points: Vec<Point> = pts
                    .iter()
                    .map(|p| Point {
                        size: p.size,
                        median: p.median,
                        lo: p.lo,
                        hi: p.hi,
                    })
                    .collect();
                points.sort_by_key(|p| p.size);
                Curve { arm, points }
            })
            .collect();
        curves.reverse();
        let g = match guarantee {
            Guarantee::Durable => "durable",
            Guarantee::Buffered => "buffered",
        };
        let name = format!("{}-{workload}-{quantity}-{g}.svg", row.scale.as_str());
        let (svg, title) = figure(row, &workload, &quantity, &unit, guarantee, &curves, floors);
        let path = out.join(name);
        std::fs::write(&path, svg)?;
        written.push(Rendered { path, title });
    }
    Ok(written)
}

fn figure(
    row: &Row,
    workload: &str,
    quantity: &str,
    unit: &str,
    guarantee: Guarantee,
    curves: &[Curve],
    floors: Floors,
) -> (String, String) {
    let up = higher_is_better(quantity).unwrap_or(true);
    let floor = floor_for(workload, quantity, guarantee, floors);
    let sizes: Vec<u64> = curves
        .iter()
        .flat_map(|c| c.points.iter().map(|p| p.size))
        .collect();
    let (smin, smax) = (
        *sizes.iter().min().unwrap_or(&10_000),
        *sizes.iter().max().unwrap_or(&10_000),
    );
    let ymax_curves = curves
        .iter()
        .flat_map(|c| c.points.iter().map(|p| p.hi))
        .fold(0.0f64, f64::max)
        .max(1e-9);
    // A floor within reach of the curves is drawn to scale; one far above
    // them would flatten every curve into the axis, so it is stated instead.
    let floor_on_scale = floor.filter(|(f, _)| *f <= ymax_curves * 3.0);
    let ymax = floor_on_scale.map_or(ymax_curves, |(f, _)| f.max(ymax_curves));
    let (ytop, yticks) = nice_axis(ymax);

    let plot_w = W - LEFT - RIGHT;
    let plot_h = H - TOP - BOTTOM;
    let (lx0, lx1) = ((smin as f64).log10() - 0.08, (smax as f64).log10() + 0.08);
    let x = |size: u64| LEFT + ((size as f64).log10() - lx0) / (lx1 - lx0) * plot_w;
    let y = |v: f64| TOP + plot_h - (v / ytop) * plot_h;

    let (title, context) = message(workload, guarantee, quantity, curves, smax, up);
    let reps = row
        .measurements
        .iter()
        .find(|m| m.workload == workload && m.quantity == quantity)
        .map(|m| m.samples.len())
        .unwrap_or(0);
    let provenance = format!(
        "{} · {} · {}-{}-{} · median with {:.0}% CI over {} reps",
        row.class(),
        &row.sha[..row.sha.len().min(7)],
        &row.utc[..4],
        &row.utc[4..6],
        &row.utc[6..8],
        CI_CONF * 100.0,
        reps,
    );

    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" font-family="{FONT}" fill="{INK}">
<rect width="{W}" height="{H}" fill="white"/>
<text x="{LEFT}" y="28" font-size="15" font-weight="600">{}</text>
<text x="{LEFT}" y="47" font-size="12">{}</text>
<text x="{LEFT}" y="64" font-size="11" fill="{MUTED}">{}</text>"#,
        esc(&title),
        esc(&context),
        esc(&provenance)
    );

    // Axes: two lines and nothing else.
    let _ = writeln!(
        s,
        r#"<line x1="{LEFT}" y1="{}" x2="{}" y2="{}" stroke="{RULE}" stroke-width="1"/>
<line x1="{LEFT}" y1="{TOP}" x2="{LEFT}" y2="{}" stroke="{RULE}" stroke-width="1"/>"#,
        TOP + plot_h,
        LEFT + plot_w,
        TOP + plot_h,
        TOP + plot_h
    );
    // x ticks: decades labelled, the 3x rungs as minor ticks.
    let mut rung = 10_000u64;
    let mut odd = false;
    while rung <= smax {
        if rung >= smin {
            let xx = x(rung);
            let major = !odd;
            let _ = writeln!(
                s,
                r#"<line x1="{xx:.1}" y1="{}" x2="{xx:.1}" y2="{}" stroke="{RULE}" stroke-width="1"/>"#,
                TOP + plot_h,
                TOP + plot_h + if major { 6.0 } else { 3.0 }
            );
            if major {
                let _ = writeln!(
                    s,
                    r#"<text x="{xx:.1}" y="{}" font-size="11" text-anchor="middle" fill="{MUTED}">{}</text>"#,
                    TOP + plot_h + 20.0,
                    pow_label(rung)
                );
            }
        }
        rung = if odd { rung * 10 / 3 } else { rung * 3 };
        odd = !odd;
    }
    let _ = writeln!(
        s,
        r#"<text x="{}" y="{}" font-size="11" text-anchor="end" fill="{MUTED}">keys</text>"#,
        LEFT + plot_w,
        TOP + plot_h + 36.0
    );
    // y ticks
    for t in &yticks {
        let yy = y(*t);
        let _ = writeln!(
            s,
            r#"<line x1="{}" y1="{yy:.1}" x2="{LEFT}" y2="{yy:.1}" stroke="{RULE}" stroke-width="1"/>
<text x="{}" y="{:.1}" font-size="11" text-anchor="end" fill="{MUTED}">{}</text>"#,
            LEFT - 5.0,
            LEFT - 9.0,
            yy + 4.0,
            si(*t)
        );
    }
    let _ = writeln!(
        s,
        r#"<text x="{}" y="{}" font-size="11" text-anchor="start" fill="{MUTED}">{}</text>"#,
        LEFT - 62.0,
        TOP - 8.0,
        esc(unit)
    );

    // The memory line: where the raw payload crosses the machine's memory.
    let mem_keys =
        (row.machine.mem_total_kb as f64 * 1024.0 / (KEY_SIZE + VALUE_SIZE) as f64) as u64;
    if mem_keys > smin && mem_keys < smax * 2 {
        let xx = x(mem_keys.min(smax));
        let _ = writeln!(
            s,
            r#"<line x1="{xx:.1}" y1="{TOP}" x2="{xx:.1}" y2="{}" stroke="{RULE}" stroke-width="1"/>
<text x="{:.1}" y="{}" font-size="11" fill="{MUTED}">memory</text>"#,
            TOP + plot_h,
            xx + 4.0,
            TOP + 12.0
        );
    }

    // The floor: a dotted rule where it fits, a note where it does not.
    if let Some((f, label)) = floor {
        if floor_on_scale.is_some() {
            let yy = y(f);
            let _ = writeln!(
                s,
                r#"<line x1="{LEFT}" y1="{yy:.1}" x2="{}" y2="{yy:.1}" stroke="{MUTED}" stroke-width="1" stroke-dasharray="2,3"/>
<text x="{}" y="{:.1}" font-size="11" text-anchor="end" fill="{MUTED}">{label}</text>"#,
                LEFT + plot_w,
                LEFT + plot_w,
                yy - 4.0
            );
        } else {
            let _ = writeln!(
                s,
                r#"<text x="{}" y="{}" font-size="11" text-anchor="end" fill="{MUTED}">{label} {}{}, off scale</text>"#,
                LEFT + plot_w,
                TOP + 12.0,
                si(f),
                esc(unit)
            );
        }
    }

    // Bands, then curves, then labels -- so ink is never under a band.
    for c in curves {
        let (col, _) = style(&c.arm);
        if c.points.len() < 2 {
            continue;
        }
        let mut d = String::new();
        for (i, p) in c.points.iter().enumerate() {
            let _ = write!(
                d,
                "{}{:.1},{:.1} ",
                if i == 0 { "M" } else { "L" },
                x(p.size),
                y(p.hi)
            );
        }
        for p in c.points.iter().rev() {
            let _ = write!(d, "L{:.1},{:.1} ", x(p.size), y(p.lo));
        }
        d.push('Z');
        let _ = writeln!(s, r#"<path d="{d}" fill="{col}" fill-opacity="0.10"/>"#);
    }
    let mut labels: Vec<(f64, String, &str)> = Vec::new();
    for c in curves {
        let (col, dash) = style(&c.arm);
        let mut d = String::new();
        for (i, p) in c.points.iter().enumerate() {
            let _ = write!(
                d,
                "{}{:.1},{:.1} ",
                if i == 0 { "M" } else { "L" },
                x(p.size),
                y(p.median)
            );
        }
        let dash_attr = if dash.is_empty() {
            String::new()
        } else {
            format!(r#" stroke-dasharray="{dash}""#)
        };
        let width = if c.arm.starts_with("supdb") { 2.0 } else { 1.6 };
        let _ = writeln!(
            s,
            r#"<path d="{d}" fill="none" stroke="{col}" stroke-width="{width}" stroke-linejoin="round" stroke-linecap="round"{dash_attr}/>"#
        );
        if let Some(last) = c.points.last() {
            labels.push((y(last.median), pretty(&c.arm).to_string(), col));
        }
    }
    // Direct labels at the right end, pushed apart when they collide and
    // kept inside the plot's height: a downward pass, then the cluster is
    // pulled back up if it ran off the bottom, then an upward pass.
    labels.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    const GAP: f64 = 13.0;
    let (ymin, ymax_) = (TOP + 4.0, TOP + plot_h - 2.0);
    let mut placed: Vec<f64> = labels.iter().map(|l| l.0.clamp(ymin, ymax_)).collect();
    for i in 1..placed.len() {
        if placed[i] - placed[i - 1] < GAP {
            placed[i] = placed[i - 1] + GAP;
        }
    }
    if let Some(last) = placed.last().copied() {
        if last > ymax_ {
            let shift = last - ymax_;
            for p in placed.iter_mut() {
                *p -= shift;
            }
        }
    }
    for i in (0..placed.len().saturating_sub(1)).rev() {
        if placed[i + 1] - placed[i] < GAP {
            placed[i] = placed[i + 1] - GAP;
        }
    }
    let lx = LEFT + plot_w + 8.0;
    for ((yy, text, col), py) in labels.iter().zip(&placed) {
        if (py - yy).abs() > 6.0 {
            let _ = writeln!(
                s,
                r#"<line x1="{:.1}" y1="{yy:.1}" x2="{:.1}" y2="{py:.1}" stroke="{col}" stroke-width="0.75"/>"#,
                lx - 6.0,
                lx - 2.0
            );
        }
        let _ = writeln!(
            s,
            r#"<text x="{lx:.1}" y="{:.1}" font-size="11" fill="{col}">{}</text>"#,
            py + 4.0,
            esc(text)
        );
    }
    s.push_str("</svg>\n");
    (s, title)
}

/// The title is the message: supdb's default against each comparator at
/// the top rung, as a factor in whichever direction the quantity is good,
/// so "ahead" always means better and the factor is never below one. The
/// second line is the context the message is read in. "Level" is within
/// five percent, which is inside this suite's noise on every machine so far.
fn message(
    workload: &str,
    guarantee: Guarantee,
    quantity: &str,
    curves: &[Curve],
    top: u64,
    up: bool,
) -> (String, String) {
    let at = |arm: &str| {
        curves
            .iter()
            .find(|c| c.arm == arm)
            .and_then(|c| c.points.iter().find(|p| p.size == top))
            .map(|p| p.median)
    };
    let mine = at("supdb").or_else(|| at("supdb-ingest"));
    let noun = workload_noun(workload);
    let what = match quantity {
        "ops_per_s" | "reads_per_s" | "entries_per_s" => "throughput",
        "p99_us" => "p99 latency",
        "device_bytes_per_byte" => "device bytes per byte",
        _ => quantity,
    };
    let g = match guarantee {
        Guarantee::Durable => "durable per batch",
        Guarantee::Buffered => "buffered",
    };
    let context = format!("{noun}, {what}, {g}, to {} keys", pow_label(top));
    let mut ahead = Vec::new();
    let mut behind = Vec::new();
    let mut level = Vec::new();
    for comp in ["lmdb", "lmdb-nosync", "rocksdb-tuned", "rocksdb-nosync"] {
        if let (Some(m), Some(c)) = (mine, at(comp)) {
            if c > 0.0 && m > 0.0 {
                let adv = if up { m / c } else { c / m };
                if (adv - 1.0).abs() < 0.05 {
                    level.push(pretty(comp).to_string());
                } else if adv >= 1.0 {
                    ahead.push(format!("{adv:.1}× ahead of {}", pretty(comp)));
                } else {
                    behind.push(format!("{:.1}× behind {}", 1.0 / adv, pretty(comp)));
                }
            }
        }
    }
    let mut parts = Vec::new();
    if !ahead.is_empty() {
        parts.push(ahead.join(" and "));
    }
    if !level.is_empty() {
        parts.push(format!("level with {}", level.join(" and ")));
    }
    if !behind.is_empty() {
        parts.push(behind.join(" and "));
    }
    let title = if parts.is_empty() {
        format!("{noun} at {} keys", pow_label(top))
    } else {
        format!("At {} keys supdb is {}", pow_label(top), parts.join(", "))
    };
    (title, context)
}

/// `10⁴`, `3×10⁴`, `10⁵` ...
fn pow_label(n: u64) -> String {
    let sup = |d: u32| -> char { "⁰¹²³⁴⁵⁶⁷⁸⁹".chars().nth(d as usize).unwrap() };
    let e = (n as f64).log10().floor() as u32;
    let m = n as f64 / 10f64.powi(e as i32);
    let exp: String = e
        .to_string()
        .chars()
        .map(|c| sup(c.to_digit(10).unwrap()))
        .collect();
    if (m - 1.0).abs() < 1e-9 {
        format!("10{exp}")
    } else {
        format!("{m:.0}×10{exp}")
    }
}

/// A round axis top and four or five round ticks.
fn nice_axis(max: f64) -> (f64, Vec<f64>) {
    let raw = max / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(mag * 10.0);
    let top = (max / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut t = 0.0;
    while t <= top + step * 0.01 {
        ticks.push(t);
        t += step;
    }
    (top, ticks)
}

fn si(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let (div, suf) = if v >= 1e9 {
        (1e9, "G")
    } else if v >= 1e6 {
        (1e6, "M")
    } else if v >= 1e3 {
        (1e3, "k")
    } else {
        (1.0, "")
    };
    let x = v / div;
    let s = if x.fract() == 0.0 {
        format!("{x:.0}")
    } else {
        format!("{x:.1}")
    };
    format!("{s}{suf}")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn write_index(out: &Path, written: &[Rendered]) -> io::Result<()> {
    let mut h = String::from(
        "<!doctype html><meta charset=utf-8><title>supdb figures</title>\
         <style>body{font-family:system-ui;margin:2rem;max-width:800px}img{display:block;margin:1.5rem 0}</style>\n",
    );
    for r in written {
        let rel = r.path.strip_prefix(out).unwrap_or(&r.path);
        let _ = writeln!(
            h,
            "<img src=\"{}\" alt=\"{}\">",
            rel.display(),
            esc(&r.title)
        );
    }
    std::fs::write(out.join("index.html"), h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_labels() {
        assert_eq!(pow_label(10_000), "10⁴");
        assert_eq!(pow_label(30_000), "3×10⁴");
        assert_eq!(pow_label(1_000_000), "10⁶");
    }

    #[test]
    fn nice_axes_end_on_a_round_number_above_the_max() {
        let (top, ticks) = nice_axis(6_500_184.0);
        assert!(top >= 6_500_184.0);
        assert!(ticks.len() >= 4 && ticks.len() <= 6, "{ticks:?}");
        assert_eq!(ticks[0], 0.0);
    }

    #[test]
    fn si_suffixes() {
        assert_eq!(si(2_000_000.0), "2M");
        assert_eq!(si(1_500.0), "1.5k");
        assert_eq!(si(0.3), "0.3");
    }
}
