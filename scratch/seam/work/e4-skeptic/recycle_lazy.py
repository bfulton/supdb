"""dirmodels.py recycle, with the spare's entry never opened (design.md:335
'Headers are opened lazily, at a file's first open'; db.rs:27858-27865 keeps
the spare's path or removes it, and never reads it)."""
import sys, itertools
sys.path.insert(0, '/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e4-model')
import pfm, dirmodels
from pfm import Tally
orig = dirmodels.open_entry
def open_entry(W, v, sp, ino, o):
    if sp[2].startswith('spare-'):
        return {'empty': True, 'why': 'never opened'}, None
    return orig(W, v, sp, ino, o)
dirmodels.open_entry = open_entry
o = dict(pfm.DEFAULT, resume_sync=True, sh_rehome=True, retry='R2', **dirmodels.OPT)
for a in sys.argv[1:]:
    k, v = a.split('='); o[k] = {'True': True, 'False': False}.get(v, int(v) if v.isdigit() else v)
T = Tally('recycle, spare never opened %s' % sys.argv[1:])
for life in ('fresh', 'resumed'):
    for n in range(0, 4):
        for ops in itertools.product([1, 5, 'S'], repeat=n):
            dirmodels.recycle_run(o, ops, T, life)
T.report()
