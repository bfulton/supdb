#!/usr/bin/env python3
"""Lever C's unit test: the cursor's skip agrees with the seek a key."""
def edit(path, pairs):
    s = open(path).read()
    for old, new in pairs:
        n = s.count(old)
        assert n == 1, (path, n, old[:90])
        s = s.replace(old, new)
    open(path, 'w').write(s)

Z = '/home/user/supdb/src/db/zfile.rs'
edit(Z, [
('''    #[test]
    fn a_form_file_of_another_segment_is_refused() {''',
 '''    /// A cursor moved forward key by key (`ZCursor::skip_until`) cuts a
    /// batch of keys in key order where a seek a key (`ZFile::cut_in`) cuts
    /// each: every record's key, keys between records, below the first and
    /// past the last, over the suite's shape and over every record shape;
    /// and it opens no group twice where the seeks open one a key.
    #[test]
    fn the_cut_cursor_agrees_with_the_seek_a_key() {
        for (name, recs, inline_max) in [
            ("cutcursor-suite", suite(3000, 41), 256),
            ("cutcursor-shapes", shapes(), 1 << 20),
        ] {
            let fx = Fixture::new(name, recs, inline_max);
            let zfile = fx.zfile();
            let nkeys = fx.recs.len();
            let mut probes: Vec<Vec<u8>> = vec![Vec::new(), b"\\x00".to_vec()];
            for r in &fx.recs {
                let k = &r.0;
                probes.push(k.clone());
                let mut above = k.clone();
                above.push(0);
                probes.push(above);
                if let Some(last) = k.last() {
                    let mut below = k.clone();
                    *below.last_mut().unwrap() = last.wrapping_sub(1);
                    probes.push(below);
                }
            }
            probes.push(vec![0xff; 20]);
            probes.sort();
            probes.dedup();
            // The seeks, each from the rank the one before reached.
            let mut want = Vec::with_capacity(probes.len());
            let mut r = 0usize;
            for k in &probes {
                match zfile.cut_in(&fx.memo, r, nkeys, k) {
                    ZSeek::Served { rank, at, .. } => {
                        want.push((rank, at));
                        r = rank;
                    }
                    other => panic!("{other:?}"),
                }
            }
            fx.memo.take_counts();
            let mut cur = ZCursor::at(&fx.memo, &zfile, &fx.blob, 0);
            let got: Vec<(usize, KeyOrder)> = probes
                .iter()
                .map(|k| {
                    let at = cur.skip_until(k);
                    (cur.rank().min(nkeys), at)
                })
                .collect();
            assert!(!cur.fell_back());
            let decoded = cur.decoded;
            drop(cur);
            assert_eq!(got, want, "{name}");
            assert_eq!(want.last().map(|w| w.0), Some(nkeys), "the last probe is past every key");
            let (hits, misses) = fx.memo.take_counts();
            assert_eq!(misses as u64, decoded, "a miss a decode");
            let groups: usize = (0..zfile.pages())
                .map(|p| page_view(zfile.page_bytes(p).unwrap()).unwrap().groups())
                .sum();
            assert!(
                (hits + misses) as usize <= groups,
                "{name}: one open a group crossed at most, {} opens over {groups} groups",
                hits + misses
            );
        }
    }

    #[test]
    fn a_form_file_of_another_segment_is_refused() {'''),
])
print("lever C test applied")
