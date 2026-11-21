#!/usr/bin/env python3
"""G1-5 round 3 mutant runner. argv[1]: 'old' runs each mutant with keep3's
tests/blob.rs (the test before this round), 'new' with keep4's. The src files
are keep4's (= keep3's). Verdicts appended to results5.txt as they come; the
four files put back from keep4/ and checked by sum after each."""
import hashlib, subprocess, sys, os
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-5'
R = '/home/user/supdb'
K = B + '/keep4'
FILES = {'src/blob.rs': 'src_blob.rs', 'src/bytes.rs': 'src_bytes.rs', 'src/flatindex.rs': 'src_flatindex.rs', 'tests/blob.rs': 'tests_blob.rs'}
SUMS = dict(l.split()[::-1] for l in open(K + '/SHA256SUMS'))
def sha(p): return hashlib.sha256(open(p, 'rb').read()).hexdigest()
def restore():
    for f, k in FILES.items():
        open(R + '/' + f, 'wb').write(open(K + '/' + k, 'rb').read())
        assert sha(R + '/' + f) == SUMS[k], f
BL = 'src/blob.rs'; BY = 'src/bytes.rs'; FI = 'src/flatindex.rs'
BLOB = [['cargo', 'test', '--profile', 'checked', '--test', 'blob']]
LOOP_HEAD = '        let got = sec.range(a, b).ok_or(i)?;\n'
M = [
 ('R1 X2-again: each piece held to the next word', [(FI, '        .zip(row.chunks_exact(4))\n', '        .zip(row.chunks_exact(4).skip(1))\n')]),
 ('R2 skip the row when it lies late', [(BL, '    if src.authenticated() {\n        return Ok(());\n    }', '    if src.authenticated() || crc_off > sec.size() / 2 {\n        return Ok(());\n    }')]),
 ('R3 MmapBytes::authenticated true at length zero', [(BY, '    fn authenticated(&self) -> bool {\n        self.map().is_none()\n    }', '    fn authenticated(&self) -> bool {\n        self.map().is_none() || self.len() == 0\n    }')]),
 ('R4 piece 0 asked, never compared', [(FI, LOOP_HEAD, LOOP_HEAD + '        if i == 0 {\n            continue;\n        }\n')]),
 ('R5 the last piece asked, never compared', [(FI, '    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n', '    let count = pieces(crc_off, shift, base).count();\n    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n'), (FI, LOOP_HEAD, LOOP_HEAD + '        if i + 1 == count {\n            continue;\n        }\n')]),
 ('R6 every other piece compared', [(FI, LOOP_HEAD, LOOP_HEAD + '        if i % 2 == 1 {\n            continue;\n        }\n')]),
 ('R7 base aligned to the piece', [(BL, '    flatindex::verify_pieces_in(sec, crc_off, shift, base)\n', '    flatindex::verify_pieces_in(sec, crc_off, shift, base & !((1u64 << shift) - 1))\n')]),
 ('R8 both first entries dropped', [(FI, '    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n        .zip(row.chunks_exact(4))\n', '    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n        .skip(1)\n        .zip(row.chunks_exact(4).skip(1))\n')]),
 ('R9 own: the piece ending at the row never compared', [(FI, LOOP_HEAD, LOOP_HEAD + '        if b == crc_off {\n            continue;\n        }\n')]),
 ('R10 own: a piece starting at the section start never compared', [(FI, LOOP_HEAD, LOOP_HEAD + '        if a == 0 {\n            continue;\n        }\n')]),
 ('R11 own: a short piece never compared', [(FI, LOOP_HEAD, LOOP_HEAD + '        if b - a < 1 << shift {\n            continue;\n        }\n')]),
 ('R12 own: the compare of the middle piece skipped', [(FI, '    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n', '    let count = pieces(crc_off, shift, base).count();\n    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n'), (FI, LOOP_HEAD, LOOP_HEAD + '        if i == count / 2 + 1 {\n            continue;\n        }\n')]),
]
mode = sys.argv[1]
only = sys.argv[2:]
res = open(B + '/results5.txt', 'a')
env = dict(os.environ, TMPDIR='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5')
for name, edits in M:
    if only and name.split()[0] not in only: continue
    restore()
    if mode == 'old':
        open(R + '/tests/blob.rs', 'wb').write(open(B + '/keep3/tests_blob.rs', 'rb').read())
    for f, old, new in edits:
        s = open(R + '/' + f).read()
        assert s.count(old) == 1, (name, old)
        open(R + '/' + f, 'w').write(s.replace(old, new))
    v = 'SURVIVED'
    for c in BLOB:
        r = subprocess.run(c, cwd=R, env=env, capture_output=True, text=True, timeout=3000)
        out = r.stdout + r.stderr
        open(B + '/out5-' + mode + '-' + name.split()[0] + '.log', 'w').write(out)
        if 'could not compile' in out:
            v = 'BUILD-FAIL'; break
        if r.returncode != 0:
            ls = out.split('\n')
            msg = [l for l in ls if l.startswith('test ') and 'FAILED' in l]
            msg += [ls[j] + ' :: ' + (ls[j+1] if j+1 < len(ls) else '') for j, l in enumerate(ls) if 'panicked at' in l]
            v = 'KILLED ' + (' | '.join(msg[:6]))[:900]; break
    restore()
    res.write(f'{mode}\t{name}\t{v}\n'); res.flush()
    print(mode, name, v, flush=True)
restore()
print('restored', flush=True)
