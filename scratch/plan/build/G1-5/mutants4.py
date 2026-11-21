#!/usr/bin/env python3
"""G1-5 mutant runner: each mutant applied over the keep copies, its tests run,
the verdict appended to results.txt as it comes, the files put back by sum."""
import hashlib, subprocess, sys, os
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-5'
R = '/home/user/supdb'
K = B + '/keep2'
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
 ('X1 Advised::authenticated false', [('tests/blob.rs', '    fn authenticated(&self) -> bool {\n        self.inner.authenticated()\n    }', '    fn authenticated(&self) -> bool {\n        false\n    }')], BLOB),
 ('X2 each piece held to the next word', [(FI, '        .zip(row.chunks_exact(4))\n', '        .zip(row.chunks_exact(4).skip(1))\n')], BLOB),
 ('X3 the skip on src.piecewise in verify_row_by_piece', [(BL, '    if src.authenticated() {\n        return Ok(());\n    }', '    if src.piecewise() {\n        return Ok(());\n    }')], BLOB),
 ('X4 the last piece dropped', [(FI, '    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n        .zip', '    for (i, ((a, b), word)) in pieces(crc_off, shift, base)\n        .take(n / 4 - 1)\n        .zip')], BLOB),
 ('X5 a piece not lent passed over', [(FI, '        let got = sec.range(a, b).ok_or(i)?;\n        if crate::block::crc32(got).to_le_bytes()', '        let Some(got) = sec.range(a, b) else { continue };\n        if crate::block::crc32(got).to_le_bytes()')], BLOB),
 ('X6 a row not lent passes', [(FI, '    let row = sec\n        .range(crc_off, crc_off.saturating_add(n))\n        .ok_or(usize::MAX)?;', '    let Some(row) = sec.range(crc_off, crc_off.saturating_add(n)) else {\n        return Ok(());\n    };')], BLOB),
 ('X7 base 0 at the open', [(BL, 'Some(p) => verify_row_by_piece(&src, p, idx.crc_off, idx.piece_shift, sb.key_off)?,', 'Some(p) => verify_row_by_piece(&src, p, idx.crc_off, idx.piece_shift, 0)?,')], BLOB),
 ('X8 the piece arm outside verify_index', [(BL, '        if idx.crc_off != 0 && opts.verify_index {\n            match key.pieces(&src) {', '        if idx.crc_off != 0 && (opts.verify_index || key.pieces(&src).is_some()) {\n            match key.pieces(&src) {')], BLOB),
 ('X9 own: the row range cut short (saturating_sub)', [(FI, '        .range(crc_off, crc_off.saturating_add(n))\n', '        .range(crc_off, crc_off.saturating_sub(n))\n')], BLOB),
 ('X10 own: a piece not lent named as the row', [(FI, '        let got = sec.range(a, b).ok_or(i)?;', '        let got = sec.range(a, b).ok_or(usize::MAX)?;')], BLOB),
 ('X11 own: a mismatch named as piece 0', [(FI, '        if crate::block::crc32(got).to_le_bytes()[..] != *word {\n            return Err(i);', '        if crate::block::crc32(got).to_le_bytes()[..] != *word {\n            return Err(0);')], BLOB),
 ('X12 own: the word compared big-endian', [(FI, 'crate::block::crc32(got).to_le_bytes()[..] != *word', 'crate::block::crc32(got).to_be_bytes()[..] != *word')], BLOB),
]
M += [
 ('E1 the row range empty', [(FI, '        .range(crc_off, crc_off.saturating_add(n))\n', '        .range(crc_off, crc_off)\n')], BLOB),
 ('E2 a piece not lent named usize::MAX', [(FI, '        let got = sec.range(a, b).ok_or(i)?;', '        let got = sec.range(a, b).ok_or(usize::MAX)?;')], BLOB),
 ('E3 base key_off + 1', [(BL, 'Some(p) => verify_row_by_piece(&src, p, idx.crc_off, idx.piece_shift, sb.key_off)?,', 'Some(p) => verify_row_by_piece(&src, p, idx.crc_off, idx.piece_shift, sb.key_off + 1)?,')], BLOB),
 ('E4 authenticated || piecewise at the callee', [(BL, '    if src.authenticated() {\n        return Ok(());\n    }', '    if src.authenticated() || src.piecewise() {\n        return Ok(());\n    }')], BLOB),
 ('E5 Advised::piecewise false, authenticated forwarded', [('tests/blob.rs', '    fn piecewise(&self) -> bool {\n        self.inner.piecewise()\n    }\n    fn authenticated', '    fn piecewise(&self) -> bool {\n        false\n    }\n    fn authenticated')], BLOB),
 ('E7 the last word never compared (chunks take len/4-1)', [(FI, '        .zip(row.chunks_exact(4))\n', '        .zip(row.chunks_exact(4).take(row.len() / 4 - 1))\n')], BLOB),
 ('E8 three bytes of the word compared', [(FI, 'crate::block::crc32(got).to_le_bytes()[..] != *word', 'crate::block::crc32(got).to_le_bytes()[..3] != word[..3]')], BLOB),
 ('E10 a row not lent read as no words', [(FI, '        .range(crc_off, crc_off.saturating_add(n))\n        .ok_or(usize::MAX)?;', '        .range(crc_off, crc_off.saturating_add(n))\n        .unwrap_or(&[]);')], BLOB),
]
only = sys.argv[1:]
res = open(B + '/results3.txt', 'a')
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
            ls = out.split('\n')
            msg = [l for l in ls if l.startswith('test ') and 'FAILED' in l]
            msg += [ls[j] + ' :: ' + (ls[j+1] if j+1 < len(ls) else '') for j, l in enumerate(ls) if 'panicked at' in l]
            v = 'KILLED ' + (' | '.join(msg[:6]))[:900]; break
    restore()
    res.write(f'{name}\t{v}\n'); res.flush()
    print(name, v, flush=True)
restore()
print('restored', flush=True)
