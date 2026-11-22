import subprocess,hashlib,sys,os
ROOT='/home/user/supdb'; D=os.path.dirname(os.path.abspath(__file__)); K=D+'/keep'
RES=D+'/mutants-r3.results'
env=dict(os.environ, TMPDIR='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/M1')
T='cargo test --profile checked '
CA='cd bench && '+T+'--lib -- the_cipher_arms_differ the_dirkey_arm'
E='bench/src/engines.rs'
LOG=("            if cipher_log {\n                opts.direct_ingest = false;\n            }\n")
M=[
 ('B3 encryption_of: (true, _) -> (true, false), (true, true) given the cipher',[(E,[("        (true, _) => supdb::Encryption::Kernel(","        (true, false) => supdb::Encryption::Kernel("),("        (false, true) => {\n            let c = supdb::StoreCipher","        (_, true) => {\n            let c = supdb::StoreCipher")])],CA),
 ('B4 with_policy: the cipher_log line dropped',[(E,[(LOG,"")])],CA),
 ('B7 with_policy: direct_ingest set from cipher_log alone (the && operand, since taken out: if cipher_log -> if true)',[(E,[("            if cipher_log {\n                opts.direct_ingest = false;","            if true {\n                opts.direct_ingest = false;")])],CA),
 ('B8 encryption_of: 0 => c -> 0 => c.cache_bytes(1 << 30)',[(E,[("                0 => c,\n","                0 => c.cache_bytes(1 << 30),\n")])],CA),
 ('B8b B8 against the test as round 2 left it (the new budget check taken out)',[(E,[("                0 => c,\n","                0 => c.cache_bytes(1 << 30),\n"),("        assert!(\n            arm.abs_diff(own) <= arm.max(own) / 4,","        assert!(\n            true || arm.abs_diff(own) <= arm.max(own) / 4,")])],CA),
 ('X1 with_policy: if cipher_log -> if !cipher_log',[(E,[("            if cipher_log {\n                opts.direct_ingest = false;","            if !cipher_log {\n                opts.direct_ingest = false;")])],CA),
 ('X2 with_policy: direct_ingest = false -> = true under cipher_log',[(E,[("            if cipher_log {\n                opts.direct_ingest = false;","            if cipher_log {\n                opts.direct_ingest = true;")])],CA),
 ('X3 encryption_of: 0 => c -> 0 => c.cache_bytes(64 << 20)',[(E,[("                0 => c,\n","                0 => c.cache_bytes(64 << 20),\n")])],CA),
 ('X4 the new budget check: the bare store under the arm\'s own encryption_of (the check made equivalent)',[(E,[("                encryption: Encryption::Cipher(supdb::StoreCipher::new(Gcm::new(), Gcm::of)),","                encryption: encryption_of(false, true, 0),")])],CA),
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
        assert sha(p)==sha(os.path.join(K,f)),('before',f)
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
    open(D+f'/mutant-r3-{tag}.log','w').write(out)
    fails=[l for l in out.split('\n') if ' FAILED' in l or 'panicked' in l or 'FAIL:' in l or 'error[' in l][:6]
    verdict='KILLED' if rc not in (0,) else 'SURVIVED'
    with open(RES,'a') as h: h.write(f"{tag}\t{verdict}\trc={rc}\t{name}\t{' | '.join(fails)[:600]}\n")
    print(tag,verdict,rc,flush=True)
