# Samples, every 50 ms until the pid given exits: CLOCK_MONOTONIC, the global
# Dirty and Writeback, vda's write ios, write sectors and flush ios, and the
# bench cgroup's dirty, writeback and cache bytes when its path is known.
import sys, time, os, re
pid = int(sys.argv[1]); err = sys.argv[2]; out = open(sys.argv[3], 'w')
cg = None
def meminfo():
    d = {}
    for ln in open('/proc/meminfo'):
        k, v = ln.split(':'); d[k] = int(v.split()[0])
    return d['Dirty'], d['Writeback']
while True:
    try:
        os.kill(pid, 0)
    except OSError:
        break
    if cg is None:
        try:
            m = re.search(r'in group (\S+)', open(err).read(4096))
            if m: cg = m.group(1)
        except OSError:
            pass
    t = time.monotonic()
    dirty, wb = meminfo()
    f = open('/sys/block/vda/stat').read().split()
    wios, wsec, fios = int(f[4]), int(f[6]), int(f[15]) if len(f) > 15 else 0
    cd = cw = cc = -1
    if cg:
        try:
            for ln in open(cg + '/memory.stat'):
                k, v = ln.split()
                if k == 'dirty': cd = int(v)
                elif k == 'writeback': cw = int(v)
                elif k == 'cache': cc = int(v)
        except OSError:
            pass
    out.write('%.3f %d %d %d %d %d %d %d %d\n' % (t, dirty, wb, wios, wsec, fios, cd, cw, cc))
    out.flush()
    time.sleep(0.05)
