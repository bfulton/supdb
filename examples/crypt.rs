//! Encrypt a finished supdb file, or decrypt one, under AES-256-GCM.
//!
//! ```sh
//! cargo run --release --example crypt -- encrypt KEYFILE IN OUT
//! cargo run --release --example crypt -- decrypt KEYFILE IN OUT
//! cargo run --release --example crypt -- header IN
//! ```
//!
//! `KEYFILE` holds the key and nothing else: 32 bytes, as they are. `supdb`
//! ships no cipher; `Gcm` below is the whole of what a caller writes to give
//! it one, here over `ring`'s AES-GCM. Reading an encrypted file in place,
//! without a decrypted copy, is `Blob::open(Encrypted::open(src, key)?)`.
//!
//! `OUT` is written under a temporary name, synced and renamed into place,
//! since a reader may hold the file it replaces open.

use ring::aead::{Aad, LessSafeKey, Nonce, Tag, UnboundKey, AES_256_GCM};
use std::io::{Error, ErrorKind, Result, Write};
use std::path::Path;
use supdb::crypt::{self, Aead, EncryptOptions, Encrypted, Header, NONCE, TAG};
use supdb::{Bytes, MmapBytes};

struct Gcm(LessSafeKey);

impl Aead for Gcm {
    fn encrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8]) -> [u8; TAG] {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        let tag = self
            .0
            .seal_in_place_separate_tag(nonce, Aad::from(aad), buf)
            .expect("a chunk is far under the cipher's limit");
        tag.as_ref().try_into().expect("a sixteen-byte tag")
    }
    fn decrypt(&self, nonce: &[u8; NONCE], aad: &[u8], buf: &mut [u8], tag: &[u8; TAG]) -> bool {
        let nonce = Nonce::assume_unique_for_key(*nonce);
        self.0
            .open_in_place_separate_tag(nonce, Aad::from(aad), Tag::from(*tag), buf, 0..)
            .is_ok()
    }
}

fn key(path: &Path) -> Result<Gcm> {
    let raw = std::fs::read(path)?;
    let key: &[u8; 32] = raw.as_slice().try_into().map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            format!("{} holds {} bytes; a key is 32", path.display(), raw.len()),
        )
    })?;
    Ok(Gcm(LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, key).expect("a 32-byte key"),
    )))
}

/// Write `out` whole or not at all: a temporary name beside it, synced,
/// then renamed over it.
fn publish(out: &Path, write: impl FnOnce(&mut std::fs::File) -> Result<()>) -> Result<()> {
    let tmp = out.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp)?;
    write(&mut f)?;
    f.sync_all()?;
    std::fs::rename(&tmp, out)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args[..] {
        ["encrypt", keyfile, input, output] => {
            let (aead, src) = (key(Path::new(keyfile))?, MmapBytes::open(Path::new(input))?);
            publish(Path::new(output), |f| {
                let h = crypt::encrypt(&src, f, &aead, &EncryptOptions::default())?;
                println!("{} bytes in {} chunks", h.stored_len(), h.chunks());
                Ok(())
            })
        }
        ["decrypt", keyfile, input, output] => {
            let src =
                Encrypted::open(MmapBytes::open(Path::new(input))?, key(Path::new(keyfile))?)?;
            publish(Path::new(output), |f| {
                let mut buf = vec![0u8; 1 << 20];
                let mut at = 0;
                while at < src.len() {
                    let n = buf.len().min((src.len() - at) as usize);
                    src.read_at(at, &mut buf[..n])?;
                    f.write_all(&buf[..n])?;
                    at += n as u64;
                }
                println!("{at} bytes");
                Ok(())
            })
        }
        ["header", input] => {
            let h = Header::read(&MmapBytes::open(Path::new(input))?)?;
            println!(
                "plaintext {} bytes, chunks of {}, key id {:#x}, id {}",
                h.plain_len(),
                h.chunk_len(),
                h.key_id(),
                h.id()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            Ok(())
        }
        _ => {
            eprintln!("usage: crypt encrypt|decrypt KEYFILE IN OUT\n       crypt header IN");
            std::process::exit(2);
        }
    }
}
