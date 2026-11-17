"""The E4 resume family with an idealised header: every header write is
durable at once (an oracle standing in for any header protocol that issues
no sync, e.g. sessions carried in ring frames).  resume_sync is off, so the
only sync before a resumed Handle's first frame is gone.  Question: does the
rest of the resumed Handle (its ring pins, its re-home) survive without the
sync F1's fix adds?"""
import sys, time
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-model')
import pfm, e4
from logrun import Runner
def write_header(s):
    slot = 0 if s.hdr_slot is None else s.hdr_slot ^ 1
    s.hdr_seq += 1
    body = (s.fid, s.hdr_seq, tuple(s.sessions))
    img = s.W.seal(s.kf, s.W.uniq('rnd'), body, kind='hdr', fid=s.fid, hdr_seq=s.hdr_seq, table=tuple(s.sessions))
    s.W.dev.dur[(s.ino, ('h', slot))] = img        # durable at once: no lag, no tear
    s.W.dev.pend.pop((s.ino, ('h', slot)), None)
    s.hdr_slot = slot
    s.hdr_safe = True
    s.hdr_t = s.W.dev.t
pfm.LogH.write_header = write_header
fam = sys.argv[1]
o, maxlen, cap = e4.parse(sys.argv[2:])
r = Runner(o, 'idealhdr %s %s maxlen=%d' % (fam, o, maxlen), branch_cap=cap)
t0 = time.time()
for ops in e4.FAMS[fam](r.o, maxlen):
    r.run(tuple(ops))
    if time.time() - t0 > 540:
        print('time limit reached'); break
if r.sampled: print('sampled at', cap, 'in', r.sampled, 'points')
r.T.report()
