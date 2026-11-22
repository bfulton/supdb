p='/home/user/supdb/tests/cipher.rs'
s=open(p).read()
anchor="""/// The attach of a form file under a cipher decrypts what an attach reads
/// of it -- its trailer, its resident part, the sampled pages -- and not
/// the file: one partition of several megabytes, reopened.
"""
assert s.count(anchor)==1
new='''#[test]
fn a_form_build_holds_its_cache_and_not_its_partition() {
    let _quiet = one_at_a_time();
    let n = 200_000;
    let cache: u64 = 2 << 20;
    let opts = || Options {
        encryption: Encryption::Cipher(cipher(18).cache_bytes(cache)),
        zpages: supdb::ZPages::Always,
        zpages_min_bytes: 0,
        zpages_eager: false,
        seal_bytes: 4 << 20,
        partition_bytes: Some(64 << 20),
        l0_trigger: 2,
        ..Options::default()
    };
    let dir = scratch("formrests");
    {
        let mut db = Db::create(&dir, opts()).unwrap();
        fill(&mut db, n);
        eprintln!("T25 after fill: builds started {} cache {:?}", db.zpage_counts().build_starts, db.cipher_counts().unwrap().cache);
    }
    let sizes = |z: bool| -> Vec<u64> {
        let mut v: Vec<u64> = names(&dir)
            .iter()
            .filter(|n| n.starts_with("x-") && n.ends_with(".z") == z)
            .map(|n| std::fs::metadata(dir.join(n)).unwrap().len())
            .collect();
        v.sort_unstable();
        v
    };
    eprintln!("T25 envelopes before the builds: {:?}", sizes(false));
    let mut db = Db::open(&dir, opts()).unwrap();
    let opened = db.cipher_counts().unwrap().cache;
    db.zpages_settle().unwrap();
    let c = db.cipher_counts().unwrap().cache;
    eprintln!("T25 opened peak {} held {} | after builds peak {} held {} evicted {} | forms {:?} | builds {}", opened.peak, opened.held, c.peak, c.held, c.evicted_units, sizes(true), db.zpage_counts().build_starts);
    holds(&db, n);
}

'''
open(p,'w').write(s.replace(anchor,new+anchor))
