import re, sys, collections
# The file advice of a run, normalized so two runs compare: madvise and
# mincore are placed in the file mapping they fall in (by the mmaps traced
# with -y) and kept as (file, offset in the file, len, advice); calls on
# anonymous memory are dropped. fadvise64 is (file, offset, len, advice);
# openat (path, flags, ok). Paths have digits folded.
maps = {}  # start -> (end, file, file_off)
c = collections.Counter()
def fold(p):
    p = re.sub(r'.*/(supdb-next-[^/]*)', r'\1', p)
    return re.sub(r'\d+', 'N', p)
def drop_overlap(a, b):
    for s0 in [s0 for s0, (e0, _, _) in maps.items() if s0 < b and a < e0]:
        del maps[s0]
def place(addr):
    for s, (e, f, o) in maps.items():
        if s <= addr < e:
            return f, addr - s + o
    return None
pending = {}
def lines(path):
    # strace -f splits a call another thread interrupts into an
    # "<unfinished ...>" line and a "<... name resumed>" line: joined here.
    for line in open(path):
        line = line.rstrip('\n')
        m = re.match(r'^(\d+)\s+(.*) <unfinished \.\.\.>$', line)
        if m:
            pending[m.group(1)] = m.group(2)
            continue
        m = re.match(r'^(\d+)\s+<\.\.\. \w+ resumed>(.*)$', line)
        if m:
            yield m.group(1) + ' ' + pending.pop(m.group(1), '') + m.group(2)
            continue
        yield line
for line in lines(sys.argv[1]):
    line = line.strip()
    m = re.match(r'^\d+\s+(\w+)\((.*)\)\s*=\s*(-?\w+)', line)
    if not m:
        continue
    name, args, rc = m.groups()
    if name == 'mmap':
        a = [x.strip() for x in args.split(',')]
        fm = re.match(r'(\d+)<(.*)>', a[4])
        if fm and not rc.startswith('-'):
            start = int(rc, 16)
            drop_overlap(start, start + int(a[1]))
            maps[start] = (start + int(a[1]), fold(fm.group(2)), int(a[5], 0))
        elif not rc.startswith('-'):
            start = int(rc, 16)
            drop_overlap(start, start + int(a[1]))
        continue
    if name == 'munmap':
        a = [x.strip() for x in args.split(',')]
        if not rc.startswith('-'):
            st = int(a[0], 16)
            drop_overlap(st, st + int(a[1]))
        continue
    a = [x.strip() for x in args.split(',')]
    if name in ('madvise', 'mincore'):
        p = place(int(a[0], 16))
        if p is None:
            continue
        key = (name, p[0], str(p[1]), a[1], a[2] if name == 'madvise' else '', rc)
    elif name == 'fadvise64':
        fm = re.match(r'(\d+)<(.*)>', a[0])
        f = fold(fm.group(2)) if fm else a[0]
        key = ('fadvise64', f, a[1], a[2], a[3], rc)
    elif name == 'openat':
        p = fold(a[1])
        if '/proc/' in p or '/sys/' in p or '.so' in p or '/etc/' in p:
            continue
        key = ('openat', p, a[2], 'ok' if not rc.startswith('-') else rc)
    else:
        continue
    c[key] += 1
for k in sorted(c):
    print(c[k], *k)
