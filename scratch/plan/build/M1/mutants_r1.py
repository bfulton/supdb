import subprocess,hashlib,sys,os
ROOT='/home/user/supdb'; D=os.path.dirname(os.path.abspath(__file__)); K=D+'/keep'
RES=D+'/mutants-r1.results'
env=dict(os.environ, TMPDIR=D.replace('/M1','/tmp/M1'))
T='cargo test --profile checked '
SETTLE=T+'--test settle -- identity claim rewrite'
DUR=T+'--test durable -- a_write_that_ends_a_run_over_a_stopped_queue_with_room_is_taken'
QUIET=T+'--test fences -- a_third_merge_runs_only_in_a_stretch_of_writes_nobody_reads && '+T+'--test db -- a_commit_seals_whenever_the_sealer_is_idle'
COMMIT_ERR=("    pub fn commit(&mut self) -> Result<()> {\n        let _op = self.op();\n        if let Some(e) = self.pending_err.take() {\n            return Err(self.answer_pending(e));\n        }\n        if let Some(e) = self.shared.take_maint_err() {\n            return Err(e);\n",
            "    pub fn commit(&mut self) -> Result<()> {\n        let _op = self.op();\n        if let Some(e) = self.pending_err.take() {\n            return Err(self.answer_pending(e));\n        }\n        if let Some(e) = self.shared.take_maint_err() {\n            let _ = e;\n            return Err(err(\"another failure\"));\n")
NO_CAUSE=("""                    Err(e) => {
                        assert!(
                            e.to_string().contains(cause),
                            "{tag}: the segment work's own error, answered once: {e}"
                        );
                        break;
                    }""","""                    Err(_e) => {
                        break;
                    }""")
M=[
 ('X1 splice_run always compares (the same-partition short-circuit gone)',[('src/db.rs',[("                    && (blk.identity_of == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));\n","                    && (key_eq(blk.key(&blk.ents[c - lo]), key));\n")])],SETTLE),
 ('X2 key_eq compares the first sixteen bytes alone',[('src/db.rs',[("            && (a.len() == 16 || a[16..] == b[16..])\n","            && true\n")])],SETTLE),
 ('X3 agrees back to upstream\'s (self.identity || !other.identity)',[('src/db.rs',[("            && (!other.identity || self.identity_for(other.identity_of))\n","            && (self.identity || !other.identity)\n")])],
   SETTLE+' && '+T+'--test db -- convert identity && sh '+D+'/fuzz4.sh 5000 100 ZFUZZ_CACHE=all ZFUZZ_OPTVARY=1'),
 ('X4 head_bracket reads the sample before the rank\'s',[('src/ordindex.rs',[("        let h = self.top()[rank / TOP_STRIDE];\n","        let h = self.top()[(rank / TOP_STRIDE).saturating_sub(1)];\n")])],T+'--lib ordindex'),
 ('X5 KEYS held to the arms\' input',[('tests/source.rs',[('        "${{ inputs.keys }}",\n','        "${{ inputs.arms }}",\n')])],T+'--test source -- ci_checks_only_through_check_sh_and_runs_every_group'),
 ('X11 KEYS held to an input of another name',[('tests/source.rs',[('        "${{ inputs.keys }}",\n','        "${{ inputs.key }}",\n')])],T+'--test source -- ci_checks_only_through_check_sh_and_runs_every_group'),
 ('X6 the drain loop\'s assertion of the cause dropped (the engine right)',[('tests/durable.rs',[NO_CAUSE])],DUR),
 ('X6e the engine answers the segment work\'s failure with another error at a commit',[('src/db.rs',[COMMIT_ERR])],DUR),
 ('X6eb X6e with the assertion dropped',[('src/db.rs',[COMMIT_ERR]),('tests/durable.rs',[NO_CAUSE])],DUR),
 ('X7 the patch takes the rank without a compare exactly under a stale claim',[('src/db.rs',[("                    && (blk.identity_of == table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));\n","                    && (blk.identity_of != table.blob || key_eq(blk.key(&blk.ents[c - lo]), key));\n")])],SETTLE),
 ('X8 head_bracket\'s guard of the learned prefix\'s length dropped',[('src/ordindex.rs',[("        if rank >= self.n || !rank.is_multiple_of(TOP_STRIDE) || prefix.len() != self.pfx {\n","        if rank >= self.n || !rank.is_multiple_of(TOP_STRIDE) {\n")])],T+'--lib ordindex'),
 ('O1 seek_in_exact\'s held search takes heads equal to the query as below it',[('src/ordindex.rs',[("                if u64::from_be_bytes(*unsafe { held.get_unchecked(m) }) < h {\n","                if u64::from_be_bytes(*unsafe { held.get_unchecked(m) }) <= h {\n")])],T+'--lib ordindex && '+SETTLE),
 ('O2 seek_in_exact\'s held search leaves the window\'s last head out',[('src/ordindex.rs',[("        let a = if hi <= held.len() {\n            let (mut a, mut b) = (lo, hi);\n","        let a = if hi <= held.len() {\n            let (mut a, mut b) = (lo, hi.saturating_sub(1).max(lo));\n")])],T+'--lib ordindex && '+SETTLE),
 ('O3 the writer\'s read not counted (ModeNote)',[('src/db.rs',[("            let n = if scan { &m.scans } else { &m.points };\n            n.set(n.get() + 1);\n","            let n = if scan { &m.scans } else { &m.points };\n            let _ = n;\n")])],QUIET),
 ('O4 the writer\'s quiet left unset (note_read_mode)',[('src/db.rs',[("        note.quiet.set(quiet);\n","")])],QUIET),
 ('O5 the writer\'s quiet_out left set after its read',[('src/db.rs',[("                m.quiet_out.set(false);\n","")])],QUIET),
 ('X3b X3 against the predicate test',[('src/db.rs',[("            && (!other.identity || self.identity_for(other.identity_of))\n","            && (self.identity || !other.identity)\n")])],T+'--lib copy_claim'),
 ('O4b O4 against the quiet stretch\'s test',[('src/db.rs',[("        note.quiet.set(quiet);\n","")])],T+'--test db -- a_quiet_stretch_under_the_idle_seals_files_no_backlog'),
 ('O5b O5 against the quiet stretch\'s test',[('src/db.rs',[("                m.quiet_out.set(false);\n","")])],T+'--test db -- a_quiet_stretch_under_the_idle_seals_files_no_backlog'),
 ('O6 with_policy: supdb-cipherlog keeps the run',[('bench/src/engines.rs',[("            opts.direct_ingest = opts.direct_ingest && !cipher_log;\n","")])],'cd bench && '+T+'--lib -- the_cipher_arms_differ'),
]
only=sys.argv[1:]
done=set()
if os.path.exists(RES):
    for l in open(RES): done.add(l.split('\t')[0])
def sha(p): return hashlib.sha256(open(p,'rb').read()).hexdigest()
for name,files,cmd in M:
    tag=name.split()[0]
    if (only and tag not in only) or tag in done: continue
    origs={}
    for f,reps in files:
        p=os.path.join(ROOT,f); orig=open(p).read(); origs[p]=(f,orig); s=orig
        for a,b in reps:
            assert s.count(a)==1,(tag,a); s=s.replace(a,b)
        open(p,'w').write(s)
    try:
        r=subprocess.run(cmd,shell=True,cwd=ROOT,env=env,capture_output=True,text=True,timeout=3600)
        rc=r.returncode; out=(r.stdout+r.stderr)
    except subprocess.TimeoutExpired:
        rc='timeout'; out=''
    finally:
        for p,(f,orig) in origs.items():
            open(p,'w').write(orig)
            assert sha(p)==sha(os.path.join(K,f)),('restore',f)
    open(D+f'/mutant-r1-{tag}.log','w').write(out)
    fails=[l for l in out.split('\n') if ' FAILED' in l or 'panicked' in l or 'FAIL:' in l or 'error[' in l][:6]
    verdict='KILLED' if rc not in (0,) else 'SURVIVED'
    with open(RES,'a') as h: h.write(f"{tag}\t{verdict}\trc={rc}\t{name}\t{' | '.join(fails)[:600]}\n")
    print(tag,verdict,rc,flush=True)
