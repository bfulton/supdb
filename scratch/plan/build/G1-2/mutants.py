#!/usr/bin/env python3
# Mutants of G1-2. Each: edits (file, old, new) applied to the tree, an
# optional red input (also edits), a killer command that must exit non-zero
# for the mutant to be killed. Every edited file is put back after each
# mutant and checked by its sha256 against keep/SHA256SUMS' sources.
import hashlib, os, subprocess, sys, time
REPO = '/home/user/supdb'
B = os.path.dirname(os.path.abspath(__file__))
T = os.path.join(os.path.dirname(B), 'tmp', 'G1-2')
RES = os.path.join(B, 'mutants.results')
os.makedirs(T, exist_ok=True)
env = dict(os.environ, TMPDIR=T)

def sha(p):
    return hashlib.sha256(open(p, 'rb').read()).hexdigest()

SRC = 'tests/source.rs'
RUN = 'scripts/door/run.sh'
RED = 'scripts/door/red.sh'
CONF = 'scripts/door/clippy.toml'
def test(name):
    return f'cargo test --profile checked --test source {name} -- --exact'
REDUCED = [(CONF, '"libc::madvise", ', ''), (CONF, '"memmap2::MmapOptions::new",', '')]

M = [
 ('A run.sh guard gone, config missing', [(RUN, '[ -f scripts/door/clippy.toml ] || {\n  echo "door: no scripts/door/clippy.toml" >&2\n  exit 1\n}\n', '')], [('MOVE', CONF)], f'sh {RUN}'),
 ('B both guards gone, config missing', [(RUN, '[ -f scripts/door/clippy.toml ] || {\n  echo "door: no scripts/door/clippy.toml" >&2\n  exit 1\n}\n', ''), (RED, '[ -f "$conf/clippy.toml" ] || fail "no $conf/clippy.toml"\n', '')], [('MOVE', CONF)], f'sh {RUN}'),
 ('C red status swallowed (| tail -1)', [(RUN, 'TMPDIR="$red" sh scripts/door/red.sh\n', 'TMPDIR="$red" sh scripts/door/red.sh | tail -1\n')], REDUCED, f'sh {RUN}'),
 ('C0 control: reduced config, no mutant', [], REDUCED, f'sh {RUN}'),
 ('D ratchet counts only clippy::disallowed_', [(SRC, '''                || name == "clippy::style"
                || name == "clippy::all"
                || name == "warnings"
''', '')], [], test('a_new_allow_of_the_door_fails_in_each_spelling')),
 ('E literal rule does not skip test code', [(SRC, '''            if l.test {
                continue;
            }
            let n = thread_paths''', '''            let n = thread_paths''')], [], test('a_proc_path_outside_the_door_fails_and_a_tests_does_not')),
 ('F red copy not removed (trap gone)', [(RED, '''trap 'rm -rf "$copy"' EXIT\n''', '')], [], f'sh {RUN}'),
 ('G red.sh exit-status check gone', [(RED, '''[ "$status" -ne 0 ] || {
  printf '%s\\n' "$out"
  fail "the door passed six calls past it (exit 0)"
}
''', '')], REDUCED, f'sh {RED}'),
 ('H thread paths never counted', [(SRC, '''        .filter(|(at, w)| w == "thread" && starts(&c, at + w.len(), "::"))''', '''        .filter(|(at, w)| w == "thread_" && starts(&c, at + w.len(), "::"))''')], [], test('the_door_holds_what_clippy_cannot')),
 ('I inner attributes not read', [(SRC, '''        let inner = s.get(j) == Some(&'!');''', '''        let inner = false;''')], [], test('a_new_allow_of_the_door_fails_in_each_spelling')),
 ('J foreign block: ABI string not skipped', [(SRC, '''        if s.get(k) == Some(&'"') {
            k += 1;''', '''        if s.get(k) == Some(&'#') {
            k += 1;''')], [], test('a_foreign_block_outside_wasmapi_fails')),
 ('K use of thread::self read as a function', [(SRC, '''            n != "self" && n.chars()''', '''            n.chars()''')], [], test('a_thread_call_added_fails_and_a_use_of_a_thread_fn_fails')),
 ('L ordindex check gone', [(SRC, '''            if !open.contains("vault::map(") {''', '''            if open.is_empty() {''')], [], test('the_ordered_index_opens_through_the_door')),
 ('M red.sh checks only the first line', [(RED, 'for k in 1 2 3 4 5 6; do', 'for k in 1; do')], REDUCED, f'sh {RED}'),
 ('N a count of zero accepted', [(SRC, '''        if n == 0 {
            return Err(bad("a count of zero"));
        }''', '')], [], test('the_door_table_is_read_as_it_says')),
 ('O a shrink passes', [(SRC, '''        if h == w {
            continue;
        }
        let (kind, file, scope) = k;''', '''        if h <= w {
            continue;
        }
        let (kind, file, scope) = k;''')], [], test('an_allow_moved_or_taken_away_fails_until_the_list_follows')),
 ('P an outer allow keyed by its own line', [(SRC, '''                lines[line_of[at]].scope.clone()
            };''', '''                lines[ln].scope.clone()
            };''')], [], test('the_door_holds_what_clippy_cannot')),
 ('Q allow inside cfg_attr not read', [(SRC, '''        if w != "allow" && w != "expect" {''', '''        if (w != "allow" && w != "expect") || attr.iter().collect::<String>().contains("cfg_attr") {''')], [], test('a_new_allow_of_the_door_fails_in_each_spelling')),
]

BOGUS = [(CONF, '"libc::dlsym",', '"libc::dlsym", "libc::no_such_fn_g12",')]
M += [
 ('R0 control: an entry naming nothing, no mutant', [], BOGUS, f'sh {RUN}'),
 ('R an entry naming nothing passes (check gone)', [(RUN, "if printf '%s\\n' \"$out\" | grep -q 'does not refer to'; then", "if false; then")], BOGUS, f'sh {RUN}'),
]

def main(only=None):
    files = sorted({e[0] for m in M for e in m[1] + [x for x in m[2] if x[0] != 'MOVE']} | {CONF})
    base = {f: (open(os.path.join(REPO, f)).read(), sha(os.path.join(REPO, f))) for f in files}
    done = set()
    if os.path.exists(RES):
        for l in open(RES):
            done.add(l.split('\t')[0])
    for name, edits, red, cmd in M:
        if only and not name.startswith(only):
            continue
        if name in done:
            continue
        try:
            for f, old, new in edits + [x for x in red if x[0] != 'MOVE']:
                p = os.path.join(REPO, f)
                s = open(p).read()
                assert s.count(old) == 1, (name, f, old[:60], s.count(old))
                open(p, 'w').write(s.replace(old, new))
            moved = [x[1] for x in red if x[0] == 'MOVE']
            for f in moved:
                os.rename(os.path.join(REPO, f), os.path.join(B, 'moved_' + os.path.basename(f)))
            t0 = time.time()
            r = subprocess.run(cmd, shell=True, cwd=REPO, env=env, capture_output=True, text=True, timeout=1800)
            out = (r.stdout + r.stderr)[-3000:]
            verdict = 'KILLED' if r.returncode != 0 else 'SURVIVED'
            with open(os.path.join(B, 'mutants.log'), 'a') as lg:
                lg.write(f'===== {name}: {cmd} -> exit {r.returncode} ({time.time()-t0:.0f}s)\n{out}\n')
        finally:
            for f in [x[1] for x in red if x[0] == 'MOVE']:
                q = os.path.join(B, 'moved_' + os.path.basename(f))
                if os.path.exists(q):
                    os.rename(q, os.path.join(REPO, f))
            for f, (text, h) in base.items():
                p = os.path.join(REPO, f)
                if not os.path.exists(p) or sha(p) != h:
                    open(p, 'w').write(text)
                assert sha(p) == h, f
        with open(RES, 'a') as fh:
            fh.write(f'{name}\t{verdict}\texit {r.returncode}\t{cmd}\n')
        print(name, verdict, flush=True)

if __name__ == '__main__':
    main(sys.argv[1] if len(sys.argv) > 1 else None)
