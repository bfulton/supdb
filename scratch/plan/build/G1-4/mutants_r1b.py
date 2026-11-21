#!/usr/bin/env python3
"""G1-4 mutant runner: each mutant applied to the keep copies, the test run,
the verdict appended to results.txt as it comes, the files put back by sum."""
import hashlib, subprocess, sys, os
B = '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-4'
R = '/home/user/supdb'
K = B + '/keep'
FILES = {'src/blob.rs': 'src_blob.rs', 'src/flatindex.rs': 'src_flatindex.rs', 'tests/blob.rs': 'tests_blob.rs'}
SUMS = dict(l.split()[::-1] for l in open(K + '/SHA256SUMS'))
def sha(p): return hashlib.sha256(open(p, 'rb').read()).hexdigest()
def restore():
    for f, k in FILES.items():
        open(R + '/' + f, 'wb').write(open(K + '/' + k, 'rb').read())
        assert sha(R + '/' + f) == SUMS[k], f
P = 'src/blob.rs'; T = 'tests/blob.rs'
M = [
 ('M1 ranks by piece answers 0', [(P, 'fn willneed_ranks_by_piece(&self, rank: usize, n: usize, cap: usize) -> usize {\n', 'fn willneed_ranks_by_piece(&self, rank: usize, n: usize, cap: usize) -> usize {\n        if rank < usize::MAX { return 0; }\n')]),
 ('M2 dir by piece answers 0', [(P, 'fn willneed_dir_by_piece(&self, rank: usize, n: usize) -> usize {\n', 'fn willneed_dir_by_piece(&self, rank: usize, n: usize) -> usize {\n        if rank < usize::MAX { return 0; }\n')]),
 ('M3 record_offset by piece answers None', [(P, 'fn record_offset_by_piece(&self, rank: usize) -> Option<u64> {\n', 'fn record_offset_by_piece(&self, rank: usize) -> Option<u64> {\n        if rank < usize::MAX { return None; }\n')]),
 ('M4 ranks offset without index_offset', [(P, 'let off = self.index_offset() + recs_at as u64 + a as u64;\n        // A span', 'let off = recs_at as u64 + a as u64;\n        // A span')]),
 ('M5 record offset without index_offset', [(P, 'Some(self.index_offset() + recs_at as u64 + a as u64)', 'Some(recs_at as u64 + a as u64)')]),
 ('M6 dir offset without index_offset', [(P, 'let off = self.index_offset() + dir_at as u64 + a as u64;', 'let off = dir_at as u64 + a as u64;')]),
 ('M7a record offset without the region start', [(P, 'Some(self.index_offset() + recs_at as u64 + a as u64)', 'Some(self.index_offset() + a as u64)')]),
 ('M7b ranks offset at the directory start', [(P, 'let recs_at = recs.start_in(sec);\n        let off = self.index_offset() + recs_at as u64 + a as u64;\n        // A span', 'let recs_at = dir.start_in(sec);\n        let off = self.index_offset() + recs_at as u64 + a as u64;\n        // A span')]),
 ('M8a piece arm only: rank + 1', [(P, '        self.willneed_ranks_in(sec, idx, rank, n, cap)\n    }\n\n    /// `willneed_ranks` over', '        self.willneed_ranks_in(sec, idx, rank + 1, n, cap)\n    }\n\n    /// `willneed_ranks` over')]),
 ('M8b M8a with the equality assertion dropped', [(P, '        self.willneed_ranks_in(sec, idx, rank, n, cap)\n    }\n\n    /// `willneed_ranks` over', '        self.willneed_ranks_in(sec, idx, rank + 1, n, cap)\n    }\n\n    /// `willneed_ranks` over'), (T, '            assert_eq!(\n                trace, whole_trace,', '            assert_ne!(\n                0, 1,')]),
 ('M8c piece arm only: record offset of rank + 1, equality dropped', [(P, '        let (sec, idx) = self.pieces()?;\n        self.record_offset_in(sec, idx, rank)', '        let (sec, idx) = self.pieces()?;\n        self.record_offset_in(sec, idx, rank + 1)'), (T, '            assert_eq!(\n                trace, whole_trace,', '            assert_ne!(\n                0, 1,')]),
 ('O1 region place without index_offset', [(P, 'Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))', 'Some((recs.start_in(sec) as u64, recs.size()))')]),
 ('O2 dir ask without the word past the last', [(P, 'idx.dir_span(rank, n.saturating_add(1))', 'idx.dir_span(rank, n)')]),
 ('O3 ranks span not cut at the region end', [(P, '            .min(a.saturating_add(cap))\n            .min(recs.size());', '            .min(a.saturating_add(cap));\n        let _ = recs;')]),
 ('O4 whole arm sent to the piece arm', [(P, '        let Some((sec, idx)) = self.flat_lent() else {\n            return self.record_offset_by_piece(rank);\n        };\n        self.record_offset_in(sec, idx, rank)', '        let Some((_sec, _idx)) = self.flat_lent() else {\n            return self.record_offset_by_piece(rank);\n        };\n        self.record_offset_by_piece(rank)')]),
 ('O5 region place length short by one', [(P, 'Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))', 'Some((self.index_offset() + recs.start_in(sec) as u64, recs.size() - 1))')]),
 ('O6 region place by the piece only (the old shape)', [(P, '        indexed!(\n            self,\n            |sec, idx| {\n                let (recs, _) = idx.regions(sec)?;\n                Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))\n            },\n            None\n        )', '        let (sec, idx) = self.pieces()?;\n        let (recs, _) = idx.regions(sec)?;\n        Some((recs.offset(), recs.size()))')]),
 ('O7 Pieces::start_in off by one', [('src/flatindex.rs', '(self.off - outer.off) as usize', '(self.off - outer.off) as usize + 1')]),
 ('O8 slice start_in answers 0', [('src/flatindex.rs', 'self.as_ptr() as usize - outer.as_ptr() as usize', '{ let _ = outer; 0 }')]),
 ('O9 flat_lent answers None for a whole reader too', [(P, "            Index::Flat { key, idx } => Some((key.lend(&self.src)?, idx)),", "            Index::Flat { key, idx } => { let _ = (key, idx); None }")]),
 ('X1 dir ask placed at the record region start, both arms', [(P, '        let Some((_, dir)) = idx.regions(sec) else {\n            return 0;\n        };\n        // Inside', '        let Some((recs_x, dir)) = idx.regions(sec) else {\n            return 0;\n        };\n        // Inside'), (P, 'let dir_at = dir.start_in(sec);', 'let dir_at = recs_x.start_in(sec);')]),
 ('X2 region place through flat() alone', [(P, '        indexed!(\n            self,\n            |sec, idx| {\n                let (recs, _) = idx.regions(sec)?;\n                Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))\n            },\n            None\n        )', '        let (sec, idx) = self.flat()?;\n        let (recs, _) = idx.regions(sec)?;\n        Some((self.index_offset() + recs.start_in(sec) as u64, recs.size()))')]),
 ('X3a piece arm: ranks span cut at the section end', [(P, '            .min(a.saturating_add(cap))\n            .min(recs.size());', '            .min(a.saturating_add(cap))\n            .min(if self.flat_lent().is_none() { sec.size() } else { recs.size() });')]),
 ('X3b piece arm: dir span cut at the section end', [(P, 'let b = b.min(dir.size());', 'let b = b.min(if self.flat_lent().is_none() { sec.size() } else { dir.size() });')]),
 ('X4a dir: residency skip deleted', [(P, '        let off = self.index_offset() + dir_at as u64 + a as u64;\n        if self.src.resident_at(off) == Some(true) {\n            return 0;\n        }', '        let off = self.index_offset() + dir_at as u64 + a as u64;')]),
 ('X4b ranks: residency skip deleted', [(P, '        if self.src.resident_at(off) == Some(true) {\n            return 0;\n        }\n        self.src.advise_willneed(off, b - a)\n    }\n\n    /// The directory words', '        self.src.advise_willneed(off, b - a)\n    }\n\n    /// The directory words')]),
 ('X5a ranks: a >= b to a > b', [(P, '        if a >= b {\n            return 0;\n        }\n        let recs_at', '        if a > b {\n            return 0;\n        }\n        let recs_at')]),
 ('X5b dir: a >= b to a > b', [(P, '        if a >= b {\n            return 0;\n        }\n        let dir_at', '        if a > b {\n            return 0;\n        }\n        let dir_at')]),
 ('X6 piece arm: record offset by the directory start', [(P, '        let recs_at = recs.start_in(sec);\n        Some(', '        let recs_at = if self.flat_lent().is_none() { dir.start_in(sec) } else { recs.start_in(sec) };\n        Some(')]),
 ('X7 ranks by piece not cold, inlinable', [(P, '    #[cfg(not(target_family = "wasm"))]\n    #[cold]\n    #[inline(never)]\n    fn willneed_ranks_by_piece', '    #[cfg(not(target_family = "wasm"))]\n    fn willneed_ranks_by_piece')]),
 ('X8 region place by the piece answered by Pieces::offset', [(P, '        indexed!(\n            self,\n            |sec, idx| {', '        if let Some((sec, idx)) = self.pieces() {\n            let (recs, _) = idx.regions(sec)?;\n            return Some((recs.offset(), recs.size()));\n        }\n        indexed!(\n            self,\n            |sec, idx| {')]),
 ('Y2 Sec::lend answers None for a copied section', [(P, '            Sec::Owned(v, _) => Some(v),', '            Sec::Owned(v, _) => { let _ = v; None }')]),
 ('Y3 dir ask from the next rank, both arms', [(P, 'idx.dir_span(rank, n.saturating_add(1))', 'idx.dir_span(rank + 1, n.saturating_add(1))')]),
 ('Y4 flat_lent through flat (the error made) -- equivalent in answers', [(P, "            Index::Flat { key, idx } => Some((key.lend(&self.src)?, idx)),", "            Index::Flat { key, idx } => Some((key.get(&self.src).ok()?, idx)),")]),
]
only = sys.argv[1:]
res = open(B + '/results-r1b.txt', 'a')
env = dict(os.environ, TMPDIR=B.replace('/G1-4', '/tmp/G1-4'))
for name, edits in M:
    if only and name.split()[0] not in only: continue
    restore()
    for f, old, new in edits:
        s = open(R + '/' + f).read()
        assert s.count(old) == 1, (name, old)
        open(R + '/' + f, 'w').write(s.replace(old, new))
    r = subprocess.run(['cargo', 'test', '--profile', 'checked', '--test', 'blob', 'the_advice_by_rank'], cwd=R, env=env, capture_output=True, text=True, timeout=3000)
    out = r.stdout + r.stderr
    if 'error[' in out or 'error:' in out and 'could not compile' in out:
        v = 'BUILD-FAIL'
    elif r.returncode == 0:
        v = 'SURVIVED'
    else:
        msg = [l for l in out.split('\n') if 'panicked at' in l or 'failed:' in l or 'assertion' in l]
        v = 'KILLED ' + (' | '.join(msg[:2]))[:300]
    res.write(f'{name}\t{v}\n'); res.flush()
    print(name, v, flush=True)
restore()
print('restored', flush=True)
