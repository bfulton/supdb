//! A live store under a cipher the caller supplies, read through a cache
//! smaller than the store.
//!
//! ```sh
//! cargo run --release --example cipher_store -- [KEYS] [CACHE_MIB]
//! ```
//!
//! `supdb` ships no cipher. `Gcm` below is the whole of what a caller
//! writes to give a store one, here over `ring`'s AES-256-GCM, and
//! `StoreCipher::new` takes it twice: the cipher under the caller's own
//! key, which wraps the key the store draws for itself and nothing else,
//! and the constructor of the same cipher from a key, which the store calls
//! for its own key and for the key it draws for each file.
//!
//! The example makes the same store twice, in the clear and under the
//! cipher, closes both, and opens each again. A store under a cipher reads
//! its files through a cache of plaintext (`StoreCipher::cache_bytes`):
//! an open decrypts what it reads of each file and nothing else, and a
//! read decrypts the chunk it wants unless the cache holds it. This prints
//! the two opens, and then random point reads and a scan of the whole
//! store through a cache a fraction of its size, with what the cache held,
//! decrypted and gave back.

use ring::aead::{Aad, LessSafeKey, Nonce, Tag, UnboundKey, AES_256_GCM};
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::crypt::{Aead, NONCE, TAG};
use supdb::{Db, Encryption, Options, StoreCipher};

struct Gcm(LessSafeKey);

impl Gcm {
    fn new(key: &[u8; 32]) -> Gcm {
        Gcm(LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key).expect("a 32-byte key"),
        ))
    }
}

impl Aead for Gcm {
    fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG] {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        let tag = self
            .0
            .seal_in_place_separate_tag(nonce, Aad::from(aad), buf)
            .expect("under the cipher's limit");
        tag.as_ref().try_into().expect("a sixteen-byte tag")
    }
    fn decrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8], tag: &[u8; TAG]) -> bool {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        self.0
            .open_in_place_separate_tag(nonce, Aad::from(aad), Tag::from(*tag), buf, 0..)
            .is_ok()
    }
}

/// The options of a store under the caller's key: the key is the caller's
/// to keep, and `key_id` is the caller's name for it, which the store
/// writes in the clear so that `supdb::cipher_key_id` can say which key a
/// directory wants.
fn under(key: &[u8; 32], cache: u64) -> Options {
    let cipher = StoreCipher::new(Gcm::new(key), Gcm::new)
        .key_id(1)
        .cache_bytes(cache);
    Options {
        encryption: Encryption::Cipher(cipher),
        ..Options::default()
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-example-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn fill(mut db: Db, keys: u32) -> std::io::Result<()> {
    let value = [0x61u8; 100];
    for i in 0..keys {
        db.put(format!("{i:016}").as_bytes(), &value);
        if i % 1000 == 999 {
            db.commit()?;
        }
    }
    db.commit()?;
    db.close()
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok()?.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// The store opened and one key read, in milliseconds, the least of
/// `reps`.
fn open_ms(dir: &Path, opts: impl Fn() -> Options, reps: usize) -> std::io::Result<f64> {
    let mut best = f64::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        let db = Db::open(dir, opts())?;
        let mut seen = 0;
        db.read_all(format!("{:016}", 0).as_bytes(), |_| seen += 1)?;
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(seen, 1);
    }
    Ok(best)
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let keys: u32 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(1_000_000);
    let cache_mib: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(16);
    let cache = cache_mib << 20;
    let key = [0x5a; 32];

    let plain = scratch("plain");
    fill(Db::create(&plain, Options::default())?, keys)?;
    let sealed = scratch("cipher");
    fill(Db::create(&sealed, under(&key, cache))?, keys)?;
    assert_eq!(supdb::cipher_key_id(&sealed)?, Some(1));

    let plain_ms = open_ms(&plain, Options::default, 5)?;
    let sealed_ms = open_ms(&sealed, || under(&key, cache), 5)?;
    println!(
        "{keys} keys: {:.1} MB in the clear, {:.1} MB under the cipher, a cache of {cache_mib} MiB",
        dir_bytes(&plain) as f64 / 1e6,
        dir_bytes(&sealed) as f64 / 1e6
    );
    println!("open, in the clear:     {plain_ms:8.2} ms");
    let db = Db::open(&sealed, under(&key, cache))?;
    let c = db.cipher_counts().expect("a store under a cipher").cache;
    println!(
        "open, under the cipher: {sealed_ms:8.2} ms, {} chunks decrypted ({:.2} MB)",
        c.fills,
        c.fills as f64 * 4080.0 / 1e6
    );

    // Point reads far apart, then every key in order.
    let reads = 200_000u32.min(keys);
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let t = Instant::now();
    for _ in 0..reads {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let i = (s % u64::from(keys)) as u32;
        let mut seen = 0;
        db.read_all(format!("{i:016}").as_bytes(), |_| seen += 1)?;
        assert_eq!(seen, 1, "key {i}");
    }
    let read_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut seen = 0u32;
    db.scan(b"", usize::MAX, |_, _| seen += 1)?;
    assert_eq!(seen, keys);
    let scan_s = t.elapsed().as_secs_f64();
    let c = db.cipher_counts().expect("a store under a cipher").cache;
    println!(
        "{reads} point reads: {:.0} a second; a scan of {keys} keys: {:.1} M keys a second",
        f64::from(reads) / read_s,
        f64::from(keys) / scan_s / 1e6
    );
    println!(
        "the cache: {:.1} MiB held of {cache_mib}, {:.1} MiB at its most; {} chunks decrypted, {:.0} MiB given back",
        c.held as f64 / (1 << 20) as f64,
        c.peak as f64 / (1 << 20) as f64,
        c.fills,
        c.evicted_bytes as f64 / (1 << 20) as f64
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&plain);
    let _ = std::fs::remove_dir_all(&sealed);
    Ok(())
}
