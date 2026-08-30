//! A logshed-shaped day index, built with the engine and measured for size.
//!
//! logshed seals one immutable object per day and wants to answer point
//! lookups against it *from a browser*. Whether that is possible at all is
//! decided by one number -- how many bytes a day's index is -- and every other
//! decision follows from it. If a day fits in a download budget then the
//! browser can hold the whole object and the reader API stays synchronous with
//! no shape change (OPFS, R2.2(a)); if it does not, the reader has to be
//! turned inside out into a plan-then-fetch API over ranged GETs (R2.2(b)).
//!
//! So the size is measured here rather than assumed, at several day sizes,
//! against a model of the workload stated explicitly below. Size is the one
//! axis this repository allows to be compared across runs -- a file length is
//! immune to the machine drift that makes timing comparisons across runs
//! worthless -- so this experiment does not need to interleave arms.
//!
//! It also builds the fixture the browser test reads, because the browser test
//! must open a real index file rather than a stub.
//!
//!   logshed build    write one day index and describe it
//!   logshed budget   sweep day sizes against the download budget
//!   logshed fixture  write the browser test's index and its expected answers
//!   logshed bundle   record the browser bundle's size against its budget

use std::path::{Path, PathBuf};
use supdb::bench::{Finding, Profile, Record, Rng, J};
use supdb::jobj;
use supdb::bytes::MmapBytes;
use supdb::{Blob, Options, Reader, Store};

// ---------------------------------------------------------------- the model --

/// The indexed fields of an HTTP access log, and how many distinct values a
/// day holds of each.
///
/// These are the shape of the workload, and they are the assumption this whole
/// document rests on, so they are written down rather than buried. A `path`
/// cardinality of 5,000 assumes logshed normalises route parameters; a service
/// that indexes raw paths with ids in them has an unbounded key count and a
/// different problem. The `ref` and `ua` counts are the long-tailed ones and
/// are deliberately generous.
///
/// The count that actually decides the budget is not any of these. It is the
/// line count: a posting is written per line per field, so the postings scale
/// with the day's traffic and the keys do not.
const FIELDS: &[(&str, usize)] = &[
    ("method", 8),
    ("status", 24),
    ("host", 64),
    ("country", 210),
    ("ua", 500),
    ("ref", 2000),
    ("path", 5000),
];

/// A posting is a line ordinal within the day: four bytes, little-endian.
///
/// Four bytes rather than a varint because logshed wants to seek to the line,
/// and because the store already length-prefixes each value -- a varint
/// posting would save at most two bytes against a one-byte prefix that is
/// paid either way.
const POSTING_BYTES: usize = 4;

fn term(field: &str, i: usize, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(field.as_bytes());
    out.push(b'=');
    // Zero-padded so the dictionary is ordered the way a scan wants it.
    let mut buf = [0u8; 8];
    let mut v = i;
    for slot in buf.iter_mut().rev() {
        *slot = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out.extend_from_slice(&buf);
}

/// Which value of a field a given line carries.
///
/// Zipf-ish rather than uniform, because a real access log is: `status=200`
/// takes most of the traffic and the tail of `ref` is nearly empty. That
/// matters for the *shape* of the extents -- a uniform assignment gives every
/// key the same number of postings and hides what a skewed one costs -- and it
/// does not change the total, which is one posting per line per field.
fn zipf(rng: &mut Rng, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    // u^2 concentrates mass at the head without needing a table.
    let u = rng.unit();
    let i = (u * u * n as f64) as usize;
    i.min(n - 1)
}

/// How the daily roll feeds the store.
///
/// This is open question 2 from the requirements, and it decides more than the
/// question asks. `Line` is the naive shape -- stream the day and append each
/// line's terms as they arrive. `Term` is what an index builder does: group
/// the postings by term first, then append each term's run in one go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Line,
    Term,
}

impl Order {
    fn as_str(&self) -> &'static str {
        match self {
            Order::Line => "line",
            Order::Term => "term",
        }
    }
    fn parse(s: &str) -> Option<Order> {
        match s {
            "line" => Some(Order::Line),
            "term" => Some(Order::Term),
            _ => None,
        }
    }
}

struct Built {
    keys: u64,
    postings: u64,
    file_bytes: u64,
    index_bytes: u64,
    blocks: u64,
    bytes_written: u64,
    merges: u64,
    free_bytes: u64,
    payload_bytes: u64,
}

/// Build one day's index and report what it cost.
fn build_day(path: &Path, lines: u64, seed: u64, order: Order) -> std::io::Result<Built> {
    let _ = std::fs::remove_file(path);
    let opts = Options {
        // The daily roll is one pass with a single checkpoint at the end, so
        // the buffer wants to be large and the sync policy does not matter
        // until close.
        buffer_bytes: 256 << 20,
        ..Default::default()
    };
    let store = Store::create(path, opts)?;
    let mut rng = Rng::new(seed);
    let mut key = Vec::with_capacity(32);
    let mut postings = 0u64;
    match order {
        Order::Line => {
            for line in 0..lines {
                let ord = (line as u32).to_le_bytes();
                for (field, card) in FIELDS {
                    let i = zipf(&mut rng, *card);
                    term(field, i, &mut key);
                    store.append(&key, &ord[..POSTING_BYTES])?;
                    postings += 1;
                }
            }
        }
        Order::Term => {
            // (field, value, line) packed into one word so a single sort puts
            // every term's postings together, and in line order within a term.
            let mut pairs: Vec<u64> = Vec::with_capacity((lines as usize) * FIELDS.len());
            for line in 0..lines {
                for (f, (_, card)) in FIELDS.iter().enumerate() {
                    let i = zipf(&mut rng, *card);
                    pairs.push(((f as u64) << 56) | ((i as u64) << 32) | line);
                }
            }
            pairs.sort_unstable();
            let mut cur = u64::MAX;
            for p in &pairs {
                let head = p >> 32;
                if head != cur {
                    cur = head;
                    term(
                        FIELDS[(head >> 24) as usize].0,
                        (head & 0xff_ffff) as usize,
                        &mut key,
                    );
                }
                let ord = (*p as u32).to_le_bytes();
                store.append(&key, &ord[..POSTING_BYTES])?;
                postings += 1;
            }
        }
    }
    store.checkpoint()?;
    let stats = store.close()?;
    let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let r = Reader::open(path)?;
    Ok(Built {
        keys: stats.keys,
        postings,
        file_bytes,
        index_bytes: r.index_bytes() as u64,
        blocks: stats.blocks,
        bytes_written: stats.bytes_written,
        merges: stats.merges,
        free_bytes: stats.free_bytes,
        payload_bytes: postings * (POSTING_BYTES as u64 + 1),
    })
}

// ---------------------------------------------------------------- the budget --

/// What a browser will download once and keep.
///
/// This is a product decision with a number behind it rather than a measured
/// property of the engine, so it is stated here and the experiment checks the
/// engine against it -- not the other way around.
///
/// 32 MB, because:
///
///   * it is about ten seconds on a 25 Mbit/s connection, which is the outer
///     edge of a tolerable first-query wait, and it is paid once per day-index
///     and then cached in OPFS rather than per query;
///   * it is a size a phone can hold and a size OPFS grants without a quota
///     prompt, where a few hundred megabytes is neither;
///   * logshed's whole current client is 32 KB, so this is already three
///     orders of magnitude more than the application it serves, and picking a
///     larger number would mean the index, not the app, is the product.
///
/// Above it, the answer is not "download it anyway". It is to shard the day --
/// logshed already writes one immutable object per sealed period, so a busy
/// day becomes 24 hourly objects, each independently under budget and each
/// individually skippable by a query with a time range.
const BUDGET_BYTES: u64 = 32 << 20;

fn budget(profile: Profile) -> std::io::Result<Record> {
    let mut rec = Record::new("w1-daysize", profile);
    // Three scales at every profile, because two points cannot show whether
    // the marginal cost of a line is stable and one point cannot show anything.
    let scales: Vec<u64> = profile.pick(
        vec![5_000, 20_000, 50_000],
        vec![20_000, 100_000, 400_000],
        vec![50_000, 250_000, 1_000_000],
    );
    let dir = std::env::temp_dir().join("supdb-logshed");
    std::fs::create_dir_all(&dir)?;

    rec.param("budget_bytes", J::u(BUDGET_BYTES));
    rec.param("posting_bytes", J::u(POSTING_BYTES as u64));
    rec.param("fields", J::u(FIELDS.len() as u64));
    rec.param(
        "field_cardinality",
        J::O(FIELDS
            .iter()
            .map(|(f, c)| ((*f).to_string(), J::u(*c as u64)))
            .collect()),
    );

    let row = |lines: u64, b: &Built| -> J {
        jobj! {
            "lines" => J::u(lines),
            "keys" => J::u(b.keys),
            "postings" => J::u(b.postings),
            "file_bytes" => J::u(b.file_bytes),
            "index_bytes" => J::u(b.index_bytes),
            "index_bytes_per_key" => J::fp(b.index_bytes as f64 / b.keys.max(1) as f64, 2),
            "file_bytes_per_line" => J::fp(b.file_bytes as f64 / lines.max(1) as f64, 3),
            "file_bytes_per_posting" => J::fp(b.file_bytes as f64 / b.postings.max(1) as f64, 3),
            "payload_bytes" => J::u(b.payload_bytes),
            "overhead_over_payload" => J::fp(b.file_bytes as f64 / b.payload_bytes.max(1) as f64, 3),
            "blocks" => J::u(b.blocks),
            "bytes_written" => J::u(b.bytes_written),
            "merges" => J::u(b.merges),
            "free_bytes" => J::u(b.free_bytes),
            "within_budget" => J::Bool(b.file_bytes <= BUDGET_BYTES),
        }
    };

    let mut term_rows = Vec::new();
    let mut line_rows = Vec::new();
    let mut term_bytes: Vec<u64> = Vec::new();
    let mut line_bytes: Vec<u64> = Vec::new();
    let mut worst_merges = 0u64;
    for lines in &scales {
        // Not interleaved, and it does not need to be: a file length does not
        // drift with the machine, which is the one exemption CLAUDE.md grants
        // from measuring two arms in a single process.
        let path = dir.join(format!("day-{lines}.supdb"));
        let t = build_day(&path, *lines, 0x5109_5ed0 ^ lines, Order::Term)?;
        term_rows.push(row(*lines, &t));
        term_bytes.push(t.file_bytes);
        worst_merges = worst_merges.max(t.merges);
        let l = build_day(&path, *lines, 0x5109_5ed0 ^ lines, Order::Line)?;
        line_rows.push(row(*lines, &l));
        line_bytes.push(l.file_bytes);
        let _ = std::fs::remove_file(&path);
    }
    rec.series("term_order", J::arr(term_rows));
    rec.series("line_order", J::arr(line_rows));

    // Measured, not fitted. `ext-sweep` learned this the expensive way: a
    // straight line through a scan sweep put its intercept above the measured
    // one-entry cost, and both coefficients were wrong in the same direction.
    // So the marginal byte cost of a line is a difference quotient between
    // adjacent measured points, and the fixed cost is what is left of the
    // largest measured point once the marginal is taken out of it.
    let marginal = |i: usize, j: usize| -> f64 {
        let (dy, dx) = (
            term_bytes[j] as f64 - term_bytes[i] as f64,
            scales[j] as f64 - scales[i] as f64,
        );
        if dx > 0.0 {
            dy / dx
        } else {
            0.0
        }
    };
    let last = scales.len() - 1;
    let top = marginal(last - 1, last);
    let bottom = marginal(0, 1);
    let fixed = (term_bytes[last] as f64 - top * scales[last] as f64).max(0.0);
    let lines_at_budget = if top > 0.0 {
        ((BUDGET_BYTES as f64 - fixed) / top).max(0.0) as u64
    } else {
        0
    };
    let shards_for_10m = if lines_at_budget > 0 {
        (10_000_000f64 / lines_at_budget as f64).ceil() as u64
    } else {
        0
    };
    rec.series(
        "budget",
        jobj! {
            "budget_bytes" => J::u(BUDGET_BYTES),
            "marginal_bytes_per_line_top" => J::fp(top, 3),
            "marginal_bytes_per_line_bottom" => J::fp(bottom, 3),
            "fixed_bytes" => J::fp(fixed, 0),
            "lines_at_budget" => J::u(lines_at_budget),
            "shards_for_a_10m_line_day" => J::u(shards_for_10m),
        },
    );

    // W1.1 -- can a day's size be predicted from its line count at all? If the
    // marginal cost of a line moves with the size of the day, then the
    // extrapolation under W1.2 is not arithmetic, it is a guess.
    let drift = if bottom > 0.0 {
        (top - bottom).abs() / bottom
    } else {
        1.0
    };
    rec.finding(Finding::new(
        "W1.1",
        "the marginal cost of a log line does not grow with the size of the day, so a day index's size can be predicted from its line count",
        drift <= 0.20,
        format!(
            "{bottom:.2} B/line between {} and {} lines against {top:.2} B/line between {} and {} \
             ({:.1}% apart), over a fixed cost of {fixed:.0} bytes. The postings dominate and \
             there is one per line per indexed field; the key count is bounded by the field \
             cardinalities, so it lands in the fixed term rather than the marginal one",
            scales[0], scales[1], scales[last - 1], scales[last], drift * 100.0
        ),
    ));

    // W1.2 -- the decision R2.2 turns on, stated as a line count so that it can
    // be checked against a real day rather than argued about. Half a million,
    // deliberately below the measured ceiling: a threshold set at the measured
    // value tests the arithmetic rather than the engine.
    rec.finding(Finding::new(
        "W1.2",
        "a day of 500,000 log lines at seven indexed fields fits in a 32 MB browser download budget, so a browser can hold a whole day and the reader API needs no asynchronous shape change",
        lines_at_budget >= 500_000,
        format!(
            "{top:.2} B/line over {fixed:.0} fixed puts the 32 MB budget at {lines_at_budget} \
             lines/day. A busier day is sharded rather than downloaded: at this rate a 10M-line \
             day is {shards_for_10m} objects, each independently under budget and each skippable \
             by a query with a time range. This is what makes R2.2(a) -- an OPFS synchronous \
             access handle over one downloaded object -- viable, and it is why the reader in \
             `blob.rs` stays synchronous"
        ),
    ));

    // W1.3 -- open question 2, and the largest single number in this file.
    let ratio = if term_bytes[last] > 0 {
        line_bytes[last] as f64 / term_bytes[last] as f64
    } else {
        0.0
    };
    rec.finding(Finding::new(
        "W1.3",
        "grouping a day's postings by term before appending them costs less than a third of the file that appending them in log-line order does",
        ratio >= 3.0,
        format!(
            "at {} lines, line order writes {} bytes and term order writes {} ({ratio:.2}x). Line \
             order interleaves every key, so a hot key's run is sealed into many small extents and \
             merge_threshold then rewrites the whole run each time it crosses four -- the \
             inline-merge cost F5.1 records as a latency tail, showing up here as dead space. Term \
             order seals each key's run once and merges {worst_merges} times in the whole sweep",
            scales[last], line_bytes[last], term_bytes[last]
        ),
    ));

    Ok(rec)
}

// ---------------------------------------------------------------- fixture --

/// Write the day index the browser test opens, and the answers it must give.
///
/// The answers come from the *native* reader over the same file. So the
/// browser test is a differential test across the wasm boundary and an OPFS
/// handle, against a chain that `tests/blob.rs` already pins to
/// `store::Reader`. A browser test whose expectations were hand-written would
/// only ever confirm what its author already believed.
fn fixture(dir: &Path, lines: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("day.supdb");
    let built = build_day(&path, lines, 0x5109_5ed0, Order::Term)?;
    let blob = Blob::open(MmapBytes::open(&path)?)?;

    // Keys spread across the dictionary and across run lengths: the head of a
    // Zipf field is thousands of postings, the tail is one.
    let mut probes: Vec<Vec<u8>> = Vec::new();
    for rank in [0usize, 1, 7, 64, 512, 2048] {
        if let Some(k) = blob.key_at(rank) {
            probes.push(k.to_vec());
        }
    }
    if let Some(k) = blob.key_at(blob.keys().saturating_sub(1)) {
        probes.push(k.to_vec());
    }

    let mut lookups = Vec::new();
    let mut counts = Vec::new();
    for k in &probes {
        let mut vals = Vec::new();
        blob.read_all(k, |v| {
            vals.push(J::arr(v.iter().map(|b| J::u(*b as u64)).collect()))
        })?;
        // Only small runs are compared value-by-value; a 40,000-posting key
        // would put a megabyte of JSON in the fixture and prove nothing the
        // count does not.
        if vals.len() <= 64 {
            lookups.push(jobj! {
                "key" => J::s(String::from_utf8_lossy(k).into_owned()),
                "values" => J::arr(vals),
            });
        }
        counts.push(jobj! {
            "key" => J::s(String::from_utf8_lossy(k).into_owned()),
            "count" => J::u(blob.count(k)?),
            "stored_bytes" => J::u(blob.stored_bytes(k)),
        });
    }

    let from = FIELDS[3].0;
    let mut rows = Vec::new();
    blob.scan_counts(from.as_bytes(), 12, |k, n| {
        rows.push(jobj! {
            "key" => J::s(String::from_utf8_lossy(k).into_owned()),
            "count" => J::u(n),
        });
        true
    })?;

    let doc = jobj! {
        "lines" => J::u(lines),
        "file_bytes" => J::u(built.file_bytes),
        "keys" => J::u(blob.keys() as u64),
        "index_bytes" => J::u(blob.index_bytes() as u64),
        "posting_bytes" => J::u(POSTING_BYTES as u64),
        "lookups" => J::arr(lookups),
        "counts" => J::arr(counts),
        "scan" => jobj! {
            "from" => J::s(from),
            "limit" => J::u(12),
            "rows" => J::arr(rows),
        },
    };
    std::fs::write(dir.join("expected.json"), doc.render())?;
    eprintln!(
        "# wrote {} ({} bytes, {} keys) and expected.json",
        path.display(),
        built.file_bytes,
        blob.keys()
    );
    Ok(())
}

// ---------------------------------------------------------------- bundle --

/// Record what the browser actually downloads, against the budget.
///
/// The sizes are measured by `web/build.sh` -- gzipping a file is not
/// something this binary should grow a dependency to do -- and passed in, so
/// that the record still carries the machine and goes through the same
/// `Record` machinery as everything else in `results/`.
///
/// The *floor* is what makes this legible. A wasm `cdylib` in Rust is not
/// small before any of your code is in it, and a blob measured alone cannot
/// say whether it is large because supdb is large or because the language is.
/// `web/floor/` is the control: the same profile, the same standard-library
/// surface, none of supdb.
#[allow(clippy::too_many_arguments)]
fn bundle(profile: Profile, wasm: u64, wasm_gz: u64, floor: u64, floor_gz: u64) -> Record {
    // R3.3. logshed's whole current client is 32 KB raw and 12 KB gzipped,
    // and that is the calibration the requirement asks for rather than the
    // budget. 64 KB gzipped, because:
    //
    //   * it is one round trip on any connection, and it is immutable and
    //     cached, so it is paid once per deploy rather than once per query;
    //   * it is 0.2% of the 32 MB index budget it exists to read, so a
    //     library that had to be twice this size to halve a download would
    //     still be worth it;
    //   * it is five times logshed's client, and the first 12 KB of it is the
    //     Rust standard library's floor, which no amount of work on this side
    //     removes. Budgeting under that would be budgeting against Rust.
    const BUDGET_GZ: u64 = 64 << 10;
    // What supdb itself may add above the floor.
    const MARGINAL_BUDGET_GZ: u64 = 32 << 10;

    let mut rec = Record::new("w3-bundle", profile);
    let marginal = wasm.saturating_sub(floor);
    let marginal_gz = wasm_gz.saturating_sub(floor_gz);
    rec.param("budget_gzip_bytes", J::u(BUDGET_GZ));
    rec.param("marginal_budget_gzip_bytes", J::u(MARGINAL_BUDGET_GZ));
    rec.param("logshed_client_gzip_bytes", J::u(12 << 10));
    rec.series(
        "sizes",
        jobj! {
            "wasm_bytes" => J::u(wasm),
            "wasm_gzip_bytes" => J::u(wasm_gz),
            "floor_bytes" => J::u(floor),
            "floor_gzip_bytes" => J::u(floor_gz),
            "supdb_marginal_bytes" => J::u(marginal),
            "supdb_marginal_gzip_bytes" => J::u(marginal_gz),
            "floor_share_of_gzip" => J::fp(floor_gz as f64 / wasm_gz.max(1) as f64, 3),
        },
    );
    rec.finding(Finding::new(
        "W3.1",
        "the browser reader is under a 64 KB gzipped budget",
        wasm_gz <= BUDGET_GZ,
        format!(
            "{wasm_gz} bytes gzipped ({wasm} raw) against a budget of {BUDGET_GZ}. Hand-written \
             C ABI rather than a binding generator, opt-level z, fat LTO, panic=abort, stripped"
        ),
    ));
    rec.finding(Finding::new(
        "W3.2",
        "most of the module is supdb rather than the Rust runtime it is built on",
        marginal_gz > floor_gz,
        format!(
            "an empty cdylib with the same standard-library surface is {floor_gz} bytes gzipped \
             ({floor} raw), so supdb's marginal cost is {marginal_gz} gzipped ({marginal} raw) and \
             the floor is {:.0}% of what ships. The floor is not reducible from this side: it is \
             the allocator, the panic machinery and `core::fmt`, which `std::io::Error` pulls in \
             whatever it is reporting",
            floor_gz as f64 / wasm_gz.max(1) as f64 * 100.0
        ),
    ));
    rec.finding(Finding::new(
        "W3.3",
        "supdb's own contribution to the bundle is under 32 KB gzipped",
        marginal_gz <= MARGINAL_BUDGET_GZ,
        format!(
            "{marginal_gz} bytes gzipped above the floor, against {MARGINAL_BUDGET_GZ}. This is \
             the number that moves when the reader grows; W3.1 is the one the user pays"
        ),
    ));
    rec
}

// ---------------------------------------------------------------- main --

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let arg = |n: &str| -> Option<String> {
        argv.iter()
            .position(|a| a == n)
            .and_then(|i| argv.get(i + 1))
            .cloned()
    };
    let cmd = argv.get(1).map(|s| s.as_str()).unwrap_or("help");
    match cmd {
        "build" => {
            let path = PathBuf::from(arg("--path").unwrap_or_else(|| "day.supdb".into()));
            let lines: u64 = arg("--lines")
                .and_then(|v| v.parse().ok())
                .unwrap_or(20_000);
            let order =
                Order::parse(arg("--order").as_deref().unwrap_or("term")).unwrap_or(Order::Term);
            let b = build_day(&path, lines, 0x5109_5ed0, order)?;
            println!(
                "{}",
                jobj! {
                    "path" => J::s(path.display().to_string()),
                    "order" => J::s(order.as_str()),
                    "blocks" => J::u(b.blocks),
                    "merges" => J::u(b.merges),
                    "free_bytes" => J::u(b.free_bytes),
                    "payload_bytes" => J::u(b.payload_bytes),
                    "bytes_written" => J::u(b.bytes_written),
                    "lines" => J::u(lines),
                    "keys" => J::u(b.keys),
                    "postings" => J::u(b.postings),
                    "file_bytes" => J::u(b.file_bytes),
                    "index_bytes" => J::u(b.index_bytes),
                    "file_bytes_per_line" => J::fp(b.file_bytes as f64 / lines.max(1) as f64, 3),
                }
                .render()
            );
            Ok(())
        }
        "fixture" => {
            let dir = PathBuf::from(arg("--dir").unwrap_or_else(|| "web/test/out".into()));
            let lines: u64 = arg("--lines").and_then(|v| v.parse().ok()).unwrap_or(20_000);
            fixture(&dir, lines)
        }
        "bundle" => {
            let profile =
                Profile::parse(arg("--profile").as_deref().unwrap_or("ci")).unwrap_or(Profile::Ci);
            let out = PathBuf::from(arg("--out").unwrap_or_else(|| "results".into()));
            let n = |k: &str| -> u64 { arg(k).and_then(|v| v.parse().ok()).unwrap_or(0) };
            let rec = bundle(
                profile,
                n("--wasm-bytes"),
                n("--wasm-gzip"),
                n("--floor-bytes"),
                n("--floor-gzip"),
            );
            rec.print_summary();
            rec.write(&out)?;
            if rec.all_findings_hold() {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        "budget" => {
            let profile = Profile::parse(arg("--profile").as_deref().unwrap_or("ci"))
                .unwrap_or(Profile::Ci);
            let out = PathBuf::from(arg("--out").unwrap_or_else(|| "results".into()));
            let rec = budget(profile)?;
            rec.print_summary();
            rec.write(&out)?;
            if rec.all_findings_hold() {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        _ => {
            eprintln!(
                "logshed build   --path P --lines N [--order term|line]\n\
                 logshed budget  --profile ci|dev|full [--out results]\n\
                 logshed fixture --dir web/test/out [--lines N]\n\
                 logshed bundle  --profile P --wasm-bytes N --wasm-gzip N \
                 --floor-bytes N --floor-gzip N"
            );
            std::process::exit(2)
        }
    }
}
