#!/usr/bin/env python3
"""Every INTERVAL s, each thread of a bench pass whose kernel stack holds a
block free (evict, ext4_free_blocks, discard): monotonic time, tid, comm and
the stack, so the frees are laid to the thread and the call that paid them."""
import glob, sys, time, signal
out, interval = sys.argv[1], float(sys.argv[2])
f = open(out, 'w', buffering=1 << 16)
signal.signal(signal.SIGTERM, lambda *a: (f.flush(), sys.exit(0)))
KEYS = ('evict', 'free_blocks', 'discard')
while True:
    t = time.clock_gettime(time.CLOCK_MONOTONIC)
    for pd in glob.glob('/sys/fs/cgroup/memory/**/supdb-bench-*/pass-*/cgroup.procs', recursive=True):
        try: pids = open(pd).read().split()
        except OSError: continue
        for pid in pids:
            for task in glob.glob(f'/proc/{pid}/task/*'):
                try:
                    stack = open(task + '/stack').read()
                    if not any(k in stack for k in KEYS): continue
                    comm = open(task + '/comm').read().strip().replace(' ', '_')
                    frames = '<'.join(l.split(' ', 1)[1].split('+')[0] for l in stack.split('\n') if l.strip())
                    f.write(f'{t:.4f} {task.rsplit("/",1)[1]} {comm} {frames}\n')
                except (OSError, ValueError, IndexError):
                    continue
    time.sleep(interval)
