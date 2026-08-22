//! The key table on its own, to find out whether it is actually what makes a
//! put cost more as a store fills.
//!
//! The write path's cost grows with the number of distinct keys, and the table
//! was the assumed reason. That was an inference from the shape of the curve,
//! not a measurement of the table.

use supdb::keytable::KeyTable;
use std::time::Instant;

fn db_key(n: u64, buf: &mut [u8; 16]) {
    let mut v = n;
    for i in (0..16).rev() {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
}

fn main() {
    println!("{:>12} {:>14} {:>12} {:>14}", "keys", "insert ns/op", "hit ns/op", "insert ops/s");
    for &n in &[100_000u64, 1_000_000, 4_000_000, 8_000_000] {
        let mut t: KeyTable<()> = KeyTable::new();
        let mut buf = [0u8; 16];
        let start = Instant::now();
        for i in 0..n {
            db_key(i, &mut buf);
            t.get_or_insert(&buf);
        }
        let ins = start.elapsed().as_secs_f64();

        // lookups of keys that are present, in a scattered order
        let start = Instant::now();
        let mut found = 0u64;
        let mut x = 12345u64;
        for _ in 0..n {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            db_key(x % n, &mut buf);
            if t.get_mut(&buf).is_some() { found += 1 }
        }
        let hit = start.elapsed().as_secs_f64();
        assert_eq!(found, n);
        println!("{:>12} {:>14.1} {:>12.1} {:>14.0}",
                 n, ins * 1e9 / n as f64, hit * 1e9 / n as f64, n as f64 / ins);
    }
}
