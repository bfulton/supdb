p = '/home/user/supdb/src/db/vault.rs'
s = open(p).read()


def rep(old, new, count=1):
    global s
    assert s.count(old) == count, (old, s.count(old))
    s = s.replace(old, new)


rep("""    cache: Option<u64>,
}""", """    cache: Option<u64>,
    // SCRATCH(once): `Pager::new_with`'s miss path and budget mode.
    once: u8,
    dyn_pct: u8,
}""")
rep("""            cache: None,
        }""", """            cache: None,
            once: 0,
            dyn_pct: 0,
        }""")
rep("""    /// The caller's name for its key, written""", """    /// SCRATCH(once): a miss read past the page cache (1) or after a
    /// RWF_NOWAIT probe (2).
    pub fn scratch_once(mut self, mode: u8) -> StoreCipher {
        self.once = mode;
        self
    }

    /// SCRATCH(once): the budget recomputed as `pct` of the read budget.
    pub fn scratch_dynamic(mut self, pct: u8) -> StoreCipher {
        self.dyn_pct = pct;
        self
    }

    /// The caller's name for its key, written""")
rep("""            pager: Pager::new(cipher.cache.unwrap_or_else(default_cache)),""",
    """            pager: Pager::new_with(
                cipher.cache.unwrap_or_else(default_cache),
                cipher.once,
                cipher.dyn_pct,
            ),""")
open(p, 'w').write(s)

p = '/home/user/supdb/bench/src/engines.rs'
s = open(p).read()
rep("""    /// `Policy::cipher_cache`.
    cipher_cache: u64,
""", """    /// `Policy::cipher_cache`.
    cipher_cache: u64,
    /// SCRATCH(once)
    cipher_once: u8,
    cipher_dyn: u8,
""")
rep("""    cipher_cache: u64,
    /// The store under a cipher with ordered ingest off""", """    cipher_cache: u64,
    /// SCRATCH(once): `StoreCipher::scratch_once` and `scratch_dynamic`.
    cipher_once: u8,
    cipher_dyn: u8,
    /// The store under a cipher with ordered ingest off""")
rep("""            cipher_cache: 0,
            cipher_log: false,""", """            cipher_cache: 0,
            cipher_once: 0,
            cipher_dyn: 0,
            cipher_log: false,""")
arms = [
    ("b384", 384, 0, 0), ("b432", 432, 0, 0),
    ("d320", 320, 1, 0), ("d384", 384, 1, 0), ("d432", 432, 1, 0),
    ("ddyn", 0, 1, 75), ("dndyn", 0, 2, 75),
]
fns = ""
for name, mib, once, dyn_ in arms:
    fns += f"""
    /// SCRATCH(once): `supdb-cipher{name}`.
    pub fn create_cipher_{name}(path: &Path) -> Res<Supdb> {{
        Supdb::with_policy(
            path,
            Policy {{
                cipher: true,
                cipher_cache: {mib} << 20,
                cipher_once: {once},
                cipher_dyn: {dyn_},
                ..Policy::default()
            }},
        )
    }}
"""
rep("""    /// `supdb-cipher` with a cache of sixty-four mebibytes
    /// (`Policy::cipher_cache`).""", fns.lstrip("\n") + """
    /// `supdb-cipher` with a cache of sixty-four mebibytes
    /// (`Policy::cipher_cache`).""")
rep("""            cipher_auto,
            cipher_cache,
            cipher_log,
            partition,""", """            cipher_auto,
            cipher_cache,
            cipher_once,
            cipher_dyn,
            cipher_log,
            partition,""", 2)
rep("""                let c = supdb::StoreCipher::new(Gcm::new(), Gcm::of);
                supdb::Encryption::Cipher(match cipher_cache {""", """                let c = supdb::StoreCipher::new(Gcm::new(), Gcm::of)
                    .scratch_once(cipher_once)
                    .scratch_dynamic(cipher_dyn);
                supdb::Encryption::Cipher(match cipher_cache {""")
names = "".join(
    f"""                true if self.cipher_once == {once} && self.cipher_dyn == {dyn_} && self.cipher_cache == {mib} << 20 => "supdb-cipher{name}",\n"""
    for name, mib, once, dyn_ in arms)
rep("""                true if self.cipher_cache != 0 => "supdb-cipher64",""",
    names + """                true if self.cipher_cache != 0 => "supdb-cipher64",""")
rep("""        | "supdb-cipher64"
        | "supdb-cipherlog\"""", """        | "supdb-cipher64"
""" + "".join(f"""        | "supdb-cipher{n}"\n""" for n, *_ in arms) + """        | "supdb-cipherlog\"""")
rep("""        "supdb-cipher64" => supdb_arm(Supdb::create_cipher64, dir)?,""",
    """        "supdb-cipher64" => supdb_arm(Supdb::create_cipher64, dir)?,\n""" + "".join(
        f"""        "supdb-cipher{n}" => supdb_arm(Supdb::create_cipher_{n}, dir)?,\n""" for n, *_ in arms).rstrip("\n"))
rep("""            ("cipher_evictor_passes", pc.passes as f64),""", """            ("cipher_evictor_passes", pc.passes as f64),
            ("cipher_direct_reads", pc.direct_reads as f64),
            ("cipher_direct_bytes", pc.direct_bytes as f64),
            ("cipher_direct_us", pc.direct_us as f64),
            ("cipher_run_chunks", pc.run_chunks as f64),
            ("cipher_nowait_hits", pc.nowait_hits as f64),
            ("cipher_limit_lo", pc.limit_lo as f64),
            ("cipher_limit_hi", pc.limit_hi as f64),""")
open(p, 'w').write(s)
print("ok")
