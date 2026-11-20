"""Count token occurrences by file, token and region.

usage: count.py ROOT TOKENS.json OUT_PREFIX FILE...
TOKENS.json: {"label": "python regex", ...}
Regions: code (outside tests), comment (outside tests), tcode, tcomment.
Code text keeps string literal contents; comments include doc comments.
Writes OUT_PREFIX.hits.tsv (file, line, region, enclosing fn, label, text)
and prints a table file x label x region.
"""
import json, os, re, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lex import regions

root, tokf, outp = sys.argv[1:4]
files = sys.argv[4:]
toks = json.load(open(tokf))
rx = {k: re.compile(v) for k, v in toks.items()}
hits = []
table = {}
for f in files:
    src = open(os.path.join(root, f)).read()
    for ln, (code, comm, test, ent) in enumerate(regions(src), 1):
        for part, text in (('code', code), ('comment', comm)):
            region = ('t' if test else '') + part
            for lab, r in rx.items():
                n = len(r.findall(text))
                if n:
                    table.setdefault((f, lab), {}).setdefault(region, 0)
                    table[(f, lab)][region] += n
                    hits.append((f, ln, region, ent, lab, n, text.strip()[:160]))
with open(outp + '.hits.tsv', 'w') as o:
    for h in hits:
        o.write('\t'.join(map(str, h)) + '\n')
R = ['code', 'comment', 'tcode', 'tcomment']
print('file\ttoken\t' + '\t'.join(R))
for (f, lab) in sorted(table):
    d = table[(f, lab)]
    print(f'{f}\t{lab}\t' + '\t'.join(str(d.get(r, 0)) for r in R))
