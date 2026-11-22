p = '/home/user/supdb/scripts/seal-red.sh'
s = open(p).read()
T = '\t'
old = ('-' + T + 'src/db/zfile.rs' + T +
       '#[allow(dead_code)] fn seal_red_as_slice(m: &crate::bytes::MmapBytes) -> &[u8] { m.as_slice() }\nEOF\n')
lines = [
    ('src/ordindex.rs', 'src/db.rs', '#[allow(dead_code)] fn seal_red_lent_ord(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(crate::ordindex::lend::Token::new()) }'),
    ('src/ordindex.rs', 'src/blob.rs', '#[allow(dead_code)] fn seal_red_lent_ord_blob(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(crate::ordindex::lend::Token::new()) }'),
    ('src/db/zfile.rs', 'src/db.rs', '#[allow(dead_code)] fn seal_red_lent_stride(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(crate::db::StrideLendToken::new()) }'),
    ('src/db/zfile.rs', 'src/ordindex.rs', '#[allow(dead_code)] fn seal_red_lent_stride_ord(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent(crate::db::StrideLendToken::new()) }'),
    ('-', 'src/db.rs', '#[allow(dead_code)] fn seal_red_lent_bare(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent() }'),
    ('-', 'src/db/zfile.rs', '#[allow(dead_code)] fn seal_red_lend_default(m: &crate::bytes::MmapBytes) -> Option<&[u8]> { crate::bytes::Bytes::lend_at(m, 0, 1, Default::default()) }'),
    ('-', 'src/db.rs', '#[allow(dead_code)] fn seal_red_lent_ord_default(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent::<crate::ordindex::OrdLendToken>(Default::default()) }'),
    ('-', 'src/db.rs', '#[allow(dead_code)] fn seal_red_lent_stride_default(m: &crate::bytes::MmapBytes) -> &[u8] { m.lent::<crate::db::StrideLendToken>(Default::default()) }'),
    ('-', 'src/db/pager.rs', '#[allow(dead_code)] fn seal_red_base_default(p: &dyn crate::bytes::Paging) -> *const u8 { p.base(Default::default()) }'),
]
new = old[:-len('EOF\n')] + ''.join(T.join(l) + '\n' for l in lines) + 'EOF\n' + '''# The script's own red input (scripts/seal-red-red.sh) adds lines of its
# own, in the same form, to show that each check below fails.
if [ -n "${SEAL_RED_EXTRA:-}" ]; then
  cat "$SEAL_RED_EXTRA" >> "$copy/lines" || fail "no $SEAL_RED_EXTRA"
fi
'''
assert s.count(old) == 1
s = s.replace(old, new)
o2 = '''# paged mapping (`Paging::base`) takes a token only `bytes` can make
# (`bytes::Base`, whose field is private); and `MmapBytes::as_slice` is gone.'''
n2 = '''# paged mapping (`Paging::base`) takes a token only `bytes` can make
# (`bytes::Base`, whose field is private); a mapping as it lies
# (`MmapBytes::lent`) takes a token only its readers make, the ordered
# index (`ordindex::lend`) and the stride form (`db::zfile::lend`); no
# token is had by `Default::default()`; and `MmapBytes::as_slice` is gone.
# A token made by any other route, and a new caller inside a reader's own
# module, are held by tests/source.rs
# (`the_seals_tokens_are_made_only_where_they_are_spent`).'''
assert s.count(o2) == 1
s = s.replace(o2, n2)
open(p, 'w').write(s)
