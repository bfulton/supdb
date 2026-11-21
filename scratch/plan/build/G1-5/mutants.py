#!/usr/bin/env python3
"""G1-5 mutant runner: each mutant applied over the keep copies, its tests run,
the verdict appended to results.txt as it comes, the files put back by sum."""
import hashlib, subprocess, sys, os
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-5'
R = '/home/user/supdb'
K = B + '/keep'
FILES = {'src/blob.rs': 'src_blob.rs', 'src/bytes.rs': 'src_bytes.rs', 'src/flatindex.rs': 'src_flatindex.rs', 'tests/blob.rs': 'tests_blob.rs'}
SUMS = dict(l.split()[::-1] for l in open(K + '/SHA256SUMS'))
def sha(p): return hashlib.sha256(open(p, 'rb').read()).hexdigest()
def restore():
    for f, k in FILES.items():
        open(R + '/' + f, 'wb').write(open(K + '/' + k, 'rb').read())
        assert sha(R + '/' + f) == SUMS[k], f
BL = 'src/blob.rs'; BY = 'src/bytes.rs'; FI = 'src/flatindex.rs'
BLOB = [['cargo', 'test', '--profile', 'checked', '--test', 'blob']]
CIPH = [['cargo', 'test', '--profile', 'checked', '--test', 'cipher', '--', 'a_store_larger_than_its_cache_reads_as_itself_within_it', '--exact']]
M = [
 ('P1 authenticated true by default', [(BY, '    /// is always a correct answer.\n    fn authenticated(&self) -> bool {\n        false\n    }', '    /// is always a correct answer.\n    fn authenticated(&self) -> bool {\n        true\n    }')], BLOB),
 ('P2 the skip on piecewise alone', [(BL, '    if src.authenticated() {\n        return Ok(());\n    }', '    if src.piecewise() {\n        return Ok(());\n    }')], BLOB),
 ('P3 verify through Pieces asking for the section whole', [(FI, '    for (i, (a, b)) in pieces(crc_off, shift, base).enumerate() {\n        let got = sec.range(a, b).ok_or(i)?;', '    let all = sec.range(0, crc_off).ok_or(usize::MAX)?;\n    for (i, (a, b)) in pieces(crc_off, shift, base).enumerate() {\n        let got = &all[a..b];')], BLOB),
 ('P4 MmapBytes true for a mapped file', [(BY, '    fn authenticated(&self) -> bool {\n        self.map().is_none()\n    }', '    fn authenticated(&self) -> bool {\n        true\n    }')], BLOB),
 ('O1 MmapBytes false for a paged mapping', [(BY, '    fn authenticated(&self) -> bool {\n        self.map().is_none()\n    }', '    fn authenticated(&self) -> bool {\n        false\n    }')], CIPH),
 ('O2 a piece mismatch passed over', [(FI, '        if crate::block::crc32(got) != piece_crc(row, i).ok_or(usize::MAX)? {\n            return Err(i);\n        }\n    }\n    Ok(())\n}\n\n/// Where the fence', '        if crate::block::crc32(got) != piece_crc(row, i).ok_or(usize::MAX)? {\n            continue;\n        }\n    }\n    Ok(())\n}\n\n/// Where the fence')], BLOB),
 ('O3 the verdict dropped at the open', [(BL, '    flatindex::verify_pieces_in(sec, crc_off, shift, base)\n        .map_err(|p| corrupt(&format!("key index checksum mismatch in piece {p}")))', '    let _ = flatindex::verify_pieces_in(sec, crc_off, shift, base);\n    Ok(())')], BLOB),
 ('O4 the arm by the piece verifies nothing', [(BL, '                Some(p) => verify_row_by_piece(&src, p, idx.crc_off, idx.piece_shift, sb.key_off)?,', '                Some(_p) => {}')], BLOB),
 ('O5 the row asked one piece short', [(FI, '    for (i, (a, b)) in pieces(crc_off, shift, base).enumerate() {\n        let got = sec.range(a, b).ok_or(i)?;', '    for (i, (a, b)) in pieces(crc_off, shift, base).enumerate().skip(1) {\n        let got = sec.range(a, b).ok_or(i)?;')], BLOB),
]
only = sys.argv[1:]
res = open(B + '/results.txt', 'a')
env = dict(os.environ, TMPDIR='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-5')
for name, edits, cmds in M:
    if only and name.split()[0] not in only: continue
    restore()
    for f, old, new in edits:
        s = open(R + '/' + f).read()
        assert s.count(old) == 1, (name, old)
        open(R + '/' + f, 'w').write(s.replace(old, new))
    v = 'SURVIVED'
    for c in cmds:
        r = subprocess.run(c, cwd=R, env=env, capture_output=True, text=True, timeout=3000)
        out = r.stdout + r.stderr
        if 'could not compile' in out:
            v = 'BUILD-FAIL'; break
        if r.returncode != 0:
            msg = [l for l in out.split('\n') if 'panicked at' in l or l.startswith('test ') and 'FAILED' in l or 'opened' in l or 'asked' in l or 'fills' in l]
            v = 'KILLED ' + (' | '.join(msg[:4]))[:400]; break
    restore()
    res.write(f'{name}\t{v}\n'); res.flush()
    print(name, v, flush=True)
restore()
print('restored', flush=True)
