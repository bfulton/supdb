import sys
p=sys.argv[1]
s=open(p).read()
anchor="""/// The attach of a form file under a cipher decrypts what an attach reads
/// of it -- its trailer, its resident part, the sampled pages -- and not
/// the file: one partition of several megabytes, reopened.
"""
assert s.count(anchor)==1
new='''/// A form file's build rests as it goes: a partition several times its
/// cache is built within the cache and a slack, where a build that read
/// under one hold with no rest held the partition. The partition is one
/// ordered run's, twice the bound and more, and it is built by a store
/// reopened: a file being written is in memory whole until it has its
/// envelope, so a store's peak through its own load is a segment's size
/// whatever its builds do, and a reopened store's peak is its builds'.
#[test]
fn a_form_build_holds_its_cache_and_not_its_partition() {
    let _quiet = one_at_a_time();
    let n = 250_000;
    let cache: u64 = 2 << 20;
    let slack: u64 = 8 << 20;
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(18).cache_bytes(cache)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        // Built by the form's own settle, after the reopen, and not as
        // the partition lands.
        zpages_eager: false,
        ..Options::default()
    };
    let dir = scratch("formrests");
    {
        let mut db = Db::create(&dir, opts()).unwrap();
        for i in 0..n {
            db.append(&k(i), &v(i, 0));
            if i % 500 == 499 {
                db.commit().unwrap();
            }
        }
        db.commit().unwrap();
        db.flush().unwrap();
        db.settle().unwrap();
        assert_eq!(db.zpage_counts().build_starts, 0, "a build before the reopen");
    }
    let largest = names(&dir)
        .iter()
        .filter(|n| n.starts_with("x-") && !n.ends_with(".z"))
        .map(|n| std::fs::metadata(dir.join(n)).unwrap().len())
        .max()
        .unwrap();
    assert!(
        largest > 2 * (cache + slack),
        "the largest segment is {largest} bytes: a build that held it whole would pass the bound"
    );
    let mut db = Db::open(&dir, opts()).unwrap();
    db.zpages_settle().unwrap();
    let z = db.zpage_counts();
    assert!(z.build_starts >= 1 && z.build_failed == 0, "{} builds, {} failed", z.build_starts, z.build_failed);
    let forms = db.zpage_form_files();
    assert!(!forms.is_empty() && forms.iter().all(|f| !f.3), "the form files: {forms:?}");
    let c = db.cipher_counts().unwrap().cache;
    eprintln!("T25 largest {largest} peak {} forms {}", c.peak, forms.len());
    assert!(
        c.peak < cache + slack,
        "the cache held {} bytes of a budget of {cache} through the build of a segment of {largest}: {c:?}",
        c.peak
    );
    for i in (0..n).step_by(997) {
        let mut got: Vec<Vec<u8>> = Vec::new();
        db.read_all(&k(i), vf(&mut |x| got.push(x.to_vec()))).unwrap();
        assert_eq!(got, [v(i, 0)], "key {i}");
    }
    assert!(db.zpage_counts().reads > z.reads, "no read through the form");
}

'''
open(p,'w').write(s.replace(anchor,new+anchor))
