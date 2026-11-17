# count: instructions by function, 3df85d22 (base engine) vs HEAD 8b83dc03, HEAD's bench/examples both sides
Step log (newest last):
- 1 notes written; tree clean at HEAD 8b83dc03
- 2 run script count/run_counts.sh written; HEAD examples building (build-head.log)
- 3 HEAD built; running head counts (head.status)
- 4 head counts done; base src checked out (git checkout 3df85d22 -- src); building base. RESTORE: git checkout HEAD -- src
- 5 base built (examples build against base, no seam missing); running base counts (base.status)
- 6 base counts done; HEAD src restored, status empty
- 7 (reader) all ten probes exit 0 (head.status, base.status). Totals: load_durable 340,385,259 -> 340,560,651; load_buffered 339,945,359 -> 340,212,629; work 705,249,609 -> 703,741,850; read 166,114,879 -> 166,115,088; merge 1,681,203,430 -> 1,681,782,260. Writing reading.md.
- 8 reading.md started (control + method); next: load_durable rows
- 9 all probes read; writing the table and reading into reading.md
- 10 reading.md complete
