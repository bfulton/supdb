#!/usr/bin/env python3
"""Every INTERVAL s, each thread of a bench pass: monotonic time, pid, tid,
comm, state, utime+stime ticks, and the top of its kernel stack. For laying
a thread's wall to running, a futex, a sleep, a fault or a sync."""
import glob, sys, time, signal
out, interval = sys.argv[1], float(sys.argv[2])
f = open(out, 'w', buffering=1 << 20)
signal.signal(signal.SIGTERM, lambda *a: (f.flush(), sys.exit(0)))
while True:
    t = time.clock_gettime(time.CLOCK_MONOTONIC)
    for pd in glob.glob('/sys/fs/cgroup/memory/**/supdb-bench-*/pass-*/cgroup.procs', recursive=True):
        try: pids = open(pd).read().split()
        except OSError: continue
        for pid in pids:
            for task in glob.glob(f'/proc/{pid}/task/*'):
                try:
                    st = open(task + '/stat').read()
                    r = st.rsplit(')', 1)[1].split()
                    state = r[0]; ticks = int(r[11]) + int(r[12])
                    comm = st.split('(', 1)[1].rsplit(')', 1)[0].replace(' ', '_')
                    frames = ''
                    if state != 'R':
                        stack = open(task + '/stack').read()
                        frames = '<'.join(l.split(' ', 1)[1].split('+')[0] for l in stack.split('\n')[:14] if l.strip())
                    f.write(f'{t:.4f} {pid} {task.rsplit("/",1)[1]} {comm} {state} {ticks} {frames}\n')
                except (OSError, ValueError, IndexError):
                    continue
    time.sleep(interval)
