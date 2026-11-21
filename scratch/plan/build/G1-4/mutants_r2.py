#!/usr/bin/env python3
"""G1-4 round 2 mutant runner: each mutant applied to the keep copies, its tests run,
the verdict appended to results-r2.txt as it comes, the files put back by sum."""
import hashlib, subprocess, sys, os
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4'
R = '/home/user/supdb'
K = B + '/keep'
FILES = {'src/blob.rs': 'src_blob.rs', 'src/flatindex.rs': 'src_flatindex.rs', 'tests/blob.rs': 'tests_blob.rs',
         'src/db.rs': 'src_db.rs', 'tests/fences.rs': 'tests_fences.rs'}
SUMS = dict(l.split()[::-1] for l in open(K + '/SHA256SUMS'))
def sha(p): return hashlib.sha256(open(p, 'rb').read()).hexdigest()
def restore():
    for f, k in FILES.items():
        if sha(R + '/' + f) != SUMS[k]:
            open(R + '/' + f, 'wb').write(open(K + '/' + k, 'rb').read())
        assert sha(R + '/' + f) == SUMS[k], f
P = 'src/blob.rs'; F = 'src/flatindex.rs'; D = 'src/db.rs'
BLOB = [['--test', 'blob', 'the_advice_by_rank']]
DROP = [['--test', 'fences', 'drop'], ['--test', 'zpages', 'a_merge_drops']]
GUARD = '        if a >= b {\n            return 0;\n        }\n        let recs_at'
M = [
 ('Z1 dir span without the word past the last', BLOB, [(P, 'idx.dir_span(rank, n.saturating_add(1))', 'idx.dir_span(rank, n)')]),
 ('Z2 region place start_in arguments swapped', BLOB, [(P, 'Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))', 'Some((self.index_offset() + sec.start_in(recs) as u64, recs.size()))')]),
 ('Z3 ranks by piece with cap usize::MAX', BLOB, [(P, '        self.willneed_ranks_in(sec, idx, rank, n, cap)\n    }\n\n    /// `willneed_ranks` over', '        self.willneed_ranks_in(sec, idx, rank, n, usize::MAX)\n    }\n\n    /// `willneed_ranks` over')]),
 ('Z4 Sec::lend Lent arm None', BLOB, [(P, '            Sec::Lent { off, len } => src.lend_at(*off, *len),\n            Sec::Owned(v, _) => Some(v),', '            Sec::Lent { off, len } => { let _ = (off, len, src); None }\n            Sec::Owned(v, _) => Some(v),')]),
 ('Z5 compact_run dropped_to from record_offset(1)', DROP, [(D, '.map(|b| b.record_offset(0).unwrap_or(0))', '.map(|b| b.record_offset(1).unwrap_or(0))')]),
 ('Z6 Sec::lend Lent arm by slice_at', BLOB, [(P, '            Sec::Lent { off, len } => src.lend_at(*off, *len),\n            Sec::Owned(v, _) => Some(v),', '            Sec::Lent { off, len } => src.slice_at(*off, *len),\n            Sec::Owned(v, _) => Some(v),')]),
 ('Z7 Pieces::start_in absolute', BLOB, [(F, '(self.off - outer.off) as usize', '{ let _ = outer; self.off as usize }')]),
 ("Z11 a drop to the record's page, not a page behind", DROP, [(D, 'let to = off.saturating_sub(4096) & !4095;', 'let to = off & !4095;')]),
 ("Z12 each drop from the input's first record (dropped_to not moved)", DROP, [(D, '        dropped_bytes += to - dropped_to[i];\n        dropped_to[i] = to;', '        dropped_bytes += to - dropped_to[i];')]),
 ("Z13 the first drop from the file's start", DROP, [(D, '.map(|b| b.record_offset(0).unwrap_or(0))', '.map(|_| 0)')]),
 ('Z10 ranks guard removed, both arms', BLOB, [(P, GUARD, '        let recs_at')]),
 ('Z10s ranks guard removed in the slice arm only', BLOB, [(P, GUARD, '        if a >= b && self.flat_lent().is_none() {\n            return 0;\n        }\n        let recs_at')]),
 ('Z10p ranks guard removed in the piece arm only', BLOB, [(P, GUARD, '        if a >= b && self.flat_lent().is_some() {\n            return 0;\n        }\n        let recs_at')]),
]
only = sys.argv[1:]
res = open(B + '/results-r2.txt', 'a')
env = dict(os.environ, TMPDIR='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-4')
os.makedirs(env['TMPDIR'], exist_ok=True)
for name, tests, edits in M:
    if only and name.split()[0] not in only: continue
    restore()
    for f, old, new in edits:
        s = open(R + '/' + f).read()
        assert s.count(old) == 1, (name, old)
        open(R + '/' + f, 'w').write(s.replace(old, new))
    verdicts = []
    for t in tests:
        r = subprocess.run(['cargo', 'test', '--profile', 'checked'] + t, cwd=R, env=env, capture_output=True, text=True, timeout=3000)
        out = r.stdout + r.stderr
        if 'could not compile' in out:
            verdicts.append('BUILD-FAIL ' + ' '.join(l for l in out.split('\n') if l.startswith('error'))[:200]); continue
        ran = [l for l in out.split('\n') if l.startswith('test result')]
        if r.returncode == 0:
            verdicts.append(f'{t[1]}: passed {ran}')
        else:
            msg = [l for l in out.split('\n') if 'panicked at' in l or 'failed:' in l or 'assertion' in l]
            verdicts.append(f'{t[1]}: KILLED ' + (' | '.join(msg[:2]))[:300])
    v = ('KILLED' if any('KILLED' in x for x in verdicts) else 'SURVIVED' if all('passed' in x for x in verdicts) else 'OTHER') + ' :: ' + ' || '.join(verdicts)
    res.write(f'{name}\t{v}\n'); res.flush()
    print(name, v, flush=True)
restore()
print('restored', flush=True)
