# The io sampler for a run of several bench processes: every 50 ms until
# STOPFILE exists, CLOCK_MONOTONIC, the global Dirty and Writeback, vda's
# write ios, write sectors and flush ios, and the live bench group's dirty,
# writeback and cache bytes (the newest supdb-bench-* group).
import sys, time, os, glob
stop, out = sys.argv[1], open(sys.argv[2], 'w')
def meminfo():
    d = {}
    for ln in open('/proc/meminfo'):
        k, v = ln.split(':'); d[k] = int(v.split()[0])
    return d['Dirty'], d['Writeback']
while not os.path.exists(stop):
    t = time.monotonic()
    dirty, wb = meminfo()
    f = open('/sys/block/vda/stat').read().split()
    wios, wsec, fios = int(f[4]), int(f[6]), int(f[15]) if len(f) > 15 else 0
    cd = cw = cc = -1
    gs = glob.glob('/sys/fs/cgroup/memory/process_api/*/claude-code-bash/supdb-bench-*/memory.stat')
    if gs:
        try:
            for ln in open(max(gs, key=os.path.getmtime)):
                k, v = ln.split()
                if k == 'dirty': cd = int(v)
                elif k == 'writeback': cw = int(v)
                elif k == 'cache': cc = int(v)
        except OSError:
            pass
    out.write('%.3f %d %d %d %d %d %d %d %d\n' % (t, dirty, wb, wios, wsec, fios, cd, cw, cc))
    out.flush()
    time.sleep(0.05)
