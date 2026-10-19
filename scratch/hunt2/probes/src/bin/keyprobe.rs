//! keyprobe DIR KEYHEX: reopen the store with the fuzz seed 36's options,
//! scan it and point-read the key, then ask every segment file in the
//! directory what it holds for the key, tombstones marked.
use supdb::{Blob, BlobOptions, Db, MmapBytes, Options, SyncPolicy, Upkeep};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let key = unhex(&std::env::args().nth(2).expect("key hex"));
    let dir = std::path::PathBuf::from(dir);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".sup"))
        .collect();
    names.sort();
    println!("== segment files holding the key ==");
    for n in &names {
        let b = match Blob::open_with(MmapBytes::open(&dir.join(n)).unwrap(), BlobOptions::default()) {
            Ok(b) => b,
            Err(e) => {
                println!("{n}: open failed: {e}");
                continue;
            }
        };
        if let Some((exts, tail)) = b.lookup_full(&key) {
            let mut lens = Vec::new();
            let _ = b.read_exts(&exts, tail, |v| lens.push(v.len()));
            let tombs: Vec<bool> = exts.iter().map(|e| e.is_tombstone()).collect();
            println!("{n}: keys {} | exts tombstone flags {:?} | value lens {:?}", b.keys(), tombs, lens);
        }
    }
    let opts = Options {
        scan_block_cache: false,
        seal_bytes: 2048,
        partition_bytes: Some(4096),
        l0_trigger: 4,
        upkeep: Upkeep::Background(1),
        publish_in_background: true,
        sync: SyncPolicy::Always,
        compact: true,
        promote: true,
        ..Options::default()
    };
    println!("== the store reopened ==");
    let db = Db::open(&dir, opts).expect("open");
    let mut lens = Vec::new();
    let n = db.read_all(&key, |v| lens.push(v.len())).expect("read_all");
    println!("read_all: {n} values, lens {lens:?}; count {}", db.count(&key).expect("count"));
    let mut got: Vec<usize> = Vec::new();
    let mut keys_seen = 0usize;
    let mut last: Option<Vec<u8>> = None;
    db.scan(b"", usize::MAX, |k, v| {
        if last.as_deref() != Some(k) {
            keys_seen += 1;
            last = Some(k.to_vec());
        }
        if k == key.as_slice() {
            got.push(v.len());
        }
    })
    .expect("scan");
    println!("scan from the start: {keys_seen} keys; the key's value lens {got:?}");
    let mut got2: Vec<usize> = Vec::new();
    db.scan(&key, 1, |k, v| {
        if k == key.as_slice() {
            got2.push(v.len());
        }
    })
    .expect("scan at key");
    println!("scan from the key, limit 1: value lens {got2:?}");
}
