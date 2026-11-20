"""Draft tests/data/couplings.txt from the export: one line per
(side, file, scope, token, region) with its count, the stage it is expected
to fall at, and a short reason. Comments are not counted. Code (strings
included) and #[cfg(test)]/#[test] code are counted on separate lines.

usage: python3 gen.py TREE > couplings.draft.txt
"""
import json, os, re, sys, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from lex import regions

tree = sys.argv[1]
here = os.path.dirname(os.path.abspath(__file__))
CIPHER = {k: re.compile(v) for k, v in json.load(open(os.path.join(here, 'wall_cipher.json'))).items()}
FORM = {k: re.compile(v) for k, v in json.load(open(os.path.join(here, 'wall_form.json'))).items()}
FORM_FILES = ['src/db.rs', 'src/db/zfile.rs', 'src/db/zpages.rs', 'src/db/zselect.rs', 'src/db/zlz4.rs',
              'src/db/blocks.rs', 'src/ordindex.rs', 'src/flatindex.rs', 'src/blob.rs', 'src/block.rs',
              'src/index.rs', 'src/format.rs', 'src/reserve.rs']
ENC_FILES = ['src/db/vault.rs', 'src/db/pager.rs', 'src/crypt.rs', 'src/db/dirkey.rs']
ITEM = re.compile(r'\b(const|static|struct|enum|mod|trait|type)\s+(\w+)')

def scope_of(ent, code):
    if ent.startswith('<item'):
        m = ITEM.search(code)
        return f'{m.group(1)} {m.group(2)}' if m else ent
    return ent

# Stage by (file, scope[, token]). First match wins; '*' matches any.
def stage(f, scope, tok, region):
    if region == 'test':
        if f == 'src/db/vault.rs':
            return 'S3/S6', 'vault unit tests by stored name; rewritten or die with the envelope arm (design.md 10, test table)'
        if f == 'src/db.rs':
            return 'stays', 'cipher_looks: the plain/cipher look words of the plumbing'
        return '?', ''
    if f == 'src/db.rs':
        S = {
            'mod pager': ('stays', 'module declaration'),
            'mod dirkey': ('stays', 'module declaration'),
            'mod vault': ('stays', 'module declaration'),
            'Encryption::<item>': ('stays', 'the public mode enum'),
            'Db::<item>': ('stays', 'Db.vault field: the open store under a cipher'),
            'Shared::<item>': ('stays', 'Shared.pager: the cache the read path pins'),
            'struct PagerGrace': ('stays', 'the reader table offered to the pager as its grace'),
            'PagerGrace::<item>': ('stays', 'impl pager::Grace'),
            'Readers::page_idle': ('stays', 'pager::debug_pinned hook in the reader table'),
            'Readers::page_back': ('stays', 'pager::debug_pinned hook'),
            'Readers::pin': ('stays', 'pager::debug_pinned hook'),
            'Readers::unpin': ('stays', 'pager::debug_pinned hook'),
            'Db::cipher_counts': ('stays', 'public counters'),
            'const ATTN_PAGED': ('stays', 'the handle bit (CLAUDE.md, the plain store price)'),
            'const REST_HERE': ('stays', 'the forms word under a cipher'),
            'const AHEAD_REST': ('stays', 'the builder-ahead stop word'),
            'Wal::<item>': ('S6', 'Wal.enc: Option<vault::LogCipher>'),
            'const WAL_SEALED_LEN': ('S6', 'sealed log header length'),
            'Wal::begin': ('S6', 'log_begin under vault::under'),
            'Wal::resume': ('S6', 'log_resume'),
            'Wal::open_append': ('S6', 'sealed header read'),
            'Wal::write': ('S6', 'enc.seal_into'),
            'Wal::replay': ('S6', 'open_records'),
            'manifest_image': ('S6', 'v.persist + seal_manifest (tokens part goes at S3)'),
            'manifest_open': ('S6', 'open_manifest + adopt_all (tokens part goes at S3)'),
            'cipher_controller': ('S5', 'controller off under a cipher'),
            'Seg::records_resident': ('G1', 'paged_resident through the byte source (R3)'),
            'RunMap::<item>': ('S1', 'vault::Mapped becomes the door\'s mapped backing (design 2); add to the door list then'),
            'RunMap::open': ('S1', 'vault::open_mapped becomes a door verb (design 2)'),
            'Direct::<item>': ('S4', 'Direct.logged'),
            'const RUN_LOGGED': ('S4', ''),
            'MemTable::logged': ('S4', ''),
            'Db::append_pinned': ('S4', 'self.vault.is_some() picks a logged table'),
            'Db::commit_direct': ('S4', 'log_run'),
            'Db::log_run': ('S4', ''),
            'Db::log_run_again': ('S4', ''),
            'Db::run_attended': ('S4', ''),
            'Db::run_again': ('S4', ''),
            'Emitter::leave': ('S3*', 'vault::persist_one'),
            'Db::close_run': ('S3*', 'vault::persist'),
            'Db::seal_table': ('S3*', 'vault::persist'),
            'Db::z_on_disk': ('S3*', 'vault::companion'),
            'Db::z_attach': ('S3*', 'vault::companion'),
            'compact_job': ('S1/R4', 'vault::hold picks MergeRest: pin rename, then the lease'),
            'tier_run': ('S1/R4', 'vault::hold picks MergeRest'),
            'Maint::z_sample_start': ('S1', 'vault::hold -> pin::hold'),
            'trait MergeRest': ('R4', 'the lease'),
            'MergeRest::<item>': ('R4', 'the lease'),
            'struct NoRest': ('R4', 'Free'),
            'NoRest::<item>': ('R4', 'Free'),
            'Hold::<item>': ('R4', 'impl MergeRest for &pager::Hold'),
            'merge_ranks': ('R4', 'R: MergeRest'),
            'merge_keys': ('R4', 'R: MergeRest'),
            'struct Unlent': ('S1', 'pin::idle guard'),
            'Unlent::<item>': ('S1', 'pin::idle guard'),
            'Reader::unlent': ('S1', 'pin::idle'),
            'Reader::unlend': ('S1', 'pin::idle'),
        }
        if scope in ('compact_run', 'tier_walk'):
            if tok == 'MergeRest/NoRest':
                return 'R4', 'R: MergeRest'
            return 'S1+S3*', 'vault::check -> pin::verify (S1); vault::persist (S3*)'
        if scope in ('Options::<item>', 'Options::default'):
            return ('S5', 'zpages_under_cipher') if tok == 'cipher' else ('stays', 'Options.encryption')
        if scope in ('Db::create', 'Db::open'):
            return 'stays', 'the open names the mode; S5 takes one cipher token (cipher_controller call)'
        if scope in S:
            return S[scope]
        if tok == 'unlent/unlend':
            return 'S1', 'a wait under no pin: pin::idle (R6)'
        if scope.startswith('Reader::'):
            return 'stays', 'Reader read path: ATTN_PAGED, rests, pager waits (forms.md R4: runtime split stays)'
        return '?', ''
    if f == 'src/db/zfile.rs':
        Z = {
            'ZFile::attach': ('S1', 'vault::hold -> pin::hold'),
            'open_seg': ('R4', 'vault::kept picks the held build (design 6 renames it vault::paged at S3; R0 bars that too)'),
            'build_file_seen': ('R4+S3*', 'SegSrc::Kept arm: vault::hold, the cipher error string (R4); vault::persist_form (S3*)'),
            'trait Walked': ('R4', 'Walked::{Mapped,Held} folded into one build generic over the lease'),
            'Walked::<item>': ('R4', ''),
            'Mapped::<item>': ('R4', 'impl Walked for Mapped'),
            'Held::<item>': ('R4', 'impl Walked for Held'),
            'struct Held': ('R4', 'Held.hold: &pager::Hold'),
            'write_pages': ('R4', 'W: Walked'),
        }
        if scope in Z:
            return Z[scope]
        return '?', ''
    if f == 'src/ordindex.rs':
        return 'G1', 'piecewise picks the CRC skip and the lazy top (R2; ordindex.rs:413-428)'
    if f == 'src/blob.rs':
        return 'stays', 'the dispatch on the source kind (R2 keeps it in blob)'
    if f == 'src/db/vault.rs':
        V = {
            'passes': ('S6', 'the log, the spare and the manifest pass by name'),
            'default_cache': ('G3', 'zfile::read_budget: the budget moves behind Memory (R12)'),
            'Vault::seal_manifest': ('S6', ''), 'Vault::open_manifest': ('S6', ''), 'slot_aad': ('S6', ''),
            'Vault::log_begin': ('S6', ''), 'Vault::log_resume': ('S6', ''), 'log_aad': ('S6', ''),
            'const RECORD_HEAD': ('S6', ''), 'struct LogCipher': ('S6', ''), 'LogCipher::<item>': ('S6', ''),
            'LogCipher::new': ('S6', ''), 'LogCipher::seal_into': ('S6', ''), 'LogCipher::open_records': ('S6', ''),
        }
        if scope in V:
            return V[scope]
        return 'S3*', 'envelope naming by kind, the persist and companion cascade, the sweep rule'
    return '?', ''

rows = collections.OrderedDict()
def scan(files, toks, side):
    for f in files:
        src = open(os.path.join(tree, f)).read()
        for ln, (code, comm, test, ent) in enumerate(regions(src), 1):
            region = 'test' if test else 'code'
            for lab, rx in toks.items():
                n = len(rx.findall(code))
                if n:
                    sc = scope_of(ent, code)
                    k = (side, f, sc, lab, region)
                    if k not in rows:
                        rows[k] = [0, ln]
                    rows[k][0] += n

scan(FORM_FILES, CIPHER, 'form')
scan(ENC_FILES, FORM, 'enc')
print('# side\tfile\tscope\ttoken\tregion\tcount\tstage\tfirst line\treason')
for (side, f, sc, lab, region), (n, ln) in rows.items():
    st, why = stage(f, sc, lab, region)
    print(f'{side}\t{f}\t{sc}\t{lab}\t{region}\t{n}\t{st}\t{ln}\t{why}')
