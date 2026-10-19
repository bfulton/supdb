//! Open every segment of a store directory and report, per segment, its key
//! count, whether its rank order is sorted (the first inversion if not), and
//! its first and last keys beside the fence its name carries.
use supdb::{Blob, BlobOptions, MmapBytes};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn show(k: &[u8]) -> String {
    if k.iter().all(|c| c.is_ascii_graphic() || *c == b' ') && !k.is_empty() {
        format!("{:?}", String::from_utf8_lossy(k))
    } else {
        hex(k)
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("store dir");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    names.sort();
    for n in names {
        let p = std::path::Path::new(&dir).join(&n);
        let b = match Blob::open_with(MmapBytes::open(&p).unwrap(), BlobOptions::default()) {
            Ok(b) => b,
            Err(e) => {
                println!("{n}: open failed: {e}");
                continue;
            }
        };
        let k = b.keys();
        let mut inv = 0usize;
        let mut first: Option<(usize, String, String)> = None;
        for i in 1..k {
            let a = b.key_at(i - 1).unwrap();
            let c = b.key_at(i).unwrap();
            if a >= c {
                inv += 1;
                if first.is_none() {
                    first = Some((i, show(a), show(c)));
                }
            }
        }
        let lo = b.key_at(0).map(show).unwrap_or_default();
        let hi = if k > 0 { b.key_at(k - 1).map(show).unwrap_or_default() } else { String::new() };
        println!("{n}\n    keys {k} inversions {inv} first {first:?}\n    first key {lo}\n    last key  {hi}");
    }
}
