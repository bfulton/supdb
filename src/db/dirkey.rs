//! A store's directory under a key, encrypted by the kernel.
//!
//! A live store reads by lending from its files' mappings, and the page
//! cache is what holds those bytes: the kernel pages a file in and out
//! under the store, and the engine's reads, its advice and its accounting
//! out of core all rest on that. Encryption the engine did itself would put
//! every byte it reads into memory of its own instead, decrypted there and
//! not the kernel's to reclaim, which is a different engine. So a live
//! store under a key asks the kernel: Linux's file encryption (`fscrypt`)
//! takes a key and a policy on a directory, and from then on every file
//! made in it is ciphertext on the device and plaintext in the page cache.
//! Nothing on a read path changes. A page is decrypted once, where the
//! kernel reads it in, and encrypted where it is written back.
//!
//! What a store does with a key (`Options::dir_key`):
//!
//! - `Db::create` hands the kernel the key and sets the policy on the
//!   store's directory before it writes anything into it. A directory that
//!   cannot be encrypted -- a filesystem without the feature, a kernel
//!   without it, another platform -- is refused, and nothing is written: a
//!   store asked for a key never writes plaintext.
//! - `Db::open` hands the kernel the key and holds the directory's policy
//!   to it: a directory that is not encrypted, or is under another key, is
//!   refused before anything is read or written.
//! - `Db::open` with no key opens an encrypted store while the kernel holds
//!   its key, and refuses one it does not, by name: the files' names are
//!   themselves encrypted, and a store read without them would look empty.
//!
//! What the kernel's encryption is and is not:
//!
//! - File contents and file names are encrypted (AES-256-XTS and
//!   AES-256-CTS, a key a file derived from the one given). Sizes, times
//!   and the number of files are not.
//! - It is not authenticated. A changed block decrypts to garbage, which
//!   the engine's checksums refuse as they refuse any damage; it is not a
//!   proof against someone who can write the device.
//! - It protects the device: a disk taken, a backup of it, a block the
//!   filesystem freed. While the kernel holds the key, the files read as
//!   plaintext to whoever the filesystem's permissions let read them, this
//!   process or another. `lock` gives the key up.
//! - It is Linux's, on a filesystem that has it (ext4 and f2fs with their
//!   `encrypt` feature on, `tune2fs -O encrypt` for ext4). A segment
//!   published elsewhere takes `crate::crypt`'s envelope instead, which is
//!   the engine's own and goes where the file goes.
//!
//! The key is the caller's, 32 bytes. The kernel names it by an identifier
//! it derives from the key, the policy on the directory holds that
//! identifier, and that is how a wrong key is told from the right one
//! without either being stored.

use std::io::{Error, ErrorKind, Result};
use std::path::Path;

/// The key a store's directory is encrypted under.
///
/// Thirty-two bytes, the caller's to make and to keep: lose it and the
/// store is lost. It is zeroed where a copy of it is dropped.
#[derive(Clone)]
pub struct DirKey([u8; 32]);

impl DirKey {
    pub fn new(raw: [u8; 32]) -> DirKey {
        DirKey(raw)
    }
}

impl Drop for DirKey {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // SAFETY: a write through a reference to a byte this value
            // owns; volatile so that it is not taken for a dead store.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

impl std::fmt::Debug for DirKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DirKey(..)")
    }
}

/// What the kernel says of a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirState {
    /// Not encrypted: no policy on it, or a filesystem or a platform that
    /// has none to give.
    Plain,
    /// Encrypted, and the kernel holds its key: files read and write.
    Unlocked,
    /// Encrypted, and the kernel does not hold its key.
    Locked,
}

fn refused(dir: &Path, why: &str) -> Error {
    Error::new(
        ErrorKind::Unsupported,
        format!("supdb: the store at {}: {why}", dir.display()),
    )
}

#[cfg(target_os = "linux")]
mod sys {
    //! The kernel's interface, as `linux/fscrypt.h` has it. Four requests
    //! on a descriptor of the directory, each a fixed struct.
    use super::{DirKey, DirState};
    use std::io::{Error, Result};
    use std::os::fd::AsRawFd;
    use std::path::Path;

    /// `_IOC(dir, 'f', nr, size)`: the request numbers below are these, so
    /// that the sizes they carry are written where the structs are.
    const fn ioc(dir: u64, nr: u64, size: u64) -> u64 {
        (dir << 30) | (size << 16) | ((b'f' as u64) << 8) | nr
    }
    const READ: u64 = 2;
    const READ_WRITE: u64 = 3;
    /// `FS_IOC_SET_ENCRYPTION_POLICY`: numbered by the first policy
    /// struct's twelve bytes, and given the second's twenty-four.
    const SET_POLICY: u64 = ioc(READ, 19, 12);
    /// `FS_IOC_GET_ENCRYPTION_POLICY_EX`.
    const GET_POLICY_EX: u64 = ioc(READ_WRITE, 22, 9);
    /// `FS_IOC_ADD_ENCRYPTION_KEY`.
    const ADD_KEY: u64 = ioc(READ_WRITE, 23, 80);
    /// `FS_IOC_REMOVE_ENCRYPTION_KEY`.
    const REMOVE_KEY: u64 = ioc(READ_WRITE, 24, 64);
    /// `FS_IOC_GET_ENCRYPTION_KEY_STATUS`.
    const KEY_STATUS: u64 = ioc(READ_WRITE, 26, 128);

    const POLICY_V2: u8 = 2;
    const MODE_AES_256_XTS: u8 = 1;
    const MODE_AES_256_CTS: u8 = 4;
    /// File names padded to 32 bytes, the most the policy offers: a name's
    /// length is what a listing of a locked directory still shows.
    const FLAGS_PAD_32: u8 = 3;
    const SPEC_IDENTIFIER: u32 = 2;
    const STATUS_PRESENT: u32 = 2;
    const REMOVAL_FILES_BUSY: u32 = 1;
    const REMOVAL_OTHER_USERS: u32 = 2;

    /// `struct fscrypt_policy_v2`.
    #[repr(C)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct PolicyV2 {
        version: u8,
        contents_mode: u8,
        filenames_mode: u8,
        flags: u8,
        log2_data_unit_size: u8,
        reserved: [u8; 3],
        identifier: [u8; 16],
    }

    /// `struct fscrypt_get_policy_ex_arg`, with room for a policy of the
    /// second version.
    #[repr(C)]
    struct GetPolicy {
        size: u64,
        policy: PolicyV2,
    }

    /// `struct fscrypt_key_specifier`.
    #[repr(C)]
    struct KeySpec {
        kind: u32,
        reserved: u32,
        /// The identifier in the first sixteen of the union's 32 bytes.
        identifier: [u8; 16],
        rest: [u8; 16],
    }

    /// `struct fscrypt_add_key_arg` with a 32-byte key after it.
    #[repr(C)]
    struct AddKey {
        spec: KeySpec,
        raw_size: u32,
        key_id: u32,
        reserved: [u32; 8],
        raw: [u8; 32],
    }

    /// `struct fscrypt_remove_key_arg`.
    #[repr(C)]
    struct RemoveKey {
        spec: KeySpec,
        removal_status_flags: u32,
        reserved: [u32; 5],
    }

    /// `struct fscrypt_get_key_status_arg`.
    #[repr(C)]
    struct KeyStatus {
        spec: KeySpec,
        reserved: [u32; 6],
        status: u32,
        status_flags: u32,
        user_count: u32,
        out_reserved: [u32; 13],
    }

    const _: () = assert!(std::mem::size_of::<PolicyV2>() == 24);
    const _: () = assert!(std::mem::size_of::<GetPolicy>() == 32);
    const _: () = assert!(std::mem::size_of::<AddKey>() == 80 + 32);
    const _: () = assert!(std::mem::size_of::<RemoveKey>() == 64);
    const _: () = assert!(std::mem::size_of::<KeyStatus>() == 128);

    fn spec(identifier: [u8; 16]) -> KeySpec {
        KeySpec {
            kind: SPEC_IDENTIFIER,
            reserved: 0,
            identifier,
            rest: [0; 16],
        }
    }

    /// One request on the directory, `arg` the struct the request names.
    fn ask<T>(dir: &std::fs::File, request: u64, arg: &mut T) -> Result<()> {
        // SAFETY: `arg` is the `repr(C)` struct `request` is defined to
        // read and write, as large as the request's number says or, for
        // the two that take a trailing array, as large as their own size
        // field says; the descriptor is open for the call.
        let rc = unsafe { libc::ioctl(dir.as_raw_fd(), request as _, arg as *mut T) };
        if rc == 0 {
            Ok(())
        } else {
            Err(Error::last_os_error())
        }
    }

    /// The directory's policy: `None` for a directory with none, and for a
    /// filesystem or a kernel with none to give.
    fn policy(dir: &std::fs::File) -> Result<Option<PolicyV2>> {
        let mut arg = GetPolicy {
            size: std::mem::size_of::<PolicyV2>() as u64,
            policy: PolicyV2 {
                version: 0,
                contents_mode: 0,
                filenames_mode: 0,
                flags: 0,
                log2_data_unit_size: 0,
                reserved: [0; 3],
                identifier: [0; 16],
            },
        };
        match ask(dir, GET_POLICY_EX, &mut arg) {
            Ok(()) => Ok(Some(arg.policy)),
            Err(e) => match e.raw_os_error() {
                // No policy on it; no encryption on this filesystem; no
                // such request here at all.
                Some(libc::ENODATA) | Some(libc::EOPNOTSUPP) | Some(libc::ENOTTY) => Ok(None),
                _ => Err(e),
            },
        }
    }

    /// The key handed to the kernel, for the filesystem `dir` is on: its
    /// identifier. Handing over a key the kernel already holds is the same
    /// answer.
    fn add(dir: &std::fs::File, key: &DirKey) -> Result<[u8; 16]> {
        let mut arg = AddKey {
            spec: spec([0; 16]),
            raw_size: 32,
            key_id: 0,
            reserved: [0; 8],
            raw: key.0,
        };
        let r = ask(dir, ADD_KEY, &mut arg);
        // The copy handed over, whatever the answer.
        for b in arg.raw.iter_mut() {
            // SAFETY: a write through a reference to a byte of a local.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        r.map(|()| arg.spec.identifier)
    }

    /// The key given up for this user. `true` once the kernel no longer
    /// holds it; `false` where files under it are still open, or another
    /// user handed it over too.
    fn remove(dir: &std::fs::File, identifier: [u8; 16]) -> Result<bool> {
        let mut arg = RemoveKey {
            spec: spec(identifier),
            removal_status_flags: 0,
            reserved: [0; 5],
        };
        match ask(dir, REMOVE_KEY, &mut arg) {
            Ok(()) => {
                Ok(arg.removal_status_flags & (REMOVAL_FILES_BUSY | REMOVAL_OTHER_USERS) == 0)
            }
            // Not held at all is given up already.
            Err(e) if e.raw_os_error() == Some(libc::ENOKEY) => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn held(dir: &std::fs::File, identifier: [u8; 16]) -> Result<bool> {
        let mut arg = KeyStatus {
            spec: spec(identifier),
            reserved: [0; 6],
            status: 0,
            status_flags: 0,
            user_count: 0,
            out_reserved: [0; 13],
        };
        ask(dir, KEY_STATUS, &mut arg)?;
        Ok(arg.status == STATUS_PRESENT)
    }

    fn ours(identifier: [u8; 16]) -> PolicyV2 {
        PolicyV2 {
            version: POLICY_V2,
            contents_mode: MODE_AES_256_XTS,
            filenames_mode: MODE_AES_256_CTS,
            flags: FLAGS_PAD_32,
            log2_data_unit_size: 0,
            reserved: [0; 3],
            identifier,
        }
    }

    fn why(e: &Error, doing: &str) -> String {
        let hint = match e.raw_os_error() {
            Some(libc::EOPNOTSUPP) | Some(libc::ENOTTY) => {
                ": this filesystem has no file encryption to ask for (ext4 and f2fs have it \
                 with their `encrypt` feature on)"
            }
            Some(libc::ENOTEMPTY) => ": a directory is given a key while it is empty",
            Some(libc::EEXIST) => ": the directory is encrypted under another policy",
            _ => "",
        };
        format!("{doing}: {e}{hint}")
    }

    pub fn state(dir: &Path) -> Result<DirState> {
        let d = std::fs::File::open(dir)?;
        Ok(match policy(&d)? {
            None => DirState::Plain,
            // A policy of the first version names its key another way and
            // is none of ours; whether its key is held is the kernel's to
            // say when a file is opened.
            Some(p) if p.version != POLICY_V2 => DirState::Unlocked,
            Some(p) if held(&d, p.identifier)? => DirState::Unlocked,
            Some(_) => DirState::Locked,
        })
    }

    pub fn protect(dir: &Path, key: &DirKey) -> Result<()> {
        let d = std::fs::File::open(dir)?;
        let refuse = |e: Error, doing: &str| super::refused(dir, &why(&e, doing));
        let identifier = add(&d, key).map_err(|e| refuse(e, "the kernel did not take the key"))?;
        let want = ours(identifier);
        // A directory already under this key and this policy is what the
        // request would make of it.
        if policy(&d)? == Some(want) {
            return Ok(());
        }
        let mut arg = want;
        ask(&d, SET_POLICY, &mut arg).map_err(|e| {
            // The key was handed over for nothing: given back, unless a
            // store already open under it holds it too, which the kernel
            // answers by keeping it.
            let _ = remove(&d, identifier);
            refuse(e, "the directory was not encrypted")
        })
    }

    pub fn unlock(dir: &Path, key: &DirKey) -> Result<()> {
        let d = std::fs::File::open(dir)?;
        let Some(has) = policy(&d)? else {
            return Err(super::refused(
                dir,
                "a key was given, and the directory is not encrypted",
            ));
        };
        let identifier = add(&d, key)
            .map_err(|e| super::refused(dir, &why(&e, "the kernel did not take the key")))?;
        if has != ours(identifier) {
            // Not this store's key: handed over, and so given back.
            let _ = remove(&d, identifier);
            return Err(super::refused(
                dir,
                "the directory is encrypted under another key or another policy",
            ));
        }
        Ok(())
    }

    /// The root of the filesystem `dir` is on: the highest directory above
    /// it on the same device, or the nearest to that this process may open.
    ///
    /// A key is given up through a descriptor, and a directory under the
    /// key held open for the asking is a file under the key in use: asked
    /// through the store's own directory, the kernel answered that files
    /// were busy every time, the busy one being the question's. The root
    /// is the kernel's own advice for it.
    fn filesystem_root(dir: &Path) -> Result<std::fs::File> {
        use std::os::unix::fs::MetadataExt;
        let dir = dir.canonicalize()?;
        let dev = dir.metadata()?.dev();
        let mut root = None;
        for up in dir.ancestors().skip(1) {
            match up.metadata() {
                Ok(m) if m.dev() == dev => {
                    if let Ok(f) = std::fs::File::open(up) {
                        root = Some(f);
                    }
                }
                _ => break,
            }
        }
        match root {
            Some(f) => Ok(f),
            // A directory that is itself the root, or under nothing this
            // process may open.
            None => std::fs::File::open(&dir),
        }
    }

    pub fn lock(dir: &Path, key: &DirKey) -> Result<bool> {
        let d = filesystem_root(dir)?;
        // The identifier is the kernel's to derive, so the key is handed
        // over to learn it; a key not held before is held for the moment
        // between the two requests.
        let identifier = add(&d, key)?;
        remove(&d, identifier)
    }
}

#[cfg(not(target_os = "linux"))]
mod sys {
    use super::{DirKey, DirState};
    use std::io::Result;
    use std::path::Path;

    const NONE: &str = "a key was given, and this platform has no directory encryption supdb can \
                        ask for (Linux's, on ext4 or f2fs)";

    pub fn state(_dir: &Path) -> Result<DirState> {
        Ok(DirState::Plain)
    }
    pub fn protect(dir: &Path, _key: &DirKey) -> Result<()> {
        Err(super::refused(dir, NONE))
    }
    pub fn unlock(dir: &Path, _key: &DirKey) -> Result<()> {
        Err(super::refused(dir, NONE))
    }
    pub fn lock(dir: &Path, _key: &DirKey) -> Result<bool> {
        Err(super::refused(dir, NONE))
    }
}

/// What the kernel says of the directory at `dir`.
pub fn state(dir: &Path) -> Result<DirState> {
    sys::state(dir)
}

/// Give the key up: the kernel forgets it, and the store's files, their
/// names with them, are ciphertext to every process until a store is
/// opened under the key again. `true` once it is forgotten; `false` where
/// the kernel still holds it, because a file under it is open -- a store
/// under the key is open, in this process or another -- or another user
/// handed the same key over.
///
/// A store does not do this at its close: the key is one a second store,
/// or a second process, may be open under, and a key given up under them
/// is their next file refused. The caller knows when the last has closed.
pub fn lock(dir: &Path, key: &DirKey) -> Result<bool> {
    sys::lock(dir, key)
}

/// `Db::create`'s half: the directory encrypted under the key, before
/// anything is written into it, or the refusal.
pub(crate) fn protect(dir: &Path, key: &DirKey) -> Result<()> {
    sys::protect(dir, key)
}

/// `Db::open`'s half, with a key: the key handed over and the directory's
/// policy held to it, or the refusal.
pub(crate) fn unlock(dir: &Path, key: &DirKey) -> Result<()> {
    sys::unlock(dir, key)
}

/// `Db::open`'s half, with no key: an encrypted directory whose key the
/// kernel does not hold is refused by name, since its files' names are
/// encrypted and it would otherwise open as a store of nothing.
pub(crate) fn refuse_locked(dir: &Path) -> Result<()> {
    match state(dir)? {
        DirState::Locked => Err(refused(
            dir,
            "the directory is encrypted and its key is not held: open it with \
             `Options::dir_key`",
        )),
        DirState::Plain | DirState::Unlocked => Ok(()),
    }
}
