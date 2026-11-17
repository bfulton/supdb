p = '/home/user/supdb/src/db/pager.rs'
s = open(p).read()


def rep(old, new, count=1):
    global s
    assert s.count(old) == count, (old, s.count(old))
    s = s.replace(old, new)


rep("""/// SCRATCH(once): the most chunks one pread reads.""", """/// SCRATCH(once): whether a pager in this process reads past the page
/// cache, so an envelope opened from here on keeps its descriptors: a
/// file the store has unlinked is still read by a snapshot through them,
/// where an open by its path finds nothing.
static ONCE_ANY: AtomicBool = AtomicBool::new(false);

/// SCRATCH(once): the most chunks one pread reads.""")
rep("""            (o, _) => o,
        };""", """            (o, _) => o,
        };
        if once != 0 {
            ONCE_ANY.store(true, Ordering::SeqCst);
        }""")
rep("""        let _ = map.advise(memmap2::Advice::Random);
        Ok(Back {
            aad: hdr.encode(),
            map,
            hdr,
            aead,
            path: path.to_path_buf(),
            direct: OnceLock::new(),
            buffered: OnceLock::new(),
        })""", """        let _ = map.advise(memmap2::Advice::Random);
        let (direct, buffered) = (OnceLock::new(), OnceLock::new());
        if ONCE_ANY.load(Ordering::SeqCst) {
            use std::os::unix::fs::OpenOptionsExt;
            let d = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECT)
                .open(path)
                .or_else(|_| File::open(path))?;
            let _ = direct.set(d);
            let _ = buffered.set(file);
        }
        Ok(Back {
            aad: hdr.encode(),
            map,
            hdr,
            aead,
            path: path.to_path_buf(),
            direct,
            buffered,
        })""")
open(p, 'w').write(s)
print("ok")
