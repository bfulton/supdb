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
CI = 'cargo test --profile checked --test cipher'
ZPA = 'cargo test --profile checked --test zpages'
M = [
 ('R1 the warm asks nothing', 'src/db/zfile.rs',
  '            map.advise_willneed(off as u64, span);\n', '',
  [ZP + '--exact a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing']),
 ('R2 heads_all_resident asked from offset 0', 'src/ordindex.rs',
  'self.map.resident_share(HEADER as u64, self.n * HEAD) == Some(1.0)',
  'self.map.resident_share(0, self.n * HEAD) == Some(1.0)',
  [LIB + 'the_heads_resident_are_asked']),
 ('R3 ask_file opens by path past the door', 'src/db/zfile.rs',
  '    let Ok(f) = super::vault::open(form) else {\n        return 0;\n    };\n    let n = f.metadata()',
  '    let Ok(f) = std::fs::File::open(form) else {\n        return 0;\n    };\n    let n = f.metadata()',
  [ZPA, CI]),
 ('R4 the non-Linux ask_ahead taken out', 'src/db/zfile.rs',
  '#[cfg(not(target_os = "linux"))]\nfn ask_ahead(_map: &MmapBytes, _from: usize, _to: usize) {}\n', '',
  ['cargo test --profile checked --test source']),
 ('R6 records_resident answers a whole reader only', 'src/db.rs',
  '        raw.record_region_place()\n            .and_then(|(off, len)| crate::bytes::Bytes::resident_share(raw.source(), off, len))',
  '        raw.record_regions()\n            .and(raw.record_region_place())\n            .and_then(|(off, len)| crate::bytes::Bytes::resident_share(raw.source(), off, len))',
  [ZPA, CI]),
 ('R7 OrdIndex::resident_share answers 0', 'src/ordindex.rs',
  '        self.map\n            .resident_share(0, self.map.len() as usize)\n            .unwrap_or(0.0)\n    }\n\n    /// Whether `advise_random`',
  '        0.0\n    }\n\n    /// Whether `advise_random`',
  [ZP + '--exact a_served_partitions_ordered_index_is_dropped_again_a_second_on', ZPA]),
 ('R8a ZFile::pages_resident answers 0', 'src/db/zfile.rs',
  '        self.map\n            .resident_share(0, self.map.len() as usize)\n            .unwrap_or(0.0)\n    }\n\n    /// Heap bytes',
  '        0.0\n    }\n\n    /// Heap bytes',
  [ZP + '--exact scans_after_a_quiet_stretch_have_the_cold_form_files_asked_for', ZPA, CI]),
 ('R8b ZFile::pages_resident answers 1', 'src/db/zfile.rs',
  '        self.map\n            .resident_share(0, self.map.len() as usize)\n            .unwrap_or(0.0)\n    }\n\n    /// Heap bytes',
  '        1.0\n    }\n\n    /// Heap bytes',
  [ZPA, CI]),
 ('R9a the paged arm of resident_share ignores its offset', 'src/bytes.rs',
  '        Some(p.resident(off, len))', '        let _ = off;\n        Some(p.resident(0, len))',
  [LIB + 'a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident the_heads_resident_are_asked']),
 ('R9b the paged arm of resident_share swaps offset and length', 'src/bytes.rs',
  '        Some(p.resident(off, len))', '        Some(p.resident(len as u64, off as usize))',
  [LIB + 'a_paged_mapping_and_a_mapped_file_say_alike_what_is_resident the_heads_resident_are_asked']),
]
def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()
sums = {}
for line in open(B + '/keep/SHA256SUMS'):
    h, f = line.split()
    sums[f[2:]] = h
want = set(sys.argv[1:])
res = open(B + '/mutants-results-r1.txt', 'a')
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
