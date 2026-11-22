import subprocess,hashlib,sys,os,json
ROOT='/home/user/supdb'; D=os.path.dirname(os.path.abspath(__file__)); K=D+'/keep'
RES=D+'/mutants.results'
env=dict(os.environ, TMPDIR=D.replace('/M1','/tmp/M1'))
T='cargo test --profile checked '
M=[
 ('M1 the fix taken out whole: identity_for ignores the id, the patch takes any claim','src/db.rs',
  [('        self.identity && self.identity_of == blob\n','        self.identity\n'),
   ('&& (blk.identity_of == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));','&& true;')],
  T+'--test settle --test db -- identity claim rewrite'),
 ('M2 the patch\'s compare removed (any claim taken by rank)','src/db.rs',
  [('&& (blk.identity_of == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));','&& true;')],
  T+'--test settle -- identity claim rewrite'),
 ('M3 the patch\'s compare inverted','src/db.rs',
  [('&& (blk.identity_of == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));','&& (blk.identity_of == table.blob || !key_eq(blk.key(&blk.ents[c - lo]), key));')],
  T+'--test settle -- identity claim rewrite'),
 ('M4 identity_for ignores the id (walk start, agrees, block_cache_identity)','src/db.rs',
  [('        self.identity && self.identity_of == blob\n','        self.identity\n')],
  T+'--lib copy_claim && '+T+'--test settle --test db -- identity claim rewrite'),
 ('M5 copy_block stamps no partition (identity_of 0)','src/db.rs',
  [('            identity: false,\n            identity_of: src.seg.blob.id(),\n','            identity: false,\n            identity_of: 0,\n')],
  T+'--test settle --test db -- identity claim rewrite'),
 ('M6 head_bracket reads the eager top level (upstream\'s field) under a mapping by the piece','src/ordindex.rs',
  [('        let h = self.top()[rank / TOP_STRIDE];\n        let Some(above)','        let h = self.top_eager[rank / TOP_STRIDE];\n        let Some(above)')],
  T+'--lib ordindex'),
 ('M7 the settle stores\' names back to the colliding one','tests/settle.rs',
  [('claim_across_rewrite("copy-claim-rewrite", 4096','claim_across_rewrite("copy-identity-rewrite", 4096')],
  'for i in 1 2 3 4 5; do '+T+'--test settle || exit 1; done'),
 ('M8 copy_entry_at takes its own partition\'s claim alone (upstream\'s hint)','src/db.rs',
  [('    (blk.identity && j < blk.ents.len()).then_some((blk, j))','    (blk.identity_for(table.blob) && j < blk.ents.len()).then_some((blk, j))')],
  T+'--test settle'),
 ('M9 KEYS taken off the workflow test\'s environment list','tests/source.rs',
  [('''    (
        "KEYS",
        "${{ inputs.keys }}",
        "the quick group's keys, numbered or ids, as the person who dispatched the run named them",
    ),
''','')],
  T+'--test source -- ci_checks_only_through_check_sh_and_runs_every_group'),
 ('M10 piece_bounds taken off cost-entries','tests/data/cost-entries.txt',
  [("Reader::piece_bounds\t-\ta test seam or a counter: reads or sets the store's bookkeeping, at no key, entry, block or step of an operation\n",'')],
  T+'--test source -- every_entry_point_is_counted'),
 ('M11 the raise for block_cache_identity undeclared','scripts/raises.txt',
  [('tests/data/cost-entries.txt\tReader::block_cache_identity\t0\t1\t','#gone\t')],
  'SUPDB_BASE=HEAD sh scripts/check.sh lists'),
]
only=sys.argv[1:]
done=set()
if os.path.exists(RES):
    for l in open(RES):
        done.add(l.split('\t')[0])
def sha(p): return hashlib.sha256(open(p,'rb').read()).hexdigest()
for name,f,reps,cmd in M:
    tag=name.split()[0]
    if (only and tag not in only) or tag in done: continue
    p=os.path.join(ROOT,f); orig=open(p).read(); s=orig
    for a,b in reps:
        assert s.count(a)==1,(tag,a); s=s.replace(a,b)
    open(p,'w').write(s)
    try:
        r=subprocess.run(cmd,shell=True,cwd=ROOT,env=env,capture_output=True,text=True,timeout=3000)
        rc=r.returncode; out=(r.stdout+r.stderr)
    except subprocess.TimeoutExpired:
        rc='timeout'; out=''
    finally:
        open(p,'w').write(orig)
        assert sha(p)==sha(os.path.join(K,f)),('restore',f)
    log=D+f'/mutant-{tag}.log'; open(log,'w').write(out)
    fails=[l for l in out.split('\n') if ' FAILED' in l or 'panicked' in l or 'RED' in l or 'error[' in l][:6]
    verdict='KILLED' if rc not in (0,) else 'SURVIVED'
    with open(RES,'a') as h: h.write(f"{tag}\t{verdict}\trc={rc}\t{name}\t{' | '.join(fails)[:600]}\n")
    print(tag,verdict,rc,flush=True)
