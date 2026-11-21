#!/usr/bin/env python3
"""The lists that only go down, held across a change (scripts/ratchet.sh).

    python3 scripts/ratchet.py ROOT BASE

ROOT is the repository whose tree is checked (its working tree, uncommitted
edits included); BASE a commit of it, and the lists are compared as they
stand at the merge-base of HEAD and BASE. Each list of the REGISTRY is read
into keys and a number a key, a list at a time by what the list is for, and
a key whose number is higher in the tree than at the base -- a key new since
the base counting from zero -- is growth. Growth fails unless the change
declares it in scripts/raises.txt, exactly: list, key, from, to, reason.

What fails, exit 1, the last line `ratchet: RED`:
  - growth no declaration names;
  - a declaration not at the base that names no growth observed, or names
    one with other numbers (a declaration is matched whole, never in part);
  - a registered list the base had and the tree lacks, or one neither has;
  - a path the base's registry holds (its scripts/ratchet.py, read by the
    keys of its REGISTRY block) that the tree's REGISTRY does not, whether
    deleted or moved to NOT_HELD: a list is let go of by no change, as it is
    not taken away (a base with no scripts/ratchet.py holds no registry;
    one whose REGISTRY the check cannot read is exit 2);
  - a text file under tests/data (valid UTF-8, no NUL byte: any extension)
    the registry does not name, held or not held, and a NOT_HELD entry for a
    file that is not there.
A list new since the base has nothing to hold yet, and is said so. A list
or a declaration file that does not parse, or a base that is no commit, is
exit 2. The verdict prints every raise it let through.

A declaration present at the base, the same line, is spent: it was the
raise of a change already made, and it holds nothing now, so it is printed
as spent and never matched; a change after a merged raise is therefore not
red for it, whether the line stays or goes. Spent lines are removed by any
later change, at will (removing one is never growth); the next change that
edits scripts/raises.txt is the natural one. Lines are counted as a
multiset, so the same raise made again later is a second line.

A key is a string and a number an integer, so a check that holds some other
quantity -- a group of functions held to its net instructions, say -- is a
reader in REGISTRY and its raises lines of the same file, with no second
mechanism.
"""

import os
import re
import subprocess
import sys
from collections import Counter

RAISES = "scripts/raises.txt"


def rows(text, path, least):
    """The data lines of a tab-separated list: (line number, columns)."""
    for n, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        c = line.split("\t")
        if len(c) < least:
            raise ValueError(f"{path} line {n}: fewer than {least} columns: {line}")
        yield n, c


def number(path, n, s):
    try:
        v = int(s)
    except ValueError:
        raise ValueError(f"{path} line {n}: a count that is not a number: {s}")
    if v < 0:
        raise ValueError(f"{path} line {n}: a negative count: {s}")
    return v


def couplings(text, path):
    # file scope token region count stage reason: the wall's gate holds the
    # totals by (file, token, region), so a coupling moved between scopes is
    # no growth, and a total that rises, or a new key, is a new coupling.
    out = Counter()
    for n, c in rows(text, path, 6):
        if c[3] not in ("code", "test"):
            raise ValueError(f"{path} line {n}: a region that is neither code nor test")
        out[f"{c[0]} {c[2]} {c[3]}"] += number(path, n, c[4])
    return out


def door(text, path):
    # kind file scope count reason: each count is an exemption from the
    # door's lint at that place (an allow, a /proc path, threads, code the
    # door's build compiles out). An allow moved to another scope is an
    # allow where there was none, so a key is the whole (kind, file, scope)
    # and a rise in any one of them is growth, whatever fell elsewhere.
    out = {}
    for n, c in rows(text, path, 5):
        if c[0] not in ("allow", "literal", "thread", "cfgout"):
            raise ValueError(f"{path} line {n}: a kind the door does not have: {c[0]}")
        k = f"{c[0]} {c[1]} {c[2]}"
        if k in out:
            raise ValueError(f"{path} line {n}: a key listed twice: {k}")
        out[k] = number(path, n, c[3])
    return out


def door_libc(text, path):
    # name reason: a libc call with a string argument said to touch no file,
    # and so not barred. Each name is one exemption; a new name is growth.
    out = {}
    for n, c in rows(text, path, 2):
        if not c[1].strip():
            raise ValueError(f"{path} line {n}: a name with no reason")
        if c[0] in out:
            raise ValueError(f"{path} line {n}: {c[0]} listed twice")
        out[c[0]] = 1
    return out


def cost_entries(text, path):
    # Type::fn probes | Type::fn - reason: an entry point counted by a probe
    # is held by the cost job; one with a reason in place of a probe is an
    # exemption from it. A key is 1 where the line gives a reason, 0 where
    # it names probes, so a new reason, or a probe given up for one, is
    # growth, and a new entry point with a probe is not.
    out = {}
    for n, c in rows(text, path, 2):
        if c[0] in out:
            raise ValueError(f"{path} line {n}: {c[0]} listed twice")
        if c[1] == "-":
            if len(c) < 3 or not c[2].strip():
                raise ValueError(f"{path} line {n}: {c[0]} gives no reason")
            out[c[0]] = 1
        else:
            out[c[0]] = 0
    return out


# Every list that only goes down: its path, how a line's key and number are
# read, and why it is held.
REGISTRY = {
    "tests/data/couplings.txt": (
        couplings,
        "the wall (tests/source.rs): each coupling between the forms and the encryption modules is the owner's to raise",
    ),
    "tests/data/door.txt": (
        door,
        "the door (tests/source.rs): each place the door's lint is silenced, or cannot reach, is an exemption",
    ),
    "tests/data/door-libc.txt": (
        door_libc,
        "the door (tests/source.rs): each libc call with a string left unbarred is an exemption",
    ),
    "tests/data/cost-entries.txt": (
        cost_entries,
        "the cost job (tests/source.rs, scripts/cost.sh): each entry point with a reason in place of a probe is uncounted",
    ),
}

# Text files under tests/data that are no list held here, each with why.
NOT_HELD = {}


def git(root, *args):
    return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True)


def at_base(root, base, path):
    r = git(root, "cat-file", "-e", f"{base}:{path}")
    if r.returncode != 0:
        return None
    r = git(root, "show", f"{base}:{path}")
    if r.returncode != 0:
        raise ValueError(f"{path} at {base}: {r.stderr.strip()}")
    return r.stdout


def in_tree(root, path):
    p = os.path.join(root, path)
    if not os.path.isfile(p):
        return None
    with open(p, encoding="utf-8") as f:
        return f.read()


def text_files(root, top="tests/data"):
    out = []
    for d, _, fs in os.walk(os.path.join(root, top)):
        for f in fs:
            p = os.path.join(d, f)
            with open(p, "rb") as h:
                b = h.read()
            if b"\0" in b:
                continue
            try:
                b.decode("utf-8")
            except UnicodeDecodeError:
                continue
            out.append(os.path.relpath(p, root).replace(os.sep, "/"))
    return sorted(out)


def declarations(text):
    """(list, key, from, to, reason) lines of scripts/raises.txt."""
    out = []
    for n, c in rows(text or "", RAISES, 5):
        lst, key, a, b, why = c[0], c[1], c[2], c[3], "\t".join(c[4:])
        if lst not in REGISTRY:
            raise ValueError(f"{RAISES} line {n}: {lst} is no list of the registry")
        a, b = number(RAISES, n, a), number(RAISES, n, b)
        if b <= a:
            raise ValueError(f"{RAISES} line {n}: from {a} to {b} is no raise")
        if not why.strip():
            raise ValueError(f"{RAISES} line {n}: a raise with no reason")
        out.append((lst, key, a, b, why))
    return out


REGISTRY_FILE = "scripts/ratchet.py"


def registered(text, where):
    """The paths a registry holds, read off a ratchet.py's text: the keys
    of its `REGISTRY = {` block, one `    "path": (` line each. A change that
    takes a path out of the tree's REGISTRY is read against the base's
    with this, so it is held to reading the tree's own file as REGISTRY
    is (main)."""
    m = re.search(r"^REGISTRY = \{\n(.*?)^\}", text, re.S | re.M)
    if not m:
        raise ValueError(f"{REGISTRY_FILE} at {where} has no REGISTRY the check can read")
    return set(re.findall(r'^    "([^"]+)": \($', m.group(1), re.M))


def main(root, base):
    r = git(root, "merge-base", "HEAD", base)
    if r.returncode != 0:
        print(f"ratchet: no merge-base of HEAD and '{base}': {r.stderr.strip()}")
        return 2
    mb = r.stdout.strip()
    print(f"ratchet: the lists at {mb} (the merge-base of HEAD and {base}) against the tree")
    red = []
    with open(__file__, encoding="utf-8") as h:
        if registered(h.read(), "the tree") != set(REGISTRY):
            raise ValueError(f"{REGISTRY_FILE}: the registry's reader (registered) does not read this file's REGISTRY as Python does")
    held = at_base(root, mb, REGISTRY_FILE)
    if held is not None:
        for p in sorted(registered(held, "the base") - set(REGISTRY)):
            red.append(f"{p}: the base's registry ({REGISTRY_FILE}) holds it and the tree's does not: a list that only goes down is not let go of, by NOT_HELD or by deleting its entry")
    for f in text_files(root):
        if f not in REGISTRY and f not in NOT_HELD:
            red.append(f"{f}: a text file under tests/data the registry (scripts/ratchet.py) does not name: register it with how its growth is read, or say in NOT_HELD why it is no list")
    for f in NOT_HELD:
        if in_tree(root, f) is None:
            red.append(f"{f}: NOT_HELD names a file the tree does not have")
    growth = {}
    for path, (read, _why) in sorted(REGISTRY.items()):
        t, b = in_tree(root, path), at_base(root, mb, path)
        if t is None and b is None:
            red.append(f"{path}: registered, and neither the base nor the tree has it")
            continue
        if t is None:
            red.append(f"{path}: the base has it and the tree does not: a list that only goes down is not taken away")
            continue
        tv = read(t, path)
        if b is None:
            print(f"  {path}: new since the base, nothing to hold yet ({len(tv)} keys)")
            continue
        bv = read(b, path)
        up = 0
        for k, n in sorted(tv.items()):
            if n > bv.get(k, 0):
                growth[(path, k, bv.get(k, 0), n)] = None
                up += 1
        print(f"  {path}: held, {len(bv)} keys at the base, {len(tv)} in the tree, {up} raised")
    tree_d = Counter(declarations(in_tree(root, RAISES)))
    base_d = Counter(declarations(at_base(root, mb, RAISES)))
    fresh = tree_d - base_d
    for d in sorted((tree_d & base_d).elements()):
        print(f"  spent (at the base): {d[0]} {d[1]} {d[2]} -> {d[3]}")
    let = []
    for d in sorted(fresh.elements()):
        g = (d[0], d[1], d[2], d[3])
        if g in growth and growth[g] is None:
            growth[g] = d[4]
            let.append(d)
            continue
        seen = [x for x in growth if x[0] == d[0] and x[1] == d[1]]
        what = f"; observed {seen[0][2]} -> {seen[0][3]}" if seen else "; nothing observed"
        red.append(f"{RAISES}: declares {d[0]} {d[1]} {d[2]} -> {d[3]}, which matches no raise{what}")
    for g, why in sorted(growth.items(), key=lambda x: x[0]):
        if why is None:
            red.append(f"{g[0]}: {g[1]}: {g[2]} -> {g[3]}, a raise {RAISES} does not declare (list, key, from, to, reason)")
    for d in let:
        print(f"ratchet: raise let through: {d[0]} {d[1]} {d[2]} -> {d[3]}: {d[4]}")
    for e in red:
        print(f"ratchet: {e}")
    print("ratchet: RED" if red else "ratchet: green")
    return 1 if red else 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: ratchet.py ROOT BASE")
        sys.exit(2)
    try:
        sys.exit(main(sys.argv[1], sys.argv[2]))
    except (ValueError, UnicodeDecodeError) as e:
        print(f"ratchet: {e}")
        print("ratchet: RED")
        sys.exit(2)
