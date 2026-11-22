p='/home/user/supdb/scripts/seal-red.sh'; s=open(p).read()
T='\t'
anchor='EOF\n# The script\'s own red input'
lines=[
 ('-','src/db.rs','#[allow(dead_code)] struct SealRedMine; impl crate::bytes::whole::Reader for SealRedMine {}'),
 ('-','src/db.rs','#[allow(dead_code)] fn seal_red_lent_unit(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(()) }'),
 ('-','src/db/zfile.rs','#[allow(dead_code)] fn seal_red_lend_literal(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, crate::blob::lend::Token(())) }'),
 ('-','src/db.rs','#[allow(dead_code)] fn seal_red_lent_ord_literal(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(crate::ordindex::OrdLendToken(())) }'),
 ('-','src/db.rs','#[allow(dead_code)] fn seal_red_lent_stride_literal(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(crate::db::StrideLendToken(())) }'),
]
assert s.count(anchor)==1
s=s.replace(anchor,''.join(T.join(l)+'\n' for l in lines)+anchor)
open(p,'w').write(s)
