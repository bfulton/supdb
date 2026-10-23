#!/usr/bin/env python3
"""Lever C, the suite's arm and the tests' hooks (applied with apply.py)."""
def edit(path, pairs):
    s = open(path).read()
    for old, new in pairs:
        n = s.count(old)
        assert n == 1, (path, n, old[:90])
        s = s.replace(old, new)
    open(path, 'w').write(s)

E = '/home/user/supdb/bench/src/engines.rs'
edit(E, [
('''    /// `supdb-cache64tofitautozpagescheckedviews`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zcheckedviews: bool,
    /// What the reader handles this store gave its threads answered''',
 '''    /// `supdb-cache64tofitautozpagescheckedviews`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zcheckedviews: bool,
    /// The combined arm with a block's snapshot keys cut by a seek a key
    /// (`Options::zpages_cut_cursor` off):
    /// `supdb-cache64tofitautozpagescutseek`, the pair against
    /// `supdb-cache64tofitautozpages`.
    zcutseek: bool,
    /// What the reader handles this store gave its threads answered'''),
('''    /// `supdb-cache64tofitautozpagescheckedviews`.
    zcheckedviews: bool,
}

/// The conversions a fill makes at most in `supdb-convertcap`''',
 '''    /// `supdb-cache64tofitautozpagescheckedviews`.
    zcheckedviews: bool,
    /// A block's snapshot keys cut by a seek a key (`Options::zpages_cut_cursor`
    /// off): `supdb-cache64tofitautozpagescutseek`.
    zcutseek: bool,
}

/// The conversions a fill makes at most in `supdb-convertcap`'''),
('''            zexactcut: false,
            zcheckedviews: false,
''',
 '''            zexactcut: false,
            zcheckedviews: false,
            zcutseek: false,
'''),
('''    pub fn create_cache64_tofit_auto_zpages_checkedviews(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zcheckedviews: true,
                ..Policy::default()
            },
        )
    }
''',
 '''    pub fn create_cache64_tofit_auto_zpages_checkedviews(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zcheckedviews: true,
                ..Policy::default()
            },
        )
    }

    /// `supdb-cache64tofitautozpages` with a block's snapshot keys cut
    /// against its partition by a seek a key (`Options::zpages_cut_cursor`
    /// off), the shape before: the pair against
    /// `supdb-cache64tofitautozpages` prices the cursor moved forward key
    /// by key.
    pub fn create_cache64_tofit_auto_zpages_cutseek(path: &Path) -> Res<Supdb> {
        Supdb::with_policy(
            path,
            Policy {
                budget: 64 << 20,
                tofit: true,
                zpages_auto: true,
                zcutseek: true,
                ..Policy::default()
            },
        )
    }
'''),
('''            zexactcut,
            zcheckedviews,
        } = policy;''',
 '''            zexactcut,
            zcheckedviews,
            zcutseek,
        } = policy;'''),
('''            zpages_trusted_views: !zcheckedviews,
''',
 '''            zpages_trusted_views: !zcheckedviews,
            zpages_cut_cursor: !zcutseek,
'''),
('''            zexactcut,
            zcheckedviews,
            ord_threads: Default::default(),''',
 '''            zexactcut,
            zcheckedviews,
            zcutseek,
            ord_threads: Default::default(),'''),
('''            } else if self.zcheckedviews {
                "supdb-cache64tofitautozpagescheckedviews"
            } else {''',
 '''            } else if self.zcheckedviews {
                "supdb-cache64tofitautozpagescheckedviews"
            } else if self.zcutseek {
                "supdb-cache64tofitautozpagescutseek"
            } else {'''),
('''        | "supdb-cache64tofitautozpagescheckedviews"
''',
 '''        | "supdb-cache64tofitautozpagescheckedviews"
        | "supdb-cache64tofitautozpagescutseek"
'''),
('''        "supdb-cache64tofitautozpagescheckedviews" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_checkedviews(dir)?)
        }
''',
 '''        "supdb-cache64tofitautozpagescheckedviews" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_checkedviews(dir)?)
        }
        "supdb-cache64tofitautozpagescutseek" => {
            Box::new(Supdb::create_cache64_tofit_auto_zpages_cutseek(dir)?)
        }
'''),
('''        drop(checked);
        let pre = new(
            "supdb-cache64tofitautozpagesprebuild",''',
 '''        drop(checked);
        assert!(supdb::Options::default().zpages_cut_cursor && b.zpages_cut_cursor);
        let cutseek = new(
            "supdb-cache64tofitautozpagescutseek",
            Supdb::create_cache64_tofit_auto_zpages_cutseek,
        );
        assert_eq!(cutseek.name(), "supdb-cache64tofitautozpagescutseek");
        assert_eq!(guarantee(cutseek.name()), guarantee(base.name()));
        assert_eq!(cutseek.features(), base.features());
        let g = cutseek.db.as_ref().unwrap().options();
        assert!(!g.zpages_cut_cursor, "the one option");
        assert_eq!(
            (
                g.zpages,
                g.zpages_trusted_views,
                g.zpages_settle_span,
                g.zpages_seek_span,
                g.scan_cache_bytes
            ),
            (
                b.zpages,
                b.zpages_trusted_views,
                b.zpages_settle_span,
                b.zpages_seek_span,
                b.scan_cache_bytes
            )
        );
        drop(cutseek);
        let pre = new(
            "supdb-cache64tofitautozpagesprebuild",'''),
])

M = '/home/user/supdb/bench/DESIGN.md'
edit(M, [
('''reads a verified page's directory trusted and asks the memo for its group
before any view. The forced arm's''',
 '''reads a verified page's directory trusted and asks the memo for its group
before any view. `supdb-cache64tofitautozpagescutseek` is the combined arm
with a block's snapshot keys cut against its partition by a seek a key
(`Options::zpages_cut_cursor` off), where the default moves one cursor
through the form file forward key by key. The forced arm's'''),
])

T = '/home/user/supdb/tests/zpages.rs'
edit(T, [
('''#[test]
fn scans_with_every_page_view_checked_agree_with_the_model_over_every_window() {''',
 '''#[test]
fn scans_with_a_blocks_cuts_sought_a_key_at_a_time_agree_with_the_model_over_every_window() {
    // The block cache with a block's snapshot keys cut by a seek a key
    // (`Options::zpages_cut_cursor` off), the shape before the cuts were
    // taken by one cursor moved forward: both shapes answer alike.
    windows_over_with("windows-cache-cutseek", true, false, false, true, true, false);
}

#[test]
fn scans_with_every_page_view_checked_agree_with_the_model_over_every_window() {'''),
('''fn windows_over(name: &str, cache: bool, promote: bool, span: bool, lazy: bool) {
    windows_over_with(name, cache, promote, span, lazy, true);
}

fn windows_over_with(
    name: &str,
    cache: bool,
    promote: bool,
    span: bool,
    lazy: bool,
    trusted: bool,
) {''',
 '''fn windows_over(name: &str, cache: bool, promote: bool, span: bool, lazy: bool) {
    windows_over_with(name, cache, promote, span, lazy, true, true);
}

#[allow(clippy::too_many_arguments)]
fn windows_over_with(
    name: &str,
    cache: bool,
    promote: bool,
    span: bool,
    lazy: bool,
    trusted: bool,
    cut_cursor: bool,
) {'''),
('''    windows_over_with("windows-cache-checkedviews", true, false, false, true, false);''',
 '''    windows_over_with("windows-cache-checkedviews", true, false, false, true, false, true);'''),
('''            o.zpages_trusted_views = trusted;
            if span {''',
 '''            o.zpages_trusted_views = trusted;
            o.zpages_cut_cursor = cut_cursor;
            if span {'''),
])

F = '/home/user/supdb/tests/fuzz.rs'
edit(F, [
('''    trusted: bool,
}

/// Whether a seed runs with the block cache on.''',
 '''    trusted: bool,
    /// A block's snapshot keys cut by one cursor (`Options::zpages_cut_cursor`);
    /// off in some seeds under `ZFUZZ_OPTVARY`.
    cut_cursor: bool,
}

/// Whether a seed runs with the block cache on.'''),
('''        trusted: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
    }
}''',
 '''        trusted: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
        cut_cursor: !(std::env::var("ZFUZZ_OPTVARY").is_ok() && rng.chance(15)),
    }
}'''),
('''        zpages_trusted_views: c.trusted,
        ..Options::default()
    };''',
 '''        zpages_trusted_views: c.trusted,
        zpages_cut_cursor: c.cut_cursor,
        ..Options::default()
    };'''),
('''//! - `ZFUZZ_OPTVARY`: set, a seed turns compaction, promotion or the trusted
//!   page views of the compact form off, each with a chance of 15%.''',
 '''//! - `ZFUZZ_OPTVARY`: set, a seed turns compaction, promotion, the trusted
//!   page views of the compact form or its cut cursor off, each with a
//!   chance of 15%.'''),
])
print("lever C suite and test edits applied")
