//! A cold group decode with its lines asked for ahead, priced on the form's
//! own pages.
//!
//! Profiled on the combined arm at five million keys under 640 MiB, the E
//! mix's group decodes ran at a microsecond each, the figure `zsize` gives a
//! decode whose page's lines were flushed, against 0.27 µs with them warm:
//! a scan's groups are memo misses on pages the walk has not touched, and
//! the decoder meets their lines one miss at a time. This asks what a
//! software prefetch of those lines buys when it is issued where the engine
//! first knows the page or the group -- before the page's view is taken
//! (the fence named the page), or after it (the directory named the group's
//! payload) -- with the view's own work, or a spin standing in for the memo
//! lookup, between the hint and the decode.
//!
//! Builds a store under the forced form in the suite's shape as `zdecode`
//! does, reads every form file whole, and for each shape decodes one group
//! of every page in a random page order (a point read's or a scan start's
//! decode, the page cold) or every group of the page in turn after one view
//! (a scan's walk across the page), cycling the pages so the input is as
//! cold as a form file larger than the cache makes it; the shapes alternate
//! their order round by round and the median round is printed as ns a
//! decode. `--cold-out` rotates the decode's output over two thousand
//! buffers, as the memo's recycled buffers are cold; `--spin N` puts N
//! dependent multiplies between the hint and the decode.
//!
//!     cargo run --release --example zlines -- [--size N] [--rounds N] [--spin N] [--cold-out] [--scan]
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;
use supdb::db::zpages::{decode_group, view_trusted, GroupBuf, PageView, Trailer, ZPAGE, ZTRAILER};
use supdb_bench::engines::{self, Batch};
use supdb_bench::workload::{db_key_into, Payload, Rng};

/// Every page of every form file under `dir`, each as its bytes, in file
/// order.
fn form_pages(dir: &Path) -> Vec<Vec<u8>> {
    let mut files: Vec<_> = walk(dir)
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("zpg-") && n.ends_with(".zpg"))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no form file under {}", dir.display());
    let mut pages = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).expect("a form file reads");
        let t = Trailer::decode(&bytes[bytes.len() - ZTRAILER..]).expect("a trailer");
        for i in 0..t.n_pages as usize {
            pages.push(bytes[i * ZPAGE..(i + 1) * ZPAGE].to_vec());
        }
    }
    pages
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).expect("the store's directory reads") {
        let p = e.expect("an entry").path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[inline(always)]
fn prefetch(ptr: *const u8, bytes: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
        let mut off = 0usize;
        while off < bytes {
            // SAFETY: a prefetch is a hint that faults on no address.
            unsafe { _mm_prefetch(ptr.add(off) as *const i8, _MM_HINT_T0) };
            off += 64;
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (ptr, bytes);
}

#[inline(always)]
fn spin(n: usize) -> u64 {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    for _ in 0..n {
        x = black_box(x)
            .wrapping_mul(0xD6E8_FEB8_6659_FD93)
            .wrapping_add(1);
    }
    x
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The view, then the decode: the engine as it stands.
    Plain,
    /// The view, the group's payload lines asked for, the spin, the decode:
    /// the hint where the directory names the group.
    Payload,
    /// The page's first line asked for, the view, the payload, the spin, the
    /// decode.
    HdrPayload,
    /// The page's first half asked for, the view, the spin, the decode:
    /// the hint where the fence names the page.
    Half,
    /// The whole page asked for, then the view, the spin, the decode.
    Page,
    /// The view, the output buffer's lines asked for, the spin, the decode:
    /// the hint for the decoder's stores into a recycled buffer.
    Out,
    /// The view, the payload's and the output's lines asked for, the spin,
    /// the decode.
    PayloadOut,
}

const SHAPES: [Shape; 7] = [
    Shape::Plain,
    Shape::Payload,
    Shape::HdrPayload,
    Shape::Half,
    Shape::Page,
    Shape::Out,
    Shape::PayloadOut,
];

fn name(s: Shape) -> &'static str {
    match s {
        Shape::Plain => "plain",
        Shape::Payload => "payload",
        Shape::HdrPayload => "hdr+payload",
        Shape::Half => "half-page",
        Shape::Page => "page",
        Shape::Out => "out",
        Shape::PayloadOut => "payload+out",
    }
}

/// One page's decode under `shape`: group `j`, or every group when `all`.
#[inline(never)]
fn decode_page(
    shape: Shape,
    page: &[u8],
    j: usize,
    all: bool,
    spin_n: usize,
    buf: &mut GroupBuf,
) -> u64 {
    match shape {
        Shape::Half => prefetch(page.as_ptr(), ZPAGE / 2),
        Shape::Page => prefetch(page.as_ptr(), ZPAGE),
        Shape::HdrPayload => prefetch(page.as_ptr(), 64),
        _ => {}
    }
    let view: PageView<'_> = view_trusted(page).expect("a page we built");
    let groups = view.groups();
    let (from, to) = if all {
        (0, groups)
    } else {
        (j % groups, j % groups + 1)
    };
    let mut sum = 0u64;
    for g in from..to {
        if matches!(
            shape,
            Shape::Payload | Shape::HdrPayload | Shape::PayloadOut
        ) {
            let p = view.payload(g).expect("a payload");
            prefetch(p.as_ptr(), p.len());
        }
        if matches!(shape, Shape::Out | Shape::PayloadOut) {
            let ulen = view.group(g).expect("a group").ulen as usize;
            prefetch(buf.bytes(0).as_ptr(), ulen.min(buf.capacity_bytes()));
        }
        if spin_n > 0 {
            sum = sum.wrapping_add(spin(spin_n));
        }
        let out = decode_group(&view, g, buf).expect("a group we built decodes");
        sum = sum
            .wrapping_add(u64::from(out[out.len() / 2]))
            .wrapping_add(out.len() as u64);
    }
    sum
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let mut size: u64 = 1_000_000;
    let mut rounds: usize = 11;
    let mut spin_n: usize = 0;
    let mut cold_out = false;
    let mut all = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |what: &str| args.next().unwrap_or_else(|| panic!("{a} needs {what}"));
        match a.as_str() {
            "--size" => size = val("a count").parse().expect("a count"),
            "--rounds" => rounds = val("a count").parse().expect("a count"),
            "--spin" => spin_n = val("a count").parse().expect("a count"),
            "--cold-out" => cold_out = true,
            "--scan" => all = true,
            other => panic!("unknown argument {other}"),
        }
    }
    let root = std::env::temp_dir().join(format!("supdb-zlines-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    {
        let payload = Payload::new(100, 0.5, 0xE1);
        let mut e = engines::open("supdb-nocachezpages", &root, 1).unwrap();
        let mut vrng = Rng::new(0xE1);
        let mut buf = Batch::with_capacity(1000, payload.value_size());
        let mut kb = [0u8; 16];
        let t = Instant::now();
        for i in 0..size {
            db_key_into(i, &mut kb);
            buf.push(&kb, payload.get(&mut vrng));
            if buf.len() == 1000 {
                buf.flush(e.as_mut()).unwrap();
            }
        }
        buf.flush(e.as_mut()).unwrap();
        e.sync().unwrap();
        println!(
            "{size} keys loaded under {} in {:.2} s",
            e.name(),
            t.elapsed().as_secs_f64()
        );
    }
    let pages = form_pages(&root);
    let _ = std::fs::remove_dir_all(&root);
    let groups: usize = pages
        .iter()
        .map(|p| view_trusted(p).expect("a page").groups())
        .sum();
    println!(
        "{} pages ({:.1} MB), {groups} groups; one group a page in a random page order{}, {} output buffer{}, spin {spin_n}, {rounds} rounds",
        pages.len(),
        (pages.len() * ZPAGE) as f64 / 1e6,
        if all { " then every group of the page" } else { "" },
        if cold_out { "2048 rotating" } else { "one" },
        if cold_out { "s" } else { "" },
    );
    // A fixed random order of the pages, the same for every shape and round.
    let mut order: Vec<usize> = (0..pages.len()).collect();
    let mut rng = Rng::new(0x5EED);
    for i in (1..order.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let nbufs = if cold_out { 2048 } else { 1 };
    let mut bufs: Vec<GroupBuf> = (0..nbufs).map(|_| GroupBuf::new()).collect();
    // Every buffer sized once, as the memo's recycled buffers are, so the
    // output hint has lines to name.
    {
        let mut tmp = GroupBuf::new();
        for b in bufs.iter_mut() {
            std::mem::swap(b, &mut tmp);
            let page = &pages[0];
            let view = view_trusted(page).expect("a page");
            let _ = decode_group(&view, 0, &mut tmp);
            std::mem::swap(b, &mut tmp);
        }
    }
    let per_round = if all { groups } else { pages.len() };
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); SHAPES.len()];
    for r in 0..rounds {
        // The shapes' order rotates round by round so a drift in the host
        // falls on each.
        for k in 0..SHAPES.len() {
            let si = (k + r) % SHAPES.len();
            let shape = SHAPES[si];
            let t = Instant::now();
            let mut sum = 0u64;
            for (n, &pi) in order.iter().enumerate() {
                let buf = &mut bufs[n % nbufs];
                sum = sum.wrapping_add(decode_page(shape, &pages[pi], pi, all, spin_n, buf));
            }
            let d = t.elapsed().as_nanos() as f64;
            black_box(sum);
            times[si].push(d / per_round as f64);
        }
    }
    println!(
        "{:>12} {:>10} {:>18}",
        "shape", "ns/decode", "rounds (min-max)"
    );
    for (si, shape) in SHAPES.iter().enumerate() {
        let v = &mut times[si];
        let (mn, mx) = (
            v.iter().cloned().fold(f64::MAX, f64::min),
            v.iter().cloned().fold(0.0, f64::max),
        );
        println!(
            "{:>12} {:>10.0} {:>8.0}-{:<8.0}",
            name(*shape),
            median(v),
            mn,
            mx
        );
    }
}
