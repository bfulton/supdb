/// A partition, then a seal of rewrites into a piece over it whose log
/// file retires at the piece's durable landing (`seal_rotates_wal`), and
/// nothing after. With the newest slot damaged, the slot before it names
/// the partition alone, every file in place, and covers less of the log
/// than the piece; the log that held the rewrites is gone, so the open
/// that took the slot before would answer every rewritten key's first
/// value. It refuses.
#[test]
fn a_fallback_past_a_slot_whose_log_retired_is_refused() {
    let d = dir("oo-log");
    let c = dir("oo-log-kill");
    let opts = Options {
        direct_ingest: false,
        seal_rotates_wal: true,
        seal_bytes: 1 << 20,
        l0_trigger: 1000,
        ..Options::default()
    };
    let mut db = Db::create(&d, opts.clone()).unwrap();
    let mut model: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for k in 0..2000u32 {
        db.put(key(k).as_bytes(), b"v0");
        model.insert(key(k).into_bytes(), vec![b"v0".to_vec()]);
    }
    db.commit().unwrap();
    db.flush().unwrap();
    db.settle().unwrap();
    for k in (0..2000u32).step_by(7) {
        db.put(key(k).as_bytes(), b"v1");
        model.insert(key(k).into_bytes(), vec![b"v1".to_vec()]);
    }
    db.commit().unwrap();
    db.seal().unwrap();
    db.settle().unwrap();
    // The store as it stands, nothing in flight: what a kill leaves.
    copy_store(&d, &c);
    drop(db);
    let [newest, older] = oo_by_age(&c);
    let (cov_new, new_names) = oo_names(&std::fs::read(&newest).unwrap());
    let (cov_old, old_names) = oo_names(&std::fs::read(&older).unwrap());
    assert!(
        cov_old < cov_new
            && old_names.iter().all(|(n, _)| c.join(n).exists())
            && new_names.iter().any(|n| !old_names.contains(n)),
        "premise: the older slot covers less, names files in place, and \
         not the piece: {old_names:?} at {cov_old} beside {new_names:?} at {cov_new}"
    );
    for w in wal_names(&c) {
        assert_eq!(
            std::fs::metadata(c.join(&w)).unwrap().len(),
            8,
            "premise: {w} holds nothing, the file the rewrites were in retired"
        );
    }
    let saved = std::fs::read(&newest).unwrap();
    oo_damage(&newest);
    oo_refused(&c, &opts, OO_LOG, "log");
    std::fs::write(&newest, &saved).unwrap();
    let db = Db::open(&c, opts).expect("the store with its slot put back opens");
    oo_holds(&db, &model, "log, repaired");
    drop(db);
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::remove_dir_all(&c);
}

