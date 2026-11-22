import subprocess,hashlib,sys,os
ROOT='/home/user/supdb'; D=os.path.dirname(os.path.abspath(__file__)); K=D+'/keep'
RES=D+'/mutants-r2.results'
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
QS=T+'--test db -- a_quiet_stretch_under_the_idle_seals_files_no_backlog'
CA='cd bench && '+T+'--lib -- the_cipher_arms_differ the_dirkey_arm'
NOTE_A=("            if m.quiet_out.get() {\n                m.quiet_out.set(false);\n                self.shared\n                    .write_quiet\n","            m.quiet_out.set(false);\n            if m.quiet_out.get() {\n                self.shared\n                    .write_quiet\n")
NOTE_B=("            if m.quiet_out.get() {\n                m.quiet_out.set(false);\n                self.shared\n                    .write_quiet\n","            let was = m.quiet_out.get();\n            m.quiet_out.set(false);\n            if was {\n                self.shared\n                    .write_quiet\n")
NOTE_C=("            n.set(n.get() + 1);\n            // The first read since a quiet commit ends the write-only\n            // stretch for the segment work (`Shared::write_quiet`), once.\n            if m.quiet_out.get() {\n","            // The first read since a quiet commit ends the write-only\n            // stretch for the segment work (`Shared::write_quiet`), once.\n            if m.quiet_out.get() {\n                n.set(n.get() + 1);\n")
ENC_A=("""        if cipher {
            opts.zpages_under_cipher = cipher_auto;
            opts.direct_ingest = opts.direct_ingest && !cipher_log;
        }
""","")
ENC_B=("        let db = if REOPEN.with(|r| r.get()) {\n","""        if matches!(opts.encryption, supdb::Encryption::Cipher(_)) {
            opts.zpages_under_cipher = cipher_auto;
            opts.direct_ingest = opts.direct_ingest && !cipher_log;
        }
        let db = if REOPEN.with(|r| r.get()) {
""")
M=[
 ('E3a note_read: quiet_out cleared ahead of its test (the test then never true)',[('src/db.rs',[NOTE_A])],QUIET+' && '+QS),
 ('E3b note_read: quiet_out stored false at every read (the test on the value before)',[('src/db.rs',[NOTE_B])],QUIET+' && '+QS),
 ('E4 with_policy: the cipher block after the encryption, guarded on the field',[('bench/src/engines.rs',[ENC_A,ENC_B])],CA),
 ('E7 with_policy: the dirkey arm given no encryption',[('bench/src/engines.rs',[("            (true, _) => supdb::Encryption::Kernel(supdb::DirKey::new([0x5a; 32])),\n","            (true, _) => supdb::Encryption::None,\n")])],CA),
 ('E7b E7 against the encryption test added',[('bench/src/engines.rs',[("        (true, _) => supdb::Encryption::Kernel(supdb::DirKey::new([0x5a; 32])),\n","        (true, _) => supdb::Encryption::None,\n")])],CA),
 ('E4b E4 again after the refactor (equivalent by construction)',[('bench/src/engines.rs',[ENC_A,ENC_B])],CA),
 ('E9b E9 against the budget held: the cipher arms given no cache bound',[('bench/src/engines.rs',[("                bytes => c.cache_bytes(bytes),\n","                _ => c,\n")])],CA),
 ('E8 note_read: the writer\'s read counted only on the first after a quiet commit',[('src/db.rs',[NOTE_C])],QUIET+' && '+QS),
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
    open(D+f'/mutant-r2-{tag}.log','w').write(out)
    fails=[l for l in out.split('\n') if ' FAILED' in l or 'panicked' in l or 'FAIL:' in l or 'error[' in l][:6]
    verdict='KILLED' if rc not in (0,) else 'SURVIVED'
    with open(RES,'a') as h: h.write(f"{tag}\t{verdict}\trc={rc}\t{name}\t{' | '.join(fails)[:600]}\n")
    print(tag,verdict,rc,flush=True)
