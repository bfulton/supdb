"""The wall's reference count over a tree, with the wall check's corrections.

usage: python3 -I gen2.py TREE OUT.tsv [HITS.tsv]

Differences from plan/wall/gen.py:
- `vault::<name>` is keyed by the vault item's own name (check 2.5);
- `vault::real` is counted (check 2.3); the door list holds R0's verbs, the
  scout's nine plain ones, and the plain-store door functions added since
  682b2c09 (notes.md lists them with the reason);
- `\\bpasses\\(` is an encryption-side token (notes 5.5);
- every file under src/ is in a list: form side everything but lib.rs,
  bytes.rs and the four encryption files; bytes.rs counted against the
  cipher tokens as the seam (check 3.1).
Stages are not assigned here (stage.py does it).
"""
import json, os, re, sys, collections
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, '../../../wall'))
from lex import regions

DOOR = ('create create_rw open open_write open_append open_slot open_dir map rename hard_link '
        'remove_file names len exists is_file read sync_dir '
        # plain-store door functions added since 682b2c09
        'copy dir_exists open_rw open_lock sync_file sync_slot sync_dir_as DirSync '
        'guard guarded adopt_guard forget_guards GUARD_SUFFIX dir_ticket dir_synced_after '
        'note note_file trace_io traced_descriptors watch_dir unwatch_dir hook_dir_sync DirHook').split()

tree, out = sys.argv[1], sys.argv[2]
hitsf = sys.argv[3] if len(sys.argv) > 3 else None
cj = json.load(open(os.path.join(HERE, '../../../wall/wall_cipher.json')))
del cj['vault::<not a door verb>']
# PartWalk::Walked (zfile.rs) is a partition walk's variant, not the held build's trait.
cj['held build'] = r'(?<!::)\bWalked\b(?!\()|SegSrc::Kept'
CIPHER = {k: re.compile(v) for k, v in cj.items()}
VAULT = re.compile(r'\bvault::(\w+)')
fj = json.load(open(os.path.join(HERE, '../../../wall/wall_form.json')))
fj['passes('] = r'\bpasses\('
FORM = {k: re.compile(v) for k, v in fj.items()}
ENC = ['src/db/vault.rs', 'src/db/pager.rs', 'src/crypt.rs', 'src/db/dirkey.rs']
SEAM = ['src/bytes.rs']
EXEMPT = ['src/lib.rs']
allf = sorted(os.path.relpath(os.path.join(d, f), tree)
              for d, _, fs in os.walk(os.path.join(tree, 'src')) for f in fs if f.endswith('.rs'))
FORMF = [f for f in allf if f not in ENC + SEAM + EXEMPT]
ITEM = re.compile(r'\b(const|static|struct|enum|mod|trait|type)\s+(\w+)')

def scope_of(ent, code):
    if ent.startswith('<item'):
        m = ITEM.search(code)
        return f'{m.group(1)} {m.group(2)}' if m else ent
    return ent

rows = collections.OrderedDict()
hits = []
def scan(files, side, toks, vault):
    for f in files:
        for ln, (code, comm, test, ent) in enumerate(regions(open(os.path.join(tree, f)).read()), 1):
            region = 'test' if test else 'code'
            found = []
            for lab, rx in toks.items():
                n = len(rx.findall(code))
                if n: found.append((lab, n))
            if vault:
                c = collections.Counter(m.group(1) for m in VAULT.finditer(code) if m.group(1) not in DOOR)
                found += [('vault::' + k, n) for k, n in c.items()]
            for lab, n in found:
                sc = scope_of(ent, code)
                k = (side, f, sc, lab, region)
                rows.setdefault(k, [0, ln])[0] += n
                hits.append((f, ln, region, sc, lab, n, code.strip()[:160]))

scan(FORMF, 'form', CIPHER, True)
scan(SEAM, 'seam', CIPHER, True)
scan(ENC, 'enc', FORM, False)
with open(out, 'w') as o:
    for (side, f, sc, lab, region), (n, ln) in rows.items():
        o.write(f'{side}\t{f}\t{sc}\t{lab}\t{region}\t{n}\t{ln}\n')
if hitsf:
    with open(hitsf, 'w') as o:
        for h in hits: o.write('\t'.join(map(str, h)) + '\n')
