import sys
p=sys.argv[1]
s=open(p).read()
def rep(old,new):
    global s
    assert s.count(old)==1,(s.count(old),old[:60])
    s=s.replace(old,new)
rep("""#[test]
fn the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit() {
    let _quiet = one_at_a_time();
    let n = 150_000;
    let cache: u64 = 2 << 20;
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(12).cache_bytes(cache)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        ..small()
    };
    let dir = scratch("formlarger");
""","""#[test]
fn the_compact_form_is_built_and_read_within_a_cache_the_store_does_not_fit() {
    let _quiet = one_at_a_time();
    // Twice. With the builds left to the form's own settle, every one of
    // them falls between the two readings of the cache, and what they gave
    // back is counted exactly. With the builds begun as the partitions land
    // (the default), they run beside the fill's seals and merges, and how
    // many are left for the settle is the timing's: a third had run by the
    // first reading in every run looked at, and all of them in one run of a
    // gate's stride group, which read "the builds gave nothing back" off a
    // cache that had given back 690 units before that reading.
    for eager in [false, true] {
        a_form_larger_than_its_cache(eager);
    }
}

fn a_form_larger_than_its_cache(eager: bool) {
    let n = 150_000;
    let cache: u64 = 2 << 20;
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(12).cache_bytes(cache)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        zpages_eager: eager,
        ..small()
    };
    let dir = scratch(if eager { "formlarger-eager" } else { "formlarger" });
""")
rep("""        let before = settled(&db, "after the fill");
        db.zpages_settle().unwrap();
        let forms = db.zpage_form_files();
""","""        let before = settled(&db, "after the fill");
        let started = db.zpage_counts().build_starts;
        db.zpages_settle().unwrap();
        let forms = db.zpage_form_files();
""")
rep("""        // builds hold the store.
        assert!(
            c.evicted_units > before.evicted_units,
            "the builds gave nothing back: {c:?}"
        );
""","""        // builds hold the store.
        if eager {
            // Begun at the first landing: held to having been made, and the
            // cache to having turned over since the store's birth.
            assert!(
                z.builds >= forms.len() as u64,
                "{} builds for {} form files",
                z.builds,
                forms.len()
            );
            assert!(c.evicted_units > 0, "the cache gave nothing back: {c:?}");
        } else {
            assert_eq!(
                started, 0,
                "a build before the form's settle, with the builds left to it"
            );
            assert!(
                c.evicted_units > before.evicted_units,
                "the builds gave nothing back: {c:?}"
            );
        }
""")
open(p,'w').write(s)
