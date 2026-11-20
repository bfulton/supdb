#!/usr/bin/env python3
"""Classify the tests of supdb's export for the conform pass (read only).

For every #[test] fn: does it open a store (Db::create / Db::open),
does it read or write the store's files by path, does it set
Options::encryption. Features are taken transitively through the helper
fns of the same file and of tests/common (call graph by name, an
over-approximation: any word-boundary occurrence of a helper's name).

Usage: classify.py ROOT FILE... [--detail]
"""
import re, sys, os, json

def strip_comments(src):
    """Blank out comments, keep strings and chars (offsets preserved)."""
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if src.startswith('//', i):
            j = src.find('\n', i)
            j = n if j < 0 else j
            for k in range(i, j): out[k] = ' '
            i = j
        elif src.startswith('/*', i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith('/*', j): depth += 1; j += 2
                elif src.startswith('*/', j): depth -= 1; j += 2
                else: j += 1
            for k in range(i, j):
                if out[k] != '\n': out[k] = ' '
            i = j
        elif c == 'r' and re.match(r'r#*"', src[i:i+10]) and (i == 0 or not (src[i-1].isalnum() or src[i-1] == '_')):
            m = re.match(r'r(#*)"', src[i:])
            hashes = m.group(1)
            end = src.find('"' + hashes, i + len(m.group(0)))
            i = (end + 1 + len(hashes)) if end >= 0 else n
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == '\\' else 1
            i = j + 1
        elif c == "'":
            m = re.match(r"'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'])'", src[i:i+14])
            i = i + len(m.group(0)) if m else i + 1
        else:
            i += 1
    return ''.join(out)

def code_only(src):
    """Blank strings too, for brace matching."""
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == 'r' and re.match(r'r#*"', src[i:i+10]) and (i == 0 or not (src[i-1].isalnum() or src[i-1] == '_')):
            m = re.match(r'r(#*)"', src[i:])
            hashes = m.group(1)
            end = src.find('"' + hashes, i + len(m.group(0)))
            end = (end + 1 + len(hashes)) if end >= 0 else n
            for k in range(i, end):
                if out[k] != '\n': out[k] = ' '
            i = end
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == '\\' else 1
            for k in range(i, min(j + 1, n)):
                if out[k] != '\n': out[k] = ' '
            i = j + 1
        elif c == "'":
            m = re.match(r"'(\\(x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'])'", src[i:i+14])
            if m:
                for k in range(i, i + len(m.group(0))): out[k] = ' '
                i += len(m.group(0))
            else:
                i += 1
        else:
            i += 1
    return ''.join(out)

FN = re.compile(r'\bfn\s+([A-Za-z_][A-Za-z0-9_]*)')

def fns(src_nc, code):
    """[(name, start, body_start, body_end, is_test, line)]"""
    res = []
    for m in FN.finditer(code):
        # body: first '{' after the signature at paren depth 0, unless ';' first
        j, depth = m.end(), 0
        while j < len(code):
            ch = code[j]
            if ch == '(' or ch == '[': depth += 1
            elif ch == ')' or ch == ']': depth -= 1
            elif ch == ';' and depth <= 0: j = -1; break
            elif ch == '{' and depth <= 0: break
            j += 1
        if j < 0 or j >= len(code):
            continue
        b, d = j, 0
        k = j
        while k < len(code):
            if code[k] == '{': d += 1
            elif code[k] == '}':
                d -= 1
                if d == 0: break
            k += 1
        # attributes before the fn
        pre = code[max(0, m.start() - 600):m.start()]
        # attributes: walk back over lines that are attributes / qualifiers
        lines = pre.split('\n')
        attrs = []
        for ln in reversed(lines[:-1] if lines else []):
            s = ln.strip()
            if s.startswith('#[') or s == '' or s.endswith(')]') or s.endswith(']'):
                if s: attrs.append(s)
                continue
            break
        last = lines[-1].strip() if lines else ''
        is_test = any(a.startswith('#[test]') for a in attrs)
        line = src_nc.count('\n', 0, m.start()) + 1
        res.append((m.group(1), m.start(), b, k + 1, is_test, line))
    return res

FEAT = {
    'opens': re.compile(r'\bDb::(create|open)\s*\('),
    'enc': re.compile(r'\bencryption\b|\bEncryption::|\bStoreCipher\b'),
    # fs calls on files inside a store (not the temp dir's own make/remove)
    'fs': re.compile(r'std::fs::(read|write|read_dir|metadata|OpenOptions|remove_file|copy|File|read_to_string|rename|read_link|symlink_metadata|hard_link|set_permissions)\b'),
    'names': re.compile(r'"(seg-|pcs-|par-|wal-|spare-|ord-|direct-|zpg-|x-)(["0-9{]|dead)|"manifest(["._{]|-?prev)|"supdb\.cipher|"[^"\s]*(\.sup|\.oidx|\.zpg|\.tmp|\.o|\.z)"'),
    'bytes': re.compile(r'std::fs::(read\b|write|OpenOptions|copy|File|remove_file|rename|hard_link|set_permissions)|std::fs::read_to_string\((?!"/proc)'),
    'list': re.compile(r'std::fs::(read_dir|metadata|symlink_metadata|read_link)\b|\.exists\(\)'),
    'mapfile': re.compile(r'\b(MmapBytes::open|Blob::open|SparseBlob::open)\b|/proc/self/maps|mincore'),
}

# fns whose fs call is not on a store's files: /dev/stderr
BENIGN = {'watchdog_exit', 'oracle_in'}

def analyse(path, common_tab=None):
    src = open(path).read()
    nc = strip_comments(src)
    # a temp directory's own name is not a store file's: blank the literal
    # handed to the file's dir helper, keeping offsets
    nc = re.sub(r'(\bdir\(\s*(?:&?format!\(\s*)?)("[^"\n]*")',
                lambda m: m.group(1) + '"' + ' ' * (len(m.group(2)) - 2) + '"', nc)
    code = code_only(nc)
    fl = fns(nc, code)
    # direct features per fn (body text, comments stripped, strings kept)
    direct = {}
    for (name, st, b, e, t, line) in fl:
        body = nc[b:e]
        direct.setdefault(name, []).append({k: bool(r.search(body)) for k, r in FEAT.items()})
    return src, nc, code, fl, direct

def transitive(fl, nc, helpers_feat, local_names, common_names):
    """Features of every fn, closed through name references."""
    feat = {}
    refs = {}
    for (name, st, b, e, t, line) in fl:
        body = nc[b:e]
        f = {k: bool(r.search(body)) for k, r in FEAT.items()}
        if name in BENIGN: f['fs'] = False
        rs = set()
        for h in local_names | common_names:
            if h == name: continue
            if re.search(r'(?<![A-Za-z0-9_])' + re.escape(h) + r'(?![A-Za-z0-9_])', body):
                rs.add(h)
        feat[(name, st)] = f
        refs[(name, st)] = rs
    # helper features by name (union over same-name fns, non-test)
    byname = {}
    for (name, st, b, e, t, line) in fl:
        if t: continue
        byname.setdefault(name, []).append((name, st))
    changed = True
    while changed:
        changed = False
        for key, f in feat.items():
            for h in refs[key]:
                srcs = [feat[k] for k in byname.get(h, [])]
                if h in helpers_feat: srcs.append(helpers_feat[h])
                for g in srcs:
                    for k in f:
                        if g[k] and not f[k]:
                            f[k] = True; changed = True
    return feat, refs

def main():
    root = sys.argv[1]
    files = [a for a in sys.argv[2:] if not a.startswith('--')]
    detail = '--detail' in sys.argv
    # tests/common first
    cpath = os.path.join(root, 'tests/common/mod.rs')
    _, cnc, ccode, cfl, _ = analyse(cpath)
    cnames = {f[0] for f in cfl}
    cfeat, _ = transitive(cfl, cnc, {}, cnames, set())
    common_feat = {}
    for (name, st, b, e, t, line) in cfl:
        g = cfeat[(name, st)]
        common_feat[name] = {k: common_feat.get(name, {}).get(k, False) or g[k] for k in g}
    out = {}
    for rel in files:
        p = os.path.join(root, rel)
        src, nc, code, fl, _ = analyse(p)
        local = {f[0] for f in fl if not f[4]}
        uses_common = 'mod common' in nc
        cn = cnames if uses_common or rel.startswith('src/') is False else set()
        if not uses_common: cn = set()
        feat, refs = transitive(fl, nc, common_feat if uses_common else {}, local, cn)
        tests = [(f, feat[(f[0], f[1])]) for f in fl if f[4]]
        row = {
            'tests': len(tests),
            'opens': sum(1 for _, g in tests if g['opens']),
            'fs_or_names': sum(1 for _, g in tests if g['opens'] and (g['fs'] or g['names'] or g['mapfile'])),
            'fs': sum(1 for _, g in tests if g['opens'] and g['fs']),
            'names': sum(1 for _, g in tests if g['opens'] and g['names']),
            'mapfile': sum(1 for _, g in tests if g['opens'] and g['mapfile']),
            'enc': sum(1 for _, g in tests if g['enc']),
            'opens_noenc': sum(1 for _, g in tests if g['opens'] and not g['enc']),
            'optout': sum(1 for _, g in tests if g['opens'] and not g['enc'] and (g['fs'] or g['names'] or g['mapfile'])),
            'conform_runs': sum(1 for _, g in tests if g['opens'] and not g['enc'] and not (g['fs'] or g['names'] or g['mapfile'])),
            'direct_create': len(re.findall(r'\bDb::create\s*\(', nc)),
            'direct_open': len(re.findall(r'\bDb::open\s*\(', nc)),
        }
        out[rel] = row
        if '--helpers' in sys.argv:
            for (name, st, b, e, t, line) in fl:
                if t: continue
                body = nc[b:e]
                d = {k: bool(r.search(body)) for k, r in FEAT.items()}
                if any(d.values()):
                    reach = sum(1 for (f, g) in tests if name in refs[(f[0], f[1])])
                    tag = ''.join(k[0] if d[k] else '.' for k in ['opens', 'enc', 'fs', 'names', 'mapfile', 'bytes', 'list'])
                    print(f'H {rel}:{line}\t{tag}\tdirect-refs={reach}\t{name}')
        if detail:
            for (f, g) in tests:
                tag = ''.join(k[0] if g[k] else '.' for k in ['opens', 'enc', 'fs', 'names', 'mapfile', 'bytes', 'list'])
                print(f'{rel}:{f[5]}\t{tag}\t{f[0]}')
    print(json.dumps(out, indent=1))

main()
