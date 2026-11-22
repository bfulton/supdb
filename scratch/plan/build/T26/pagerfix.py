p='/home/user/supdb/src/db/pager.rs'
s=open(p).read()
old="""        let t = std::time::Instant::now();
        while m.resident_at(2_500_000) != Some(true) {
            assert!(t.elapsed().as_secs() < 20, "the range did not come back");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(m.resident_share(0, len).map(|s| s > 0.99), Some(true));
"""
new="""        // The ask is advice and its read is on the kernel's time: the page
        // in the middle of what was asked is back before the last of it, so
        // the whole file's share is waited for as that page is. Read once
        // behind the page alone, the share was short in one run of six.
        let t = std::time::Instant::now();
        while m.resident_at(2_500_000) != Some(true)
            || m.resident_share(0, len).map(|s| s > 0.99) != Some(true)
        {
            assert!(t.elapsed().as_secs() < 20, "the range did not come back");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
"""
assert s.count(old)==1, s.count(old)
open(p,'w').write(s.replace(old,new))
