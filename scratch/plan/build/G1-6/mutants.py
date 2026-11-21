#!/usr/bin/env python3
# mutants.py [NAME...]: each mutant applied to the tree, its tests run, the
# verdict written to results.txt as it comes, the file put back from keep/
# and checked by its sum after each. A mutant is killed when its tests exit
# nonzero (or do not compile); it survives when they pass.
import hashlib, os, subprocess, sys, time
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-6'
T = '/home/user/supdb'
TMP = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6-mut'
LIB = 'cargo test --profile checked --lib -- '
ZP = 'cargo test --profile checked --test zpages -- '
M = [
 ('M1 paged arm of resident_share answers 0', 'src/bytes.rs',
  '        Some(p.resident(off, len))\n    }', '        let _ = p;\n        Some(0.0)\n    }',
  [LIB + 'a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident the_heads_resident_are_asked']),
 ('M2 the warm touches a paged mapping', 'src/db/zfile.rs',
  'let Some(b) = map.lend_at(at as u64, 1) else {', 'let Some(b) = map.slice_at(at as u64, 1) else {',
  [ZP + '--exact a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing']),
 ('M3 z_cross takes the paged branch for a mapped file', 'src/db.rs',
  '                let Some((at, len)) = s.records_by_descriptor() else {\n                    // A file of a store under a cipher: the same swap in',
  '                let Some((at, len)) = None::<(u64, u64)> else {\n                    // A file of a store under a cipher: the same swap in',
  [ZP + '--exact builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records']),
 ('M4 the region place off by index_offset (swap_ahead and every place)', 'src/blob.rs',
  '                Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))',
  '                Some((recs.start_in(sec) as u64, recs.size()))',
  ['cargo test --profile checked --test blob -- the_advice_by_rank', ZP + '--exact builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records']),
 ('O1 heads_all_resident true on any answer', 'src/ordindex.rs',
  'self.map.resident_share(HEADER as u64, self.n * HEAD) == Some(1.0)',
  'self.map.resident_share(HEADER as u64, self.n * HEAD).is_some()',
  [LIB + 'the_heads_resident_are_asked']),
 ('O2 the warm skips a chunk with any page resident', 'src/db/zfile.rs',
  '        if share >= 1.0 {\n            w.skipped += span as u64;',
  '        if share > 0.0 {\n            w.skipped += span as u64;',
  [LIB + 'a_warm_reads_the_cold_file_once', ZP + '--exact an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off']),
 ('O3 the warm counts a chunk whole whatever was resident', 'src/db/zfile.rs',
  '            w.bytes += ((pages - resident) * page).min(span) as u64;',
  '            let _ = resident;\n            w.bytes += (pages * page).min(span) as u64;',
  [LIB + 'a_warm_reads_the_cold_file_once', ZP + '--exact an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off']),
 ('O4 the mapped arm of resident_share ignores its offset', 'src/bytes.rs',
  'let (have, pages) = self.mapped_resident(usize::try_from(off).ok()?, len)?;',
  'let (have, pages) = self.mapped_resident(0 * usize::try_from(off).ok()?, len)?;',
  [LIB + 'residency_counts a_paged_mapping_and_a_mapped_file']),
 ('O5 the merge read-ahead takes a whole reader only (the base)', 'src/db.rs',
  '                b.record_region_place()\n                    .map_or((0, 0), |(at, len)| (at, at.saturating_add(len as u64)))',
  '                b.record_regions()\n                    .and(b.record_region_place())\n                    .map_or((0, 0), |(at, len)| (at, at.saturating_add(len as u64)))',
  ['cargo test --profile checked --test cipher -- --exact a_merge_under_a_cipher_asks_ahead_for_its_cold_inputs']),
 ('O6 the build drops behind it without its mapping\'s pages', 'src/db/zfile.rs',
  '    map.advise_dontneed_range(a as u64, b - a);\n    // SAFETY: an advice over a file descriptor the build holds open',
  '    let _ = map;\n    // SAFETY: an advice over a file descriptor the build holds open',
  [ZP + '--exact the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise']),
 ('O7 records_by_descriptor answers for a reader by the piece', 'src/db.rs',
  '        raw.record_regions()?;\n        raw.record_region_place().map(|(at, len)| (at, len as u64))',
  '        raw.record_region_place().map(|(at, len)| (at, len as u64))',
  [ZP + 'cipher']),
 ('O8 the dir place is the record region\'s', 'src/blob.rs',
  '                let (_, dir) = idx.regions(sec)?;\n                Some((self.index_offset() + dir.start_in(sec) as u64, dir.size()))',
  '                let (dir, _) = idx.regions(sec)?;\n                Some((self.index_offset() + dir.start_in(sec) as u64, dir.size()))',
  [LIB + 'whatever_the_piece gives_back_what_it_read']),
 ('O9 raw_read_span answers none for a reader by the piece (the base)', 'src/blob.rs',
  '        let Index::Flat { idx, .. } = &self.index else {\n            return None;\n        };\n        let (h, r) = idx.population_spans();',
  '        let (_, idx) = self.flat()?;\n        let (h, r) = idx.population_spans();',
  [ZP + 'cipher', 'cargo test --profile checked --test blob -- the_advice_by_rank']),
]
def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()
sums = {}
for line in open(B + '/keep/SHA256SUMS'):
    h, f = line.split()
    sums[f[2:]] = h
want = set(sys.argv[1:])
res = open(B + '/mutants-results.txt', 'a')
for name, f, old, new, tests in M:
    key = name.split()[0]
    if want and key not in want:
        continue
    path = os.path.join(T, f)
    assert sha(path) == sums[f], f'{f} is not the kept copy before {key}'
    src = open(path).read()
    assert src.count(old) == 1, f'{key}: the old text occurs {src.count(old)} times'
    open(path, 'w').write(src.replace(old, new))
    verdict = 'SURVIVED'
    log = f'{B}/mut-{key}.log'
    with open(log, 'w') as lf:
        env = dict(os.environ, TMPDIR=TMP)
        os.makedirs(TMP, exist_ok=True)
        for t in tests:
            lf.write(f'$ {t}\n'); lf.flush()
            r = subprocess.run(t, shell=True, cwd=T, stdout=lf, stderr=subprocess.STDOUT, env=env)
            lf.write(f'rc {r.returncode}\n'); lf.flush()
            if r.returncode != 0:
                verdict = f'KILLED by `{t}` (rc {r.returncode})'
                break
    subprocess.run(['cp', f'{B}/keep/{f}', path])
    assert sha(path) == sums[f], f'{f} not put back after {key}'
    subprocess.run(['rm', '-rf', '--', TMP])
    res.write(f'{time.strftime("%H:%M:%S")} {name}: {verdict}\n'); res.flush()
res.write('EXIT 0\n')
