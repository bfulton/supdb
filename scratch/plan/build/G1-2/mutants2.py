#!/usr/bin/env python3
# Round 2: the gate scripts after the guards and the status check went and
# the clean went in. Each entry: name, mutant edits, red-input edits, the
# command, and what the unmutated gate does on that input: 'fail' (a red
# input: the mutant is killed when the command exits 0) or 'pass' (the green
# tree: killed when it exits non-zero). Controls carry no mutant and must
# behave as said. Files restored by sha after each.
import hashlib, os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
REPO = '/home/user/supdb'
B = os.path.dirname(os.path.abspath(__file__))
T = os.path.realpath(os.path.join(os.path.dirname(B), 'tmp', 'G1-2'))
RES = os.path.join(B, 'mutants2.results')
env = dict(os.environ, TMPDIR=T)
def sha(p): return hashlib.sha256(open(p, 'rb').read()).hexdigest()
RUN, RED, CONF = 'scripts/door/run.sh', 'scripts/door/red.sh', 'scripts/door/clippy.toml'
REDUCED = [(CONF, '"libc::madvise", ', ''), (CONF, '"memmap2::MmapOptions::new",', '')]
BOGUS = [(CONF, '"libc::dlsym",', '"libc::dlsym", "libc::no_such_fn_g12",')]
CLEAN = 'cargo clean --profile checked --target-dir target/door -p supdb -q || exit 1\n'
STALE = (f"mv {CONF} {B}/moved_conf && sh {RUN}; mv {B}/moved_conf {CONF} && "
         f"sed -i 's|\"libc::dlsym\",|\"libc::dlsym\", \"libc::no_such_fn_g12\",|' {CONF} && sh {RUN}")
M = [
 ('A0 control: config missing', [], [('MOVE', CONF)], f'sh {RUN}', 'fail'),
 ('C red status swallowed (| tail -1)', [(RUN, 'TMPDIR="$red" sh scripts/door/red.sh\n', 'TMPDIR="$red" sh scripts/door/red.sh | tail -1\n')], REDUCED, f'sh {RUN}', 'fail'),
 ('C0 control: reduced config', [], REDUCED, f'sh {RUN}', 'fail'),
 ('F red copy not removed (trap gone)', [(RED, '''trap 'rm -rf "$copy"' EXIT\n''', '')], [], f'sh {RUN}', 'pass'),
 ('F0 control: green tree', [], [], f'sh {RUN}', 'pass'),
 ('M red.sh checks only the first line', [(RED, 'for k in 1 2 3 4 5 6; do', 'for k in 1; do')], REDUCED, f'sh {RED}', 'fail'),
 ('R0 control: an entry naming nothing', [], BOGUS, f'sh {RUN}', 'fail'),
 ('R the names-nothing check gone', [(RUN, "if printf '%s\\n' \"$out\" | grep -q 'does not refer to'; then", "if false; then")], BOGUS, f'sh {RUN}', 'fail'),
 ('S0 control: a run without the config, then a bogus one', [], [], STALE, 'fail'),
 ('S the clean gone', [(RUN, CLEAN, '')], [], STALE, 'fail'),
 ('U the door run status ignored', [(RUN, '[ "$status" -eq 0 ] || exit 1\nif printf', 'if printf')], REDUCED[:0] + [(CONF, '"libc::dlsym",', '"libc::dlsym", "std::fs::File::sync_data",')], f'sh {RUN}', 'fail'),
]
def main():
    files = sorted({e[0] for m in M for e in m[1] + [x for x in m[2] if x[0] != 'MOVE']} | {CONF})
    base = {f: (open(os.path.join(REPO, f)).read(), sha(os.path.join(REPO, f))) for f in files}
    done = {l.split('\t')[0] for l in open(RES)} if os.path.exists(RES) else set()
    for name, edits, red, cmd, want in M:
        if name in done: continue
        try:
            for f, old, new in edits + [x for x in red if x[0] != 'MOVE']:
                p = os.path.join(REPO, f); s = open(p).read()
                assert s.count(old) == 1, (name, f, old[:60], s.count(old))
                open(p, 'w').write(s.replace(old, new))
            for x in red:
                if x[0] == 'MOVE': os.rename(os.path.join(REPO, x[1]), os.path.join(B, 'moved_conf'))
            t0 = time.time()
            r = subprocess.run(cmd, shell=True, cwd=REPO, env=env, capture_output=True, text=True, timeout=1800)
            control = not edits
            ok_as_said = (r.returncode != 0) == (want == 'fail')
            if control:
                verdict = 'CONTROL-OK' if ok_as_said else 'CONTROL-WRONG'
            else:
                verdict = 'SURVIVED' if ok_as_said else 'KILLED'
            with open(os.path.join(B, 'mutants2.log'), 'a') as lg:
                lg.write(f'===== {name}: {cmd} -> exit {r.returncode} ({time.time()-t0:.0f}s)\n{(r.stdout + r.stderr)[-2500:]}\n')
        finally:
            q = os.path.join(B, 'moved_conf')
            if os.path.exists(q): os.rename(q, os.path.join(REPO, CONF))
            for f, (text, h) in base.items():
                p = os.path.join(REPO, f)
                if not os.path.exists(p) or sha(p) != h: open(p, 'w').write(text)
                assert sha(p) == h, f
        with open(RES, 'a') as fh: fh.write(f'{name}\t{verdict}\texit {r.returncode}\twant {want}\t{cmd}\n')
        print(name, verdict, flush=True)
main()
