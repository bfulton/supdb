//! A probe: the suite's ordered load through the `supdb` arm, then every
//! segment file of the store with its key count and its first and last key,
//! for the question of what a load leaves to promote or merge.
use std::path::Path;
use supdb::{Blob, MmapBytes};
use supdb_bench::engines::{Engine, Supdb};

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300_000);
    let dir = std::env::temp_dir().join(format!("supdb-segs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Supdb::create(Path::new(&dir)).unwrap();
    let val = [0xabu8; 100];
    let mut keys: Vec<[u8; 16]> = Vec::with_capacity(1000);
    for i in 0..n {
        let mut k = [0u8; 16];
        let s = format!("{i:016}");
        k.copy_from_slice(s.as_bytes());
        keys.push(k);
        if keys.len() == 1000 {
            let items: Vec<(&[u8], &[u8])> = keys.iter().map(|k| (&k[..], &val[..])).collect();
            e.write_batch(&items).unwrap();
            keys.clear();
        }
    }
    if !keys.is_empty() {
        let items: Vec<(&[u8], &[u8])> = keys.iter().map(|k| (&k[..], &val[..])).collect();
        e.write_batch(&items).unwrap();
    }
    e.sync().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    names.sort();
    for name in names {
        let b = Blob::open(MmapBytes::open(&dir.join(&name)).unwrap()).unwrap();
        let k = b.keys();
        let lossy = |x: Option<&[u8]>| {
            x.map(|k| String::from_utf8_lossy(k).to_string())
                .unwrap_or_default()
        };
        println!(
            "{name} keys {k} first {} last {}",
            lossy(b.key_at(0)),
            lossy(k.checked_sub(1).and_then(|r| b.key_at(r)))
        );
    }
    drop(e);
    let _ = std::fs::remove_dir_all(&dir);
}
