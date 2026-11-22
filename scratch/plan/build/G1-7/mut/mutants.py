# Each mutant: (name, file, old, new, killer command). Applied one at a time,
# the killer run, the verdict appended to results.txt, the file put back from
# keep/final and checked by its sum.
M = [
 ("M1 blob::lend::Token::new widened to pub(crate)", "src/blob.rs",
  "        pub(in crate::blob) fn new() -> Token {", "        pub(crate) fn new() -> Token {",
  "sh scripts/seal-red.sh"),
 ("M2 bytes::Base's field widened to pub(crate)", "src/bytes.rs",
  "pub(crate) struct Base(());", "pub(crate) struct Base(pub(crate) ());",
  "sh scripts/seal-red.sh"),
 ("M3 sample: a wildcard arm for the records by the piece", "src/db/zfile.rs",
  "        Some(Records::Pieces(recs, dir)) => {\n            let images = PieceImages {",
  "        #[allow(unreachable_patterns)]\n        _ => Err(super::err(\"mutant\")),\n        #[allow(unreachable_patterns)]\n        Some(Records::Pieces(recs, dir)) => {\n            let images = PieceImages {",
  "cargo test --profile checked --test zpages -- cipher"),
 ("M3b the kept build: a wildcard arm in place of the records by the piece", "src/db/zfile.rs",
  "                Some(Records::Pieces(recs, words)) => (recs, words),\n",
  "                #[allow(unreachable_patterns)]\n                _ => return Err(super::err(\"mutant\")),\n                #[allow(unreachable_patterns)]\n                Some(Records::Pieces(recs, words)) => (recs, words),\n",
  "cargo test --profile checked --test zpages -- cipher"),
 ("M4 the warm's touch dropped", "src/db/zfile.rs",
  "            sum = sum.wrapping_add(map.touch(off as u64, span));\n", "",
  "cargo test --profile checked --lib -- a_warm_reads_the_cold_file_once_and_skips_a_resident_one"),
 ("M5 touch bounded by the file and not the lendable length (a paged mapping touched)", "src/bytes.rs",
  "        let end = off.saturating_add(len as u64).min(self.lend);\n        let mut sum = 0u64;",
  "        let end = off.saturating_add(len as u64).min(self.len);\n        let mut sum = 0u64;",
  "cargo test --profile checked --lib -- a_kept_file_lives_in_memory_and_reaches_the_directory_as_an_envelope"),
 ("M6 records(): the arm by the piece dropped", "src/blob.rs",
  "        let (sec, idx) = self.pieces()?;\n        idx.regions(sec).map(|(r, d)| Records::Pieces(r, d))",
  "        let _ = self.pieces()?;\n        None",
  "cargo test --profile checked --test zpages -- cipher"),
 ("M7 records_by_descriptor: a place for a reader by the piece", "src/db.rs",
  "            crate::blob::Records::Pieces(..) => None,",
  "            crate::blob::Records::Pieces(..) => raw.record_region_place().map(|(at, len)| (at, len as u64)),",
  "cargo test --profile checked --test zpages -- cipher"),
 ("M8 Paging made pub", "src/bytes.rs",
  "pub(crate) trait Paging: Send + Sync {", "pub trait Paging: Send + Sync {",
  "cargo test --profile checked --doc"),
 ("M9 MmapBytes::lent made pub", "src/bytes.rs",
  "    pub(crate) fn lent(&self) -> &[u8] {", "    pub fn lent(&self) -> &[u8] {",
  "cargo test --profile checked --doc"),
 ("M10 MmapBytes::lend_at bounded by the file (a paged mapping lent unasked)", "src/bytes.rs",
  "        let end = off.checked_add(len as u64)?;\n        if end > self.lend {\n            return None;\n        }",
  "        let end = off.checked_add(len as u64)?;\n        if end > self.len {\n            return None;\n        }",
  "cargo test --profile checked --test cipher -- a_store_larger_than_its_cache_reads_as_itself_within_it"),
]
import subprocess, sys, hashlib, os, shutil
root="/home/user/supdb"; keep=os.path.dirname(os.path.abspath(__file__))+"/../keep/final"
res=os.path.dirname(os.path.abspath(__file__))+"/results.txt"
only=sys.argv[1:]
env=dict(os.environ, TMPDIR="/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-7")
def sha(p): return hashlib.sha256(open(p,'rb').read()).hexdigest()
for name,f,old,new,cmd in M:
    if only and name.split()[0] not in only: continue
    p=os.path.join(root,f); s=open(p).read()
    if s.count(old)!=1:
        open(res,'a').write(f"{name}\tNOT APPLIED (old text found {s.count(old)} times)\n"); continue
    open(p,'w').write(s.replace(old,new))
    log=os.path.dirname(res)+"/"+name.split()[0]+".log"
    r=subprocess.run(cmd,shell=True,cwd=root,env=env,stdout=open(log,'w'),stderr=subprocess.STDOUT)
    verdict="KILLED" if r.returncode!=0 else "SURVIVED"
    shutil.copy(os.path.join(keep,f),p)
    ok = sha(p)==sha(os.path.join(keep,f))
    open(res,'a').write(f"{name}\t{verdict} (exit {r.returncode}) by: {cmd}\tput back: {'sum ok' if ok else 'SUM MISMATCH'}\n")
    if not ok: sys.exit(3)
