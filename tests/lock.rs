//! The store's lock beside a forked child of the process that holds it.
//!
//! Its own test binary because the test forks on purpose and holds the
//! child before its exec: for that long the child has a copy of every
//! descriptor this process has open, which in a binary of other tests is
//! every store of theirs.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use supdb::Db;

mod common;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("supdb-next-lock-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A store closed while a forked child of this process still holds a copy
/// of its `LOCK`'s descriptor opens again at once, and the open that took
/// it excludes a second as any open does.
///
/// A `flock` is the open file description's, and a child has a copy of the
/// descriptor from its fork to its exec, or for its life where it never
/// execs: a close that only closed the descriptor left the lock held
/// through the child's copy, and the reopen was refused as a second open.
/// It was met as a race, the crash oracle's spawns beside other tests'
/// stores in `tests/runlog.rs`, once on a hosted runner and nine runs of
/// that binary in thirty with the child held 30 ms before its exec. Here
/// the window is built: the child stops in `pre_exec`, says so through one
/// pipe and waits on another, and the store is closed and opened while it
/// waits. To the kernel a child held there is a child that never execs.
///
/// Run in the clear and under a cipher (`Encryption::Cipher`): the lock is
/// a file of the store's own directory either way (`vault::open_lock`),
/// and a store under a cipher holds more of its directory open across a
/// close -- its marker, its envelopes and their memory files -- for a
/// child's copy to keep.
#[test]
fn a_store_closed_while_a_forked_child_holds_its_lock_opens_again() {
    for cipher in [false, true] {
        closed_beside_a_forked_child(cipher);
    }
}

fn opts(cipher: bool) -> supdb::Options {
    let mut o = common::options();
    if cipher {
        o.encryption = supdb::Encryption::Cipher(supdb::StoreCipher::new(
            common::Gcm::new(&[7; 32]),
            common::Gcm::new,
        ));
    }
    o
}

fn closed_beside_a_forked_child(cipher: bool) {
    let d = dir(&format!("fork-window-{cipher}"));
    let mut db = Db::create(&d, opts(cipher)).unwrap();
    db.append(b"k", b"v");
    db.commit().unwrap();

    let (mut forked_r, forked_w) = std::io::pipe().unwrap();
    let (go_r, mut go_w) = std::io::pipe().unwrap();
    let (forked_fd, go_fd, go_w_fd) = (forked_w.as_raw_fd(), go_r.as_raw_fd(), go_w.as_raw_fd());
    // This binary run on no test: a process that starts and ends.
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "no test has this name"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the closure runs in the forked child and makes three system
    // calls on descriptors the fork copied, a close, a write and a read,
    // each async-signal-safe; it allocates nothing and takes no lock.
    unsafe {
        cmd.pre_exec(move || {
            // The child's own copy of the end it is let go through, closed:
            // with it open the read below never sees that end closed, and a
            // test that failed before letting the child go left it waiting
            // for good, a process for every such run.
            libc::close(go_w_fd);
            let mut b = [1u8];
            libc::write(forked_fd, b.as_ptr().cast(), 1);
            while libc::read(go_fd, b.as_mut_ptr().cast(), 1) < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
            }
            Ok(())
        });
    }
    // `spawn` returns once the child has made its exec, so it runs on a
    // thread of its own while the child is held short of that.
    let spawning = std::thread::spawn(move || cmd.spawn());
    // The child's word that it has forked, or the spawn's end without it:
    // a spawn that failed short of the child's write must not hang this.
    let mut said = libc::pollfd {
        fd: forked_r.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: a poll of one descriptor this test holds open.
    while unsafe { libc::poll(&mut said, 1, 10) } < 1 {
        assert!(
            !spawning.is_finished(),
            "the spawn ended before the child reached its pre_exec"
        );
    }
    let mut b = [0u8];
    forked_r.read_exact(&mut b).unwrap();

    // The child is between its fork and its exec, with its copy of the
    // lock's descriptor open.
    db.close().unwrap();
    let reopened = Db::open(&d, opts(cipher));
    let second = reopened
        .as_ref()
        .ok()
        .map(|_| Db::open(&d, opts(cipher)).err().map(|e| e.kind()));

    // The child let go whatever the opens said, so a failure leaves none.
    go_w.write_all(&[1]).unwrap();
    let mut child = spawning.join().unwrap().unwrap();
    child.wait().unwrap();

    let mut db = reopened.expect("the reopen of a store closed beside a forked child");
    assert_eq!(db.count(b"k").unwrap(), 1);
    // A second open in this process is refused: by the lock in the clear,
    // and under a cipher by the store's registry of its open vaults, which
    // the open asks before it reaches the lock.
    let refused = if cipher {
        std::io::ErrorKind::AlreadyExists
    } else {
        std::io::ErrorKind::WouldBlock
    };
    assert_eq!(
        second,
        Some(Some(refused)),
        "a second open beside the reopened store, the child's stale copy still open"
    );
    db.append(b"k2", b"v");
    db.commit().unwrap();
    db.close().unwrap();
    let _ = std::fs::remove_dir_all(&d);
}
