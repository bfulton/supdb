import sys
p=sys.argv[1]
s=open(p).read()
old="""        let before = settled(&db, "after the fill");
        let started = db.zpage_counts().build_starts;
"""
new="""        // SCRATCH T14: the writer's thread comes late to its reading.
        if let Some(ms) = std::env::var("SUPDB_SCRATCH_T14_WAIT_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        let before = settled(&db, "after the fill");
        let started = db.zpage_counts().build_starts;
        eprintln!("T14 eager {eager} at the reading: evicted {} builds started {started} form files {}", before.evicted_units, db.zpage_form_files().len());
"""
assert s.count(old)==1
open(p,'w').write(s.replace(old,new))
