//! One merge of a partition and the pieces over it, in the clear and under
//! a cipher, for a profile of where a merge's CPU goes under each: the
//! store and the merge `merge_walk` makes, with the suite's AES-GCM.
//!
//! usage: merge_cipher [KEYS] [PIECES] [plain|cipher] [CACHE_MIB] [stream]
use ring::aead::{Aad, LessSafeKey, Nonce, Tag, UnboundKey, AES_256_GCM};
use std::path::{Path, PathBuf};
use std::time::Instant;
use supdb::crypt::{Aead, NONCE, TAG};
use supdb::{Db, Encryption, Options, StoreCipher};

struct Gcm(LessSafeKey);

impl Gcm {
    fn of(key: &[u8; 32]) -> Gcm {
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

fn key(i: u64) -> Vec<u8> {
    format!("{i:016}").into_bytes()
}

fn opts(pieces: usize, cipher: bool, cache: u64) -> Options {
    Options {
        partition_bytes: Some(1 << 30),
        seal_bytes: 1 << 30,
        l0_trigger: pieces,
        zpages: supdb::ZPages::Off,
        encryption: if cipher {
            let c = StoreCipher::new(Gcm::of(&[0x5a; 32]), Gcm::of)
                .merges_beside_cache(std::env::args().nth(5).as_deref() == Some("stream"));
            Encryption::Cipher(if cache == 0 { c } else { c.cache_bytes(cache) })
        } else {
            Encryption::None
        },
        ..Options::default()
    }
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let keys: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(100_000);
    let pieces: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(4);
    let cipher = args.get(3).map(|a| a == "cipher").unwrap_or(false);
    let cache: u64 = args.get(4).and_then(|a| a.parse::<u64>().ok()).unwrap_or(0) << 20;
    let tmp = std::env::var_os("TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let base = tmp.join(format!("merge-cipher-base-{}", std::process::id()));
    let trial = tmp.join(format!("merge-cipher-trial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    {
        let mut o = opts(1000, cipher, cache);
        o.l0_trigger = 1000;
        let mut db = Db::create(&base, o).unwrap();
        let val = vec![b'v'; 100];
        for i in 0..keys {
            db.append(&key(i), &val);
        }
        db.commit().unwrap();
        db.flush().unwrap();
        let mut order: Vec<u64> = (0..keys).collect();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for i in (1..order.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            order.swap(i, (x % (i as u64 + 1)) as usize);
        }
        let per = (keys as usize).div_ceil(pieces);
        let val = vec![b'w'; 100];
        for chunk in order.chunks(per) {
            let mut c = chunk.to_vec();
            c.sort_unstable();
            for &i in &c {
                db.append(&key(i), &val);
            }
            db.commit().unwrap();
            db.seal().unwrap();
        }
        db.settle().unwrap();
        eprintln!("base levels {:?}", db.levels());
        drop(db);
    }
    copy_dir(&base, &trial);
    let mut db = Db::open(&trial, opts(pieces, cipher, cache)).unwrap();
    let before = db.levels();
    let t = Instant::now();
    db.append(&key(keys), b"x");
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    println!(
        "cipher {cipher} levels {before:?} -> {:?} seal+settle_ms {:.1}",
        db.levels(),
        t.elapsed().as_secs_f64() * 1e3
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&trial);
}
