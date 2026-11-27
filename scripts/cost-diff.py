#!/usr/bin/env python3
"""One probe's count, base against head, by function (scripts/cost.sh).

    cost-diff.py BASE.self HEAD.self OPS HOT NAME MODE OWN

BASE.self and HEAD.self are `callgrind_annotate --inclusive=no
--threshold=100` outputs: each function's own instructions. The verdict is
by function and never by the total: a hot function that gains past its
slack fails, whatever the total did, and a total that moved with no hot
function gaining passes, since the total is the sum of every inlining
decision the compiler made and names no cost. A side whose toggle
collected under one instruction an operation fails as well: a toggle that
names no function reads zero on both sides, and zero against zero is no
count; and so does a side whose functions do not sum to its program total,
a listing cut at a threshold, which leaves out the small functions a cost
is added to first. Exit 0 green, 1 a hot gain, an empty or a partial side,
2 an input it cannot read.

MODE is `toggle` for a probe whose callgrind toggle collects the operation
alone, `whole` for one that collects the whole program; OWN is the
comma-separated path prefixes of the engine, the suite and the probe's own
crate (`supdb::,supdb_bench::,read_only::`). What is hot, and its slack:

  toggle  Every function compiled from source -- the engine's, the suite's,
          the probe's, and the standard library's and the dependencies'
          code built into the binary -- at TOGGLE_SLACK instructions over
          the whole run, not an operation: a toggle collects the operation
          on its own thread and nothing else, and against itself every such
          function has read the same to the instruction but a few that wait
          on another thread or a clock -- the segment writer's look at its
          helper (+63 over a commit probe's run), the seal's wait in `land`
          (+332) -- which `~` lines bound; so a cost of one instruction a
          block or a piece of a probe's whole run is red where a slack an
          operation would let it through. A library function instantiated
          over the engine's types (`std::thread::local::LocalKey::with::<
          supdb_bench::engines::consume::{closure#0}>`) is keyed apart from
          its other monomorphs as `<name> (over the engine)`. A function
          with no source -- libc's, the allocator's, the loader's: its file
          is `./...`, `/usr/...` or `???` -- moves with the addresses of
          what it is handed, the point read's memcmp by most of an
          instruction a read between two runs of one build, so it fails only
          where it gains past LIB_SLACK an operation and past LIB_REL of its
          own count both, which a copy of twice the bytes at a step of every
          operation is.
  whole   Every function of OWN by name (`supdb::...`), and every function
          with a bare name and a file of the workspace (a `#[no_mangle]`
          helper), at WHOLE_SLACK an operation: these probes count the
          store's open, its background threads and its close beside their
          operations, so a function a background thread runs on a clock or
          polls while another works moves between two runs of one build,
          and the slack is the measured noise of the rest, ten times over.
          Libraries are not hot here: their counts follow the threads.

Every line of HOT (scripts/cost-hot.txt, one path a line, as below) is hot
in both modes, at the mode's slack. A line that begins with `!` takes the
functions it matches back out in both modes. One that begins with `~` gives
the functions it matches a bound of their own, `~path BOUND`, BOUND in
instructions an operation, in place of the mode's slack: a function whose
count follows the clock or another thread's timing, by as much as two runs
of one build have been measured to move it, three times over. Written so it
holds in every probe with no toggle; written `~probe,probe: path BOUND`, in
those probes alone, whatever their mode. Where several apply, the largest.
A `~` line without its bound is refused: a line that took its function out
with no bound let any gain in it through.

A hot line is matched against each function's canonical name, never
callgrind's as printed: callgrind names a method `<supdb::db::Reader>::f`
and a monomorph with its generic arguments, so a list written as paths
(`supdb::db::Reader::f`) matched as substrings names none of them, and the
first version of this script compared nothing but the probes' own
functions, reached through the closure types in other functions' generic
arguments. The canonical name drops every generic argument list and reads
`<T>::f` and `<T as Tr>::f` as `T::f`; a line matches the name it equals, a name below it
(`T::f::{closure#5}`), or, ending in `::`, every name in the module or type
it opens. A probe in which no line matches a function fails, since a list
that matches nothing passes everything; with SEEN set, the lines that
matched are appended to that file, for cost.sh to name the lines no probe
of a whole run reached: a function inlined into its callers everywhere
today, or one that has gone. The comparison itself is by canonical name
too, a function's monomorphs summed, so that a closure type renamed in a
generic argument -- `part_read_z::<F, read_all::{closure#5}>` becoming
`part_read_z::<F, read_all_in::{closure#5}>` -- is one function on both
sides and not a gain beside a loss.
"""
import os
import re
import sys


# The slacks, each the one definition of itself (scripts/cost.sh passes none).
TOGGLE_SLACK = 64  # instructions over a toggled probe's whole run
WHOLE_SLACK = 0.01  # instructions an operation, a probe with no toggle
LIB_SLACK = 2.0  # instructions an operation, and
LIB_REL = 0.05  # of the function's own count, a library's under a toggle


def foreign_file(f):
    """A file no source of the build is in: glibc's (`./malloc/...`), a
    system header's or library's (`/usr/...`), or none (`???`)."""
    return f.startswith(("./", "/usr/", "???"))


def workspace_file(f):
    """A file of the workspace: callgrind prints the engine's, the suite's
    and the probe's sources by a path relative to the crate."""
    return not f.startswith(("/", "./", "???"))


def read(path):
    """name -> (instructions, files): a function's rows summed, and the
    files they are attributed to."""
    out = {}
    for line in open(path):
        m = re.match(r"\s*([\d,]+)\s+\(\s*[\d.]+%\)\s+(.*)$", line) or re.match(
            r"\s*([\d,]+)\s+(\S.*)$", line
        )
        if not m or ":" not in m.group(2):
            continue
        file, fn = m.group(2).split(":", 1)
        fn = re.sub(r"\s+\[.*\]$", "", fn.strip())
        n, files = out.get(fn, (0, set()))
        out[fn] = (n + int(m.group(1).replace(",", "")), files | {file.strip()})
    return out


def program_total(path):
    """The listing's PROGRAM TOTALS line, or None where it has none."""
    for line in open(path):
        m = re.match(r"\s*([\d,]+)\s+(?:\(\s*[\d.]+%\)\s+)?PROGRAM TOTALS\s*$", line)
        if m:
            return int(m.group(1).replace(",", ""))
    return None


def strip_generics(s):
    """Every `<...>` that follows a name or `::` taken out: the generic
    arguments. A `<` that opens a name -- `<T>::f`, `<T as Tr>::f` -- stays."""
    out = []
    i, n = 0, len(s)
    while i < n:
        if s[i] == "<" and i > 0 and (s[i - 1].isalnum() or s[i - 1] in "_:"):
            depth = 0
            while i < n:
                if s[i] == "<":
                    depth += 1
                elif s[i] == ">":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            i += 1
            continue
        out.append(s[i])
        i += 1
    return "".join(out).replace("::::", "::")


def canon(name):
    s = strip_generics(name.strip()).rstrip(":")
    m = re.match(r"^<([^<> ]+)(?: as [^<>]+)?>::(.*)$", s)
    return m.group(1) + "::" + m.group(2) if m else s


def matches(line, c):
    """Whether hot line `line` (canonical) names canonical function `c`."""
    if line.endswith("::"):
        return c.startswith(line)
    return c == line or c.startswith(line + "::")


OVER = " (over the engine)"


def key(name, own):
    """A function's key: its canonical name, or, for a function outside the
    OWN prefixes that names one of them in its generic arguments, that name
    with OVER after it, so its monomorphs over the engine's types are one
    function and the others another."""
    c = canon(name)
    if own and not c.startswith(own) and any(o in name for o in own):
        return c + OVER
    return c


def by_canon(rows, own=()):
    """A side's counts by key, a function's monomorphs summed, so a generic
    argument renamed between the two builds is the same function; and each
    key's files."""
    counts, files = {}, {}
    for f, (n, fs) in rows.items():
        c = key(f, own)
        counts[c] = counts.get(c, 0) + n
        files[c] = files.get(c, set()) | fs
    return counts, files


def hot_list(path, name, mode):
    """The hot lines, the `!` lines, and the bounds of the `~` lines that
    apply to probe `name` in `mode`: an unscoped `~` line every probe's with
    no toggle, one written `~probe,probe: path BOUND` those probes' alone."""
    lines = [l.strip() for l in open(path) if l.strip() and not l.startswith("#")]
    norm = lambda l: l if l.endswith("::") else canon(l)
    fns = [norm(l) for l in lines if l[0] not in "!~"]
    out = [norm(l[1:]) for l in lines if l[0] == "!"]
    bounds = []
    for l in lines:
        if l[0] != "~":
            continue
        m = re.match(r"^~(?:([\w,]+): +)?(\S+) +([\d.]+)$", l)
        if m is None:
            print(f"cost: the `~` line `{l}` names no bound an operation")
            sys.exit(2)
        probes, fn, bound = m.groups()
        if (mode == "whole" if probes is None else name in probes.split(",")):
            bounds.append((norm(fn), float(bound)))
    if not fns:
        print(f"cost: the hot list {path} names no function")
        sys.exit(2)
    return fns, out, bounds


def main(argv):
    if len(argv) != 8 or argv[6] not in ("toggle", "whole"):
        print(__doc__)
        return 2
    base, head, ops, hot_path, name, mode, own = argv[1:8]
    own = tuple(o for o in own.split(",") if o)
    ops = float(ops)
    hotfns, outfns, bounds = hot_list(hot_path, name, mode)
    rb, rh = read(base), read(head)
    status = 0
    for side, path, rows in (("base", base, rb), ("head", head, rh)):
        total = program_total(path)
        got = sum(n for n, _ in rows.values())
        if total is None:
            print(f"  PARTIAL: the {side} side's listing has no PROGRAM TOTALS line: {path}")
            status = 1
        elif got != total:
            print(
                f"  PARTIAL: the {side} side's functions sum to {got:,} of its "
                f"{total:,} instructions: a listing cut at a threshold leaves functions out"
            )
            status = 1
    b, fb = by_canon(rb, own if mode == "toggle" else ())
    h, fh = by_canon(rh, own if mode == "toggle" else ())
    files = {f: fb.get(f, set()) | fh.get(f, set()) for f in set(fb) | set(fh)}
    tb, th = sum(b.values()), sum(h.values())
    print(f"== {name} ({mode}): total {tb:,} -> {th:,} ({(th - tb) / ops:+.2f} an op over {ops:,.0f})")
    for side, t in (("base", tb), ("head", th)):
        if t < ops:
            print(f"  EMPTY: the {side} side collected {t:,} instructions over {ops:,.0f} operations")
            status = 1
    fns = set(b) | set(h)
    gain = {f: h.get(f, 0) - b.get(f, 0) for f in fns}
    moved = sorted(((d, f) for f, d in gain.items() if d), key=lambda x: -abs(x[0]))
    print(f"  {len(moved)} functions moved")
    for d, f in moved[:25]:
        print(f"  {d / ops:+9.3f} an op {d:+12,}  {b.get(f, 0):>14,} -> {h.get(f, 0):>14,}  {f}")
    listed = {f for f in fns if any(matches(hf, f) for hf in hotfns)}
    lib = {f for f in fns if all(foreign_file(x) for x in files[f])}
    if mode == "toggle":
        exact = fns - lib
    else:
        exact = {
            f for f in fns
            if f.startswith(own) or ("::" not in f and any(workspace_file(x) for x in files[f]))
        }
    exact = {f for f in exact | listed if not any(matches(o, f) for o in outfns)}
    libhot = {f for f in lib - exact if not any(matches(o, f) for o in outfns)} if mode == "toggle" else set()
    seen = sorted({hf for hf in hotfns for f in fns if matches(hf, f)})
    if os.environ.get("SEEN"):
        with open(os.environ["SEEN"], "a") as out:
            out.writelines(hf + "\n" for hf in seen)
    if not listed:
        print(f"  NO HOT: no line of {hot_path} names a function this probe ran")
        status = 1
    slack = TOGGLE_SLACK if mode == "toggle" else WHOLE_SLACK * ops
    # A `~` line's function is held to its own bound, the largest of the
    # lines that name it, in place of the mode's slack.
    own_bound = {}
    for f in exact:
        bs = [b for line, b in bounds if matches(line, f)]
        if bs:
            own_bound[f] = max(bs) * ops
    moved_exact = sum(1 for f in exact if gain[f])
    print(
        f"  {len(exact)} hot functions ran ({len(exact - listed)} on no line), {moved_exact} of"
        f" them moved, {sum(gain[f] for f in exact) / ops:+.3f} an op between them;"
        f" slack {slack:,.0f} instructions a function over the run,"
        f" {len(own_bound)} of them held to a bound of their own"
    )
    for f in sorted(own_bound):
        if gain[f]:
            print(f"  bound {own_bound[f] / ops:.3f} an op: {f} {gain[f] / ops:+.3f} an op")
    bad = sorted(f for f in exact if gain[f] > own_bound.get(f, slack))
    for f in bad:
        print(
            f"  HOT GAINED: {f} {gain[f] / ops:+.3f} an op, {gain[f]:+,} over the run"
            + (f" (its bound {own_bound[f] / ops:.3f} an op)" if f in own_bound else "")
        )
    libbad = sorted(
        f for f in libhot if gain[f] > LIB_SLACK * ops and gain[f] > LIB_REL * b.get(f, 0)
    )
    for f in libbad:
        print(
            f"  LIBRARY GAINED: {f} {gain[f] / ops:+.3f} an op,"
            f" {gain[f] / max(b.get(f, 0), 1):+.1%} of its count"
        )
    return 1 if bad or libbad else status


if __name__ == "__main__":
    sys.exit(main(sys.argv))
