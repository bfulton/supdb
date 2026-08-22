//! One writer, many concurrent readers -- the premise the whole design rests
//! on and the one thing never yet tested here.
//!
//! A reader opens at a checkpoint and holds that state: the writer appends and
//! checkpoints underneath it without disturbing anything the reader can see,
//! because nothing is ever overwritten in place. What must hold is that every
//! open yields a complete, self-consistent state, that states only move
//! forward, and that no read ever observes a partially written one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use supdb::{Options, Reader, Reclaim, Store};

fn key(k: usize) -> Vec<u8> {
    format!("k{:08}", k).into_bytes()
}

fn main() -> std::io::Result<()> {
    let keys = 2000usize;
    let rounds = 60usize;
    let readers = 4usize;

    let dir = std::env::temp_dir().join("supdb-concur");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("s.dat");

    let reclaim = match std::env::var("SUPDB_RECLAIM").as_deref() {
        Ok("now") => Reclaim::Now,
        Ok("delay") => Reclaim::AfterDelay(8),
        Ok("onclose") => Reclaim::OnClose,
        Ok("never") => Reclaim::Never,
        _ => Reclaim::AfterReads,
    };
    println!("reclaim policy: {reclaim:?}");
    let store = Store::create(
        &path,
        Options {
            buffer_bytes: 1 << 20,
            reclaim,
            ..Default::default()
        },
    )?;
    store.checkpoint()?;

    let done = Arc::new(AtomicBool::new(false));
    let opens = Arc::new(AtomicU64::new(0));
    let torn = Arc::new(AtomicU64::new(0));
    let backwards = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let open_err = Arc::new(AtomicU64::new(0));
    let first_err = Arc::new(std::sync::Mutex::new(String::new()));

    std::thread::scope(|scope| -> std::io::Result<()> {
        for _ in 0..readers {
            let (path, done, opens, torn, backwards, errors, open_err, first_err) = (
                path.clone(),
                done.clone(),
                opens.clone(),
                torn.clone(),
                backwards.clone(),
                errors.clone(),
                open_err.clone(),
                first_err.clone(),
            );
            scope.spawn(move || {
                let mut high = 0u64;
                while !done.load(Ordering::Relaxed) {
                    let r = match Reader::open(&path) {
                        Ok(r) => r,
                        Err(_) => {
                            open_err.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    };
                    opens.fetch_add(1, Ordering::Relaxed);
                    let (gen, _) = r.version();
                    if gen < high {
                        backwards.fetch_add(1, Ordering::Relaxed);
                    }
                    high = high.max(gen);
                    // every key present in this state must hold a whole number
                    // of complete values, all of them readable
                    let mut counts = Vec::new();
                    for k in (0..keys).step_by(97) {
                        let mut n = 0usize;
                        match r.read_all(&key(k), |v| {
                            if v.len() < 4 {
                                torn.fetch_add(1, Ordering::Relaxed);
                            }
                            n += 1;
                        }) {
                            Ok(_) => counts.push(n),
                            Err(e) => {
                                errors.fetch_add(1, Ordering::Relaxed);
                                let mut g = first_err.lock().unwrap();
                                if g.is_empty() {
                                    *g = e.to_string();
                                }
                            }
                        }
                    }
                    // a checkpoint is taken between rounds, so within one state
                    // every key should have received the same number of values
                    if let (Some(&a), Some(&b)) = (counts.iter().min(), counts.iter().max()) {
                        if a != b {
                            torn.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }

        let mut val = vec![b'x'; 96];
        for round in 0..rounds {
            for k in 0..keys {
                val[..8].copy_from_slice(&((round as u64) << 32 | k as u64).to_be_bytes());
                store.append(&key(k), &val)?;
            }
            store.checkpoint()?;
        }
        done.store(true, Ordering::Relaxed);
        Ok(())
    })?;

    let final_gen = Reader::open(&path)?.version().0;
    println!(
        "writer: {rounds} rounds x {keys} keys, {readers} concurrent readers\n  \
         reader opens: {}\n  final generation: {final_gen}\n  \
         torn or inconsistent states: {}\n  generations seen going backwards: {}\n  \
         open failures: {}\n  read errors: {}\n  first read error: {}",
        opens.load(Ordering::Relaxed),
        torn.load(Ordering::Relaxed),
        backwards.load(Ordering::Relaxed),
        open_err.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        first_err.lock().unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
