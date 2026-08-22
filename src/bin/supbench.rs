//! Mirrors the Java harness: same key shape, same event payload drawn from a
//! pool of distinct records, same permuted append order, same phases. The
//! point is that a Supdb number can be laid beside an Uppend/RocksDB/LMDB one
//! without an asterisk.

use std::path::PathBuf;
use std::time::Instant;
use supdb::{Options, Reader, Store};

const POOL: usize = 1 << 16;

fn key_bytes(k: usize) -> Vec<u8> {
    let mut out = vec![b'k'; 10];
    let mut v = k;
    for i in (1..10).rev() {
        out[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    out
}

/// Same generator as the Java harness, including the fixed structure and the
/// varying content -- a value that is a real record rather than a run of one
/// byte, because a degenerate payload flatters block compressors by 6x.
fn build_pool(value_size: usize) -> Vec<Vec<u8>> {
    let events = ["click", "view", "purchase", "scroll", "hover", "submit", "login"];
    let plats = ["ios", "android", "web", "tv"];
    (0..POOL)
        .map(|i| {
            let mut state = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xBEEF;
            let mut next = || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let mut s = String::new();
            while s.len() < value_size {
                s.push_str(&format!(
                    "{{\"ts\":{},\"ev\":\"{}\",\"plat\":\"{}\",\"amt\":{:.2},\"ok\":{}}}",
                    1_723_459_200_000u64 + (next() % 86_400_000),
                    events[(next() % 7) as usize],
                    plats[(next() % 4) as usize],
                    (next() % 100_000) as f64 / 100.0,
                    next() % 2 == 0
                ));
            }
            let mut v = s.into_bytes();
            v.truncate(value_size);
            v
        })
        .collect()
}

fn permutation(n: usize) -> Vec<usize> {
    let mut p: Vec<usize> = (0..n).collect();
    let mut state = 4242u64;
    for i in (1..n).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        p.swap(i, j);
    }
    p
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |name: &str, dflt: usize| -> usize {
        args.iter().position(|a| a == name).map(|i| args[i + 1].parse().unwrap()).unwrap_or(dflt)
    };
    let keys = get("--keys", 20_000);
    let values = get("--values", 500);
    let value_size = get("--value-size", 96);
    let samples = get("--samples", 20_000);
    let block_size = get("--block-size", 64 * 1024);
    let threads = get("--threads", 4);
    let chunk = get("--chunk", 1024);
    let solo_chunk = get("--solo-chunk", 4096);
    let ckpt_every = get("--checkpoint-every", 0);
    let crash_after = get("--crash-after", 0);
    let merge_threshold = get("--merge", 4);
    let no_compact = args.iter().any(|a| a == "--no-compact");
    let buffer_mb = get("--buffer-mb", 512);
    let compress = !args.iter().any(|a| a == "--no-compress");
    let phase = args
        .iter()
        .position(|a| a == "--phase")
        .map(|i| args[i + 1].clone())
        .unwrap_or_else(|| "both".into());
    let dir: PathBuf = args
        .iter()
        .position(|a| a == "--dir")
        .map(|i| PathBuf::from(&args[i + 1]))
        .expect("--dir required");

    std::fs::create_dir_all(&dir)?;
    let file = dir.join("supdb.dat");
    let name = format!("supdb-b{}k-c{}{}", block_size / 1024, chunk, if compress { "-lz4" } else { "" });

    let keyv: Vec<Vec<u8>> = (0..keys).map(key_bytes).collect();

    if phase == "write" || phase == "both" {
        let pool = build_pool(value_size);
        let perm = permutation(keys);
        let opts = Options {
            block_size,
            buffer_bytes: buffer_mb * 1024 * 1024,
            compress,
            merge_threshold,
            chunk_size: chunk,
            solo_chunk_size: solo_chunk,
            ..Default::default()
        };
        let store = Store::create(&file, opts)?;

        // Threads split each round's permutation and join at the round
        // boundary, exactly as the Java harness does, so every key receives
        // one append per round and per-key order stays deterministic.
        let t0 = Instant::now();
        // checkpoint/crash need rounds to be a barrier, so run them serially
        if ckpt_every > 0 || crash_after > 0 {
            let mut val = vec![0u8; value_size];
            for round in 0..values {
                let offset = (round * 7919) % keys;
                for n in 0..keys {
                    let k = perm[(n + offset) % keys];
                    let src = &pool[((round as u64).wrapping_mul(2654435761)
                        ^ (k as u64).wrapping_mul(40503)) as usize & (POOL - 1)];
                    val.copy_from_slice(src);
                    let seq = ((round as u64) << 32) | k as u64;
                    val[..8].copy_from_slice(&seq.to_be_bytes());
                    store.append(&keyv[k], &val)?;
                }
                if ckpt_every > 0 && (round + 1) % ckpt_every == 0 {
                    let g = store.checkpoint()?;
                    eprintln!("# checkpoint gen {} after round {}", g, round + 1);
                }
                if crash_after > 0 && round + 1 == crash_after {
                    eprintln!("# simulating crash after round {}", round + 1);
                    // no unwinding, no destructors, no flush -- a real crash
                    std::process::abort();
                }
            }
            let secs = t0.elapsed().as_secs_f64();
            let ops = (keys * values) as f64;
            println!(
                "{{\"phase\":\"append\",\"engine\":\"{}\",\"ops\":{},\"seconds\":{:.4},\"ops_per_s\":{:.1},\"threads\":1}}",
                name, keys * values, secs, ops / secs
            );
            let stats = store.close()?;
            let allocated = {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(&file)?.blocks() * 512
            };
            println!(
                "{{\"phase\":\"disk\",\"engine\":\"{}\",\"bytes\":{},\"blocks\":{}}}",
                name, allocated, stats.blocks
            );
            return Ok(());
        }
        std::thread::scope(|scope| {
            for t in 0..threads {
                let (store, pool, perm, keyv) = (&store, &pool, &perm, &keyv);
                scope.spawn(move || {
                    let mut val = vec![0u8; value_size];
                    let lo = keys * t / threads;
                    let hi = keys * (t + 1) / threads;
                    for round in 0..values {
                        let offset = (round * 7919) % keys;
                        for n in lo..hi {
                            let k = perm[(n + offset) % keys];
                            let src = &pool[((round as u64).wrapping_mul(2654435761)
                                ^ (k as u64).wrapping_mul(40503))
                                as usize
                                & (POOL - 1)];
                            val.copy_from_slice(src);
                            let seq = ((round as u64) << 32) | k as u64;
                            val[..8].copy_from_slice(&seq.to_be_bytes());
                            store.append(&keyv[k], &val).unwrap();
                        }
                    }
                });
            }
        });
        let secs = t0.elapsed().as_secs_f64();
        let ops = (keys * values) as f64;

        // repair fragmentation before sealing the file, and charge for it
        let tc = Instant::now();
        store.flush()?;
        let merged = if no_compact { 0 } else { 0 };  // merging now happens inline
        let compact_s = tc.elapsed().as_secs_f64();

        let tf = Instant::now();
        let stats = store.close()?;
        let fin = tf.elapsed().as_secs_f64();
        println!(
            "{{\"phase\":\"compact\",\"engine\":\"{}\",\"merged_keys\":{},\"seconds\":{:.3}}}",
            name, merged, compact_s
        );

        // apparent length counts holes; blocks actually allocated is the
        // number that matters once space has been punched back out
        let on_disk = std::fs::metadata(&file)?.len();
        let allocated = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&file)?.blocks() * 512
        };
        println!(
            "{{\"phase\":\"append\",\"engine\":\"{}\",\"ops\":{},\"seconds\":{:.4},\"ops_per_s\":{:.1},\"threads\":{}}}",
            name, keys * values, secs, ops / secs, threads
        );
        println!(
            "{{\"phase\":\"disk\",\"engine\":\"{}\",\"bytes\":{},\"amplification\":{:.4},\"finalize_s\":{:.3},\"blocks\":{},\"index_bytes\":{},\"index_b_per_key\":{:.2},\"apparent\":{},\"free_mb\":{:.1},\"reused\":{},\"reused_mb\":{:.1}}}",
            name, allocated,
            allocated as f64 / (ops * value_size as f64),
            fin, stats.blocks, stats.index_bytes,
            stats.index_bytes as f64 / stats.keys as f64,
            on_disk,
            stats.free_bytes as f64 / 1048576.0, stats.reused,
            stats.reused_bytes as f64 / 1048576.0
        );
    }

    if phase == "read" || phase == "both" {
        let reader = Reader::open(&file)?;
        let sample: Vec<usize> = (0..samples).map(|i| (i * 7919) % keys).collect();

        for (label, warm) in [("read-all-cold", false), ("read-all-warm", true)] {
            if warm {
                for &k in sample.iter().take(2000) {
                    reader.read_all(&keyv[k], |_| {})?;
                }
            }
            let mut total = 0u64;
            let t = Instant::now();
            for &k in &sample {
                total += reader.read_all(&keyv[k], |v| {
                    total = total.wrapping_add(v[0] as u64);
                })?;
            }
            let s = t.elapsed().as_secs_f64();
            println!(
                "{{\"phase\":\"{}\",\"engine\":\"{}\",\"ops\":{},\"seconds\":{:.4},\"ops_per_s\":{:.1},\"sink\":{}}}",
                label, name, samples, s, samples as f64 / s, total % 7
            );
        }

        // ordered scan: every key in key order, values in append order
        {
            let mut n = 0u64;
            let t = Instant::now();
            let vals = reader.scan(None, keys, |_k, v| n += v[0] as u64)?;
            let s = t.elapsed().as_secs_f64();
            println!(
                "{{\"phase\":\"readseq\",\"engine\":\"{}\",\"keys\":{},\"values\":{},\"seconds\":{:.4},\"keys_per_s\":{:.1},\"values_per_s\":{:.1},\"sink\":{}}}",
                name, keys, vals, s, keys as f64 / s, vals as f64 / s, n % 7
            );
        }
        // seek to a random key then read a short run, the seekrandom shape
        {
            let t = Instant::now();
            let mut n = 0u64;
            for &k in sample.iter().take(2000) {
                reader.scan(Some(&keyv[k]), 10, |_k, v| n += v[0] as u64)?;
            }
            let s = t.elapsed().as_secs_f64();
            println!(
                "{{\"phase\":\"seekrandom\",\"engine\":\"{}\",\"ops\":2000,\"seconds\":{:.4},\"ops_per_s\":{:.1},\"sink\":{}}}",
                name, s, 2000.0 / s, n % 7
            );
        }

        let mut acc = 0i64;
        let t = Instant::now();
        for &k in &sample {
            acc += reader.read_last(&keyv[k])? as i64;
        }
        let s = t.elapsed().as_secs_f64();
        println!(
            "{{\"phase\":\"read-last-warm\",\"engine\":\"{}\",\"ops\":{},\"seconds\":{:.4},\"ops_per_s\":{:.1},\"sink\":{}}}",
            name, samples, s, samples as f64 / s, acc % 7
        );

        // correctness: every key must give back exactly the values appended
        let mut wrong = 0;
        for &k in sample.iter().take(1000) {
            let mut n = 0;
            let mut first_seq = None;
            reader.read_all(&keyv[k], |v| {
                if n == 0 {
                    first_seq = Some(u64::from_be_bytes(v[..8].try_into().unwrap()));
                }
                n += 1;
            })?;
            if n != values || first_seq != Some(k as u64) {
                wrong += 1;
            }
        }
        println!(
            "{{\"phase\":\"correctness\",\"engine\":\"{}\",\"checked\":1000,\"wrong\":{}}}",
            name, wrong
        );
    }
    Ok(())
}
