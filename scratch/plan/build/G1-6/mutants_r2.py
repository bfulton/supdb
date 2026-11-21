#!/usr/bin/env python3
# mutants_r2.py [KEY...]: repair round 2's mutants. Each applied, every one
# of its commands run (not stopping at the first red, so that a mutant is
# shown red at each test the round names), the verdict per command written
# to mutants-results-r2.txt as it comes, the file put back from keep/ and
# checked by its sum after each. C0 is the control (no change): its strace
# gate must be green.
import hashlib, os, subprocess, sys, time
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-6'
T = '/home/user/supdb'
TMP = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-6-mut'
LIB = 'cargo test --profile checked --lib -- --exact '
ZP = 'cargo test --profile checked --test zpages -- --exact '
CI = 'cargo test --profile checked --test cipher -- --exact '
def GATE(key):
    return ("BIN=$(cargo test --profile checked --test zpages --no-run 2>&1 | sed -n 's/.*Executable tests\\/zpages.rs (\\(.*\\))/\\1/p'); "
            f"echo bin $BIN; sh {B}/strace/gate.sh \"$BIN\" {B}/strace/gate-{key}")
ZT = 'db::zfile::tests::'
M = [
 ('C0 control: no change', 'src/db/zfile.rs', None, None, [GATE('C0')]),
 ('E1 drop_behind passes its end as a length', 'src/db/zfile.rs',
  '    map.advise_dontneed_range(a as u64, b - a);\n', '    map.advise_dontneed_range(a as u64, b);\n',
  [LIB + ZT + 'a_drop_behind_unmaps_the_window_and_no_page_past_it', GATE('E1')]),
 ('E2 records_resident asks the directory', 'src/db.rs',
  '        raw.record_region_place()\n            .and_then(|(off, len)| crate::bytes::Bytes::resident_share(raw.source(), off, len))\n            .unwrap_or(0.0)',
  '        raw.record_dir_place()\n            .and_then(|(off, len)| crate::bytes::Bytes::resident_share(raw.source(), off, len))\n            .unwrap_or(0.0)',
  [ZP + 'builds_ahead_at_landing_read_the_records_residency_and_not_the_directorys',
   ZP + 'builds_ahead_at_landing_start_past_the_percent_and_only_from_resident_records',
   ZP + 'builds_ahead_under_a_cipher_seal_their_form_files_and_keep_them_cold']),
 ('E3 warm_file measures the slice', 'src/db/zfile.rs',
  '    let map = super::vault::map(path)?;\n    let len = map.len() as usize;\n',
  '    let map = super::vault::map(path)?;\n    let len = map.as_slice().len();\n',
  [ZP + 'a_warm_under_a_cipher_asks_for_the_envelope_and_decrypts_nothing']),
 ('E4 recs_at and dir_at swapped', 'src/db/zfile.rs',
  '                .map(|((r, _), (d, _))| (r as usize, d as usize))\n',
  '                .map(|((r, _), (d, _))| (d as usize, r as usize))\n',
  [LIB + ZT + 'a_form_file_is_the_same_bytes_whatever_the_piece_and_the_window',
   LIB + ZT + 'a_stride_walk_that_declines_alone_gives_back_what_it_read',
   LIB + ZT + 'a_stride_file_is_the_same_bytes_whatever_the_piece_and_the_window',
   ZP + 'the_build_drops_the_raw_records_it_passed_when_asked_to_and_not_otherwise']),
 ('E5 ask_ahead without the alignment', 'src/db/zfile.rs',
  '    let a = from & !(ZPAGE - 1);\n    if a < to {\n        map.advise_willneed',
  '    let a = from;\n    if a < to {\n        map.advise_willneed',
  ['cargo test --profile checked --lib -- db::zfile::tests::', GATE('E5')]),
 ('E6 the warm skips a chunk half resident', 'src/db/zfile.rs',
  '        if share >= 1.0 {\n            w.skipped', '        if share >= 0.5 {\n            w.skipped',
  [ZP + 'an_attached_form_file_is_warmed_once_per_process_unless_the_warm_is_off',
   LIB + ZT + 'a_warm_reads_the_cold_file_once_and_skips_a_resident_one']),
 ('E7 records_resident: no answer reads resident', 'src/db.rs',
  '            .unwrap_or(0.0)\n    }\n\n    /// The record region\'s place',
  '            .unwrap_or(1.0)\n    }\n\n    /// The record region\'s place',
  ['cargo test --profile checked --test zpages', 'cargo test --profile checked --test cipher',
   'cargo test --profile checked --lib -- db::zfile::']),
 ('E8 records_by_descriptor answers a reader by the piece', 'src/db.rs',
  '        raw.record_regions()?;\n        raw.record_region_place().map(|(at, len)| (at, len as u64))',
  '        raw.record_region_place().map(|(at, len)| (at, len as u64))',
  [ZP + 'form_files_an_earlier_open_left_under_a_cipher_are_dormant_and_then_attached']),
 ('E9 a read-back under half resident reads as the device', 'src/db/zfile.rs',
  '    let device = read_back_residency(&map, plan) == Some(0.0);\n',
  '    let device = read_back_residency(&map, plan).is_some_and(|s| s < 0.5);\n',
  [LIB + ZT + 'a_stride_build_is_trusted_only_where_its_read_back_read_the_device']),
]
def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()
sums = {}
for line in open(B + '/keep/SHA256SUMS'):
    h, f = line.split()
    sums[f[2:]] = h
want = set(sys.argv[1:])
res = open(B + '/mutants-results-r2.txt', 'a')
for name, f, old, new, tests in M:
    key = name.split()[0]
    if want and key not in want:
        continue
    path = os.path.join(T, f)
    assert sha(path) == sums[f], f'{f} is not the kept copy before {key}'
    if old is not None:
        src = open(path).read()
        assert src.count(old) == 1, f'{key}: the old text occurs {src.count(old)} times'
        open(path, 'w').write(src.replace(old, new))
    rcs = []
    log = f'{B}/mut-{key}.log'
    with open(log, 'w') as lf:
        env = dict(os.environ, TMPDIR=TMP)
        for t in tests:
            os.makedirs(TMP, exist_ok=True)
            lf.write(f'$ {t}\n'); lf.flush()
            r = subprocess.run(t, shell=True, cwd=T, stdout=lf, stderr=subprocess.STDOUT, env=env)
            lf.write(f'rc {r.returncode}\n'); lf.flush()
            rcs.append(r.returncode)
            subprocess.run(['rm', '-rf', '--', TMP])
    subprocess.run(['cp', f'{B}/keep/{f}', path])
    assert sha(path) == sums[f], f'{f} not put back after {key}'
    verdict = 'KILLED' if any(rcs) else 'SURVIVED'
    res.write(f'{time.strftime("%H:%M:%S")} {name}: {verdict} rcs {rcs}\n'); res.flush()
res.write('EXIT 0\n')
