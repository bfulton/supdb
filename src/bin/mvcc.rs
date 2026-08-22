//! Mutability and read-as-of, under both retention policies.
use supdb::{Options, Reader, Reclaim, Store};

fn key(k: usize) -> Vec<u8> {
    format!("k{:08}", k).into_bytes()
}

fn run(reclaim: Reclaim, label: &str) -> std::io::Result<()> {
    let dir = std::env::temp_dir().join(format!("supdb-mvcc-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("s.dat");

    let store = Store::create(&path, Options { reclaim, ..Default::default() })?;
    for k in 0..1000 {
        store.append(&key(k), format!("v1-{k}").as_bytes())?;
    }
    let g1 = store.checkpoint()?;

    // mutate: overwrite the first third, delete the second third
    for k in 0..333 {
        store.put(&key(k), format!("v2-{k}").as_bytes())?;
    }
    for k in 333..666 {
        store.delete(&key(k))?;
    }
    // Churn every key, repeatedly. Overwriting all of them retires the packed
    // blocks the originals lived in, so their space really does get handed
    // back out -- which is the only situation in which reading gen 1 is unsafe.
    for _round in 0..20 {
        for k in 0..1000 {
            store.put(&key(k), format!("v3-{k}-padded-out-so-the-slot-is-worth-reusing").as_bytes())?;
        }
    }
    let g2 = store.checkpoint()?;
    let st = store.close()?;
    println!("  (slots reused: {}, {} bytes; free left {})", st.reused, st.reused_bytes, st.free_bytes);

    let now = Reader::open(&path)?;
    let (mut over, mut del, mut untouched) = (0, 0, 0);
    for k in 0..1000usize {
        let mut got = Vec::new();
        now.read_all(&key(k), |v| got.push(String::from_utf8_lossy(v).to_string()))?;
        match k {
            _ if k < 333 => {
                if got == vec![format!("v2-{k}")] { over += 1 }
            }
            _ if k < 666 => {
                if got.is_empty() { del += 1 }
            }
            _ => {
                if got == vec![format!("v1-{k}")] { untouched += 1 }
            }
        }
    }
    println!("  gen {g2} (current): {over}/333 overwritten, {del}/333 deleted, {untouched}/334 untouched");

    // by transaction id -- refused outright if reclaim broke the history
    match Reader::open_as_of(&path, g1) {
        Ok(past) => {
            println!("     (reader knows of {} overwritten ranges)", past.overwritten_ranges());
            let (mut ok, mut refused) = (0, 0);
            let (mut by_range, mut by_decode) = (0, 0);
            let mut first_msg = String::new();
            for k in 0..1000usize {
                let mut got = Vec::new();
                match past.read_all(&key(k), |v| got.push(String::from_utf8_lossy(v).to_string())) {
                    Ok(_) if got == vec![format!("v1-{k}")] => ok += 1,
                    Ok(_) => {}
                    Err(e) => {
                        refused += 1;
                        let m = e.to_string();
                        if m.starts_with("this value was stored") { by_range += 1 } else { by_decode += 1 }
                        if first_msg.is_empty() { first_msg = m; }
                    }
                }
            }
            println!("  gen {g1} (as-of): {ok}/1000 readable, {refused} refused ({by_range} by range check, {by_decode} by decoder)");
            if !first_msg.is_empty() {
                println!("     first refusal: {}", &first_msg[..first_msg.len().min(150)]);
            }
        }
        Err(e) => println!("  gen {g1} (as-of by txn id): REFUSED -- {e}"),
    }

    // and by wall-clock time: anything at or after t1 but before t2
    let t1 = Reader::open_as_of(&path, g1).map(|r| r.version().1).unwrap_or(0);
    let (_, t2) = Reader::open(&path)?.version();
    match Reader::open_as_of_time(&path, t1) {
        Ok(by_time) => {
            let (gsel, _) = by_time.version();
            let mut t_ok = 0;
            for k in 0..1000usize {
                let mut got = Vec::new();
                by_time.read_all(&key(k), |v| got.push(String::from_utf8_lossy(v).to_string()))?;
                if got == vec![format!("v1-{k}")] { t_ok += 1 }
            }
            println!("  t1 (as-of by time):      selected gen {gsel}, {t_ok}/1000 original  [t2={t2}]");
        }
        Err(e) => println!("  t1 (as-of by time):      REFUSED -- {e}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

fn main() -> std::io::Result<()> {
    println!("Reclaim::Never -- prior versions kept, older generations readable");
    run(Reclaim::Never, "snap")?;
    println!("Reclaim::Now -- prior versions released to the free list");
    run(Reclaim::Now, "recl")?;
    Ok(())
}
