#!/usr/bin/env python3
"""The memo-size arms in the suite: supdb-cache64tofitautozpagesmemo16 and
...memo32, the combined arm with the memo at 16 and 32 MiB."""
def edit(path, pairs):
    s = open(path).read()
    for old, new in pairs:
        n = s.count(old)
        assert n == 1, (path, n, old[:90])
        s = s.replace(old, new)
    open(path, 'w').write(s)

E = '/home/user/supdb/bench/src/engines.rs'
s = open(E).read()
after_field = '''    zcutseek: bool,
    /// What the reader handles this store gave its threads answered''' if 'zcutseek: bool,\n    /// What the reader handles' in s else '''    zcheckedviews: bool,
    /// What the reader handles this store gave its threads answered'''
assert s.count(after_field) == 1
s = s.replace(after_field, after_field.replace('    /// What the reader handles this store gave its threads answered', '''    /// The combined arm with the memo of decoded groups capped at this many
    /// mebibytes instead of the engine's eight (`Options::zpages_memo_bytes`):
    /// `supdb-cache64tofitautozpagesmemo16`, `...memo32`, each the pair
    /// against `supdb-cache64tofitautozpages`. Zero is the engine's cap.
    zmemo_mib: usize,
    /// What the reader handles this store gave its threads answered'''))
pol_field = '''    zcutseek: bool,
}

/// The conversions a fill makes at most in `supdb-convertcap`''' if 'zcutseek: bool,\n}\n\n/// The conversions' in s else '''    zcheckedviews: bool,
}

/// The conversions a fill makes at most in `supdb-convertcap`'''
assert s.count(pol_field) == 1
s = s.replace(pol_field, pol_field.replace('}\n\n/// The conversions', '''    /// The memo's cap in mebibytes, zero for the engine's
    /// (`Options::zpages_memo_bytes`): `supdb-cache64tofitautozpagesmemo16`,
    /// `...memo32`.
    zmemo_mib: usize,
}

/// The conversions'''))
open(E, 'w').write(s)
edit(E, [
('''            zcheckedviews: false,
''', '''            zcheckedviews: false,
            zmemo_mib: 0,
'''),
('''    pub fn create_cache64_tofit_auto_zpages_checkedviews(path: &Path) -> Res<Supdb> {''',
 '''    /// `supdb-cache64tofitautozpages` with the memo of decoded groups capped
    /// at sixteen or thirty-two mebibytes (`Options::zpages_memo_bytes`)
    /// instead of the engine's eight: the pair against
    /// `supdb-cache64tofitautozpages` counts the decodes the larger memo
    /// saves and prices the page cache it takes under a bound.
    pub fn create_cache64_tofit_auto_zpages_memo(path: &Path, mib: usize) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zmemo_mib: mib,
                ..Policy::default()
            },
        )
    }

    pub fn create_cache64_tofit_auto_zpages_memo16(path: &Path) -> Res<Supdb> {
        Supdb::create_cache64_tofit_auto_zpages_memo(path, 16)
    }

    pub fn create_cache64_tofit_auto_zpages_memo32(path: &Path) -> Res<Supdb> {
        Supdb::create_cache64_tofit_auto_zpages_memo(path, 32)
    }

    pub fn create_cache64_tofit_auto_zpages_checkedviews(path: &Path) -> Res<Supdb> {'''),
('''            zcheckedviews,
        } = policy;''', '''            zcheckedviews,
            zmemo_mib,
        } = policy;'''),
('''            zpages_trusted_views: !zcheckedviews,
''', '''            zpages_trusted_views: !zcheckedviews,
            zpages_memo_bytes: if zmemo_mib > 0 {
                zmemo_mib << 20
            } else {
                supdb::Options::default().zpages_memo_bytes
            },
'''),
('''            zcheckedviews,
            ord_threads: Default::default(),''', '''            zcheckedviews,
            zmemo_mib,
            ord_threads: Default::default(),'''),
('''            } else if self.zcheckedviews {
                "supdb-cache64tofitautozpagescheckedviews"
            } else {''', '''            } else if self.zcheckedviews {
                "supdb-cache64tofitautozpagescheckedviews"
            } else if self.zmemo_mib == 16 {
                "supdb-cache64tofitautozpagesmemo16"
            } else if self.zmemo_mib == 32 {
                "supdb-cache64tofitautozpagesmemo32"
            } else {'''),
('''        | "supdb-cache64tofitautozpagescheckedviews"
''', '''        | "supdb-cache64tofitautozpagescheckedviews"
        | "supdb-cache64tofitautozpagesmemo16"
        | "supdb-cache64tofitautozpagesmemo32"
'''),
('''        "supdb-cache64tofitautozpagescheckedviews" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_checkedviews(dir)?)
        }
''', '''        "supdb-cache64tofitautozpagescheckedviews" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_checkedviews(dir)?)
        }
        "supdb-cache64tofitautozpagesmemo16" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memo16(dir)?)
        }
        "supdb-cache64tofitautozpagesmemo32" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_memo32(dir)?)
        }
'''),
('''        drop(checked);
''', '''        drop(checked);
        assert_eq!(supdb::Options::default().zpages_memo_bytes, 8 << 20);
        assert_eq!(b.zpages_memo_bytes, 8 << 20);
        for (mib, name, make) in [
            (
                16usize,
                "supdb-cache64tofitautozpagesmemo16",
                Supdb::create_cache64_tofit_auto_zpages_memo16 as fn(&Path) -> Res<Supdb>,
            ),
            (
                32,
                "supdb-cache64tofitautozpagesmemo32",
                Supdb::create_cache64_tofit_auto_zpages_memo32,
            ),
        ] {
            let memo = new(name, make);
            assert_eq!(memo.name(), name);
            assert_eq!(guarantee(memo.name()), guarantee(base.name()));
            assert_eq!(memo.features(), base.features());
            let g = memo.db.as_ref().unwrap().options();
            assert_eq!(g.zpages_memo_bytes, mib << 20, "the one option");
            assert_eq!(
                (
                    g.zpages,
                    g.zpages_memo,
                    g.zpages_trusted_views,
                    g.zpages_seek_span,
                    g.scan_cache_bytes
                ),
                (
                    b.zpages,
                    b.zpages_memo,
                    b.zpages_trusted_views,
                    b.zpages_seek_span,
                    b.scan_cache_bytes
                )
            );
            drop(memo);
        }
'''),
])
M = '/home/user/supdb/bench/DESIGN.md'
edit(M, [
('''with the same, the pair against `supdb-nocachezpages`. The forced arm's''',
 '''with the same, the pair against `supdb-nocachezpages`.
`supdb-cache64tofitautozpagesmemo16` and `supdb-cache64tofitautozpagesmemo32`
are the combined arm with the memo of decoded groups capped at sixteen and
thirty-two mebibytes (`Options::zpages_memo_bytes`), where the default caps
it at eight. The forced arm's'''),
])
print("memo size: suite edits applied")
