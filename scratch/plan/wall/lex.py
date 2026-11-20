"""A small Rust lexer for counting tokens by region.

For each source line it yields: code text (comments removed, string literal
contents kept), comment text, whether the line is inside a #[cfg(test)] item
or a #[test] fn, and the enclosing fn as Impl::name (or <item> at top level).

Handled: // and /* */ (nested) comments, doc comments, "..." strings with
escapes, b"..", r#".."# / br#".."# raw strings, char literals vs lifetimes,
braces only outside strings/comments/chars.
"""
import re, sys

def lex(src):
    """Return list of (code_chars, comment_chars) per line, and a list of
    (line, kind, text) code tokens with the string contents kept."""
    n = len(src)
    i = 0
    line = 0
    code = [[]]
    comm = [[]]
    blank = [[]]
    def put(ch, target, lit=False):
        nonlocal line
        if ch == '\n':
            line += 1
            code.append([]); comm.append([]); blank.append([])
        else:
            (code if target == 'c' else comm)[line].append(ch)
            if target == 'c':
                blank[line].append(' ' if lit else ch)
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i+1] == '/':
            while i < n and src[i] != '\n':
                put(src[i], 'm'); i += 1
            continue
        if c == '/' and i + 1 < n and src[i+1] == '*':
            depth = 0
            while i < n:
                if src.startswith('/*', i):
                    depth += 1; put('/', 'm'); put('*', 'm'); i += 2; continue
                if src.startswith('*/', i):
                    depth -= 1; put('*', 'm'); put('/', 'm'); i += 2
                    if depth == 0: break
                    continue
                put(src[i], 'm'); i += 1
            continue
        # raw strings r"..", r#".."#, br#".."#
        m = re.compile(r'b?r(#*)"').match(src, i)
        if m and (i == 0 or not (src[i-1].isalnum() or src[i-1] == '_')):
            hashes = m.group(1)
            end = '"' + hashes
            j = src.find(end, m.end())
            if j < 0: j = n
            for ch in src[i:j+len(end)]:
                put(ch, 'c', True)
            i = j + len(end)
            continue
        if c == '"' or (c == 'b' and i + 1 < n and src[i+1] == '"' and (i == 0 or not (src[i-1].isalnum() or src[i-1]=='_'))):
            if c == 'b':
                put('b', 'c', True); i += 1
            put('"', 'c', True); i += 1
            while i < n and src[i] != '"':
                if src[i] == '\\' and i + 1 < n:
                    put(src[i], 'c', True); put(src[i+1], 'c', True); i += 2; continue
                put(src[i], 'c', True); i += 1
            if i < n: put('"', 'c', True); i += 1
            continue
        if c == "'":
            # char literal or lifetime
            if i + 1 < n and src[i+1] == '\\':
                j = src.find("'", i + 2)
                # handle '\''
                if src[i+2:i+3] == "'": j = src.find("'", i + 3)
                for ch in src[i:j+1]: put(ch, 'c', True)
                i = j + 1; continue
            if i + 2 < n and src[i+2] == "'":
                for ch in src[i:i+3]: put(ch, 'c', True)
                i += 3; continue
            put(c, 'c'); i += 1; continue
        put(c, 'c'); i += 1
    return [''.join(x) for x in code], [''.join(x) for x in comm], [''.join(x) for x in blank]

STR_RE = re.compile(r'b?r#*"|b?"(\\.|[^"\\])*"')

def blank_strings(s):
    """Code text with string contents blanked, for brace and keyword matching."""
    out = []
    i = 0
    while i < len(s):
        m = re.compile(r'b?r(#*)"').match(s, i)
        if m and (i == 0 or not (s[i-1].isalnum() or s[i-1]=='_')):
            end = '"' + m.group(1)
            j = s.find(end, m.end())
            if j < 0: j = len(s) - len(end)
            out.append('""'); i = j + len(end); continue
        if s[i] == '"':
            j = i + 1
            while j < len(s) and s[j] != '"':
                j += 2 if s[j] == '\\' else 1
            out.append('""'); i = j + 1; continue
        out.append(s[i]); i += 1
    return ''.join(out)

FN_RE = re.compile(r'\bfn\s+(\w+)')
IMPL_RE = re.compile(r'^\s*(?:unsafe\s+)?impl\b(?:\s*<[^{]*?>)?\s*([^{;]*)')
MOD_RE = re.compile(r'\bmod\s+(\w+)')
MAC_RE = re.compile(r'\bmacro_rules!\s*(\w+)')
TRAIT_RE = re.compile(r'\btrait\s+(\w+)')
TYPE_RE = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|union)\s+(\w+)')

def impl_name(head):
    head = head.strip()
    # impl Trait for Type -> Type ; impl Type -> Type
    if ' for ' in head:
        tr, ty = head.split(' for ', 1)
        ty = ty.strip()
    else:
        ty = head
    ty = re.sub(r'<.*', '', ty).strip()
    ty = re.sub(r'\bwhere\b.*', '', ty).strip()
    ty = ty.split('::')[-1]
    return ty or '?'

def regions(src):
    """Per line: (code, comment, is_test, enclosing)."""
    code, comm, flat = lex(src)
    stack = []   # (kind, name, depth_at_open, test)
    depth = 0
    pending = None   # (kind, name)
    pt_depth = 0
    pb = 0
    pt_pb = 0
    pending_test = False   # a #[cfg(test)] / #[test] attribute seen, item not opened
    out = []
    for ln, s in enumerate(flat):
        # attributes on this line
        if re.search(r'#\[\s*cfg\s*\(\s*test\s*\)\s*\]', s) or re.search(r'#\[\s*cfg\s*\(\s*all\s*\([^\]]*\btest\b', s) or re.search(r'#\[\s*test\s*\]', s):
            pending_test = True
            pt_depth = depth
            pt_pb = pb
        s = re.sub(r'#!?\[(?:[^\[\]]|\[[^\[\]]*\])*\]', lambda m: ' ' * len(m.group(0)), s)
        in_test_at_start = any(t for (_, _, _, t) in stack) or pending_test
        ent_at_start = enclosing(stack)
        j = 0
        tokpos = []
        for m in FN_RE.finditer(s): tokpos.append((m.start(), 'fn', m.group(1)))
        for m in IMPL_RE.finditer(s): tokpos.append((m.start(), 'impl', impl_name(m.group(1))))
        for m in MOD_RE.finditer(s): tokpos.append((m.start(), 'mod', m.group(1)))
        for m in MAC_RE.finditer(s): tokpos.append((m.start(), 'macro', m.group(1)))
        for m in TRAIT_RE.finditer(s): tokpos.append((m.start(), 'trait', m.group(1)))
        for m in TYPE_RE.finditer(s): tokpos.append((m.start(), 'type', m.group(1)))
        tokpos.sort()
        k = 0
        for pos, ch in enumerate(s):
            while k < len(tokpos) and tokpos[k][0] <= pos:
                pending = (tokpos[k][1], tokpos[k][2]); k += 1
            if ch in '([':
                pb += 1
            elif ch in ')]':
                pb = max(0, pb - 1)
            elif ch == '{':
                depth += 1
                if pending:
                    stack.append((pending[0], pending[1], depth, pending_test))
                    pending = None; pending_test = False
                elif pending_test and depth == pt_depth + 1 and pb == pt_pb:
                    stack.append(('block', None, depth, True))
                    pending_test = False
                else:
                    stack.append(('block', None, depth, False))
            elif ch == '}':
                if stack and stack[-1][2] == depth:
                    stack.pop()
                depth -= 1
                if pending_test and depth < pt_depth:
                    pending_test = False
            elif ch == ',' and pending_test and pb == pt_pb and not pending and depth == pt_depth:
                pending_test = False
            elif ch == ';' and pb == (pt_pb if pending_test else 0):
                if pending and pending[0] in ('fn', 'mod', 'type'):
                    # fn decl without body, or `mod x;`
                    pending = None
                    if depth == 0 or (stack and stack[-1][0] in ('mod','impl','trait')):
                        pending_test = False
                elif pending_test and not pending and depth == pt_depth:
                    # a #[cfg(test)] use/const/static; ends at ;
                    pending_test = False
                    out_test_line = True
        in_test = in_test_at_start or any(t for (_, _, _, t) in stack)
        ent = enclosing(stack) if not enclosing(stack).startswith('<item') else ent_at_start
        if pending and pending[0] == 'fn':
            base = enclosing(stack)
            ent = (base.split('::<item>')[0] + '::' if '::<item>' in base else (base + '>' if not base.startswith('<item') else '')) + pending[1]
        out.append((code[ln], comm[ln], in_test, ent))
    return out

def enclosing(stack):
    fn = None; impl = None
    for kind, name, _, _ in stack:
        if kind == 'fn' and fn is None: fn = name
        elif kind == 'fn': fn = fn + '>' + name
        elif kind in ('impl', 'trait', 'type') and fn is None: impl = name
        elif kind == 'macro' and fn is None: fn = name + '!'
    if fn is None and impl is None:
        mods = [n for k, n, _, _ in stack if k == 'mod']
        return '<item' + (':' + '::'.join(mods) if mods else '') + '>'
    if fn is None: return impl + '::<item>'
    return (impl + '::' if impl else '') + fn

if __name__ == '__main__':
    for p in sys.argv[1:]:
        for i, (c, m, t, e) in enumerate(regions(open(p).read())):
            print(f'{p}:{i+1}\t{"T" if t else "-"}\t{e}\t{c.strip()[:80]}')
