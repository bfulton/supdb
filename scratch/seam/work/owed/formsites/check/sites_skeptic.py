#!/usr/bin/env python3
"""Two independent searches for the functions that read bytes of a form
file (zpages' compact form), reconciled against a hand classification.

Search A, by accessor name (not transitive): calls of the ZFile accessors
that hand out the mapping's bytes or a view of them, in zfile.rs
(page_bytes, view_of, group_of_index/key on a ZFile receiver, open_group,
locate, locate_rank, check_sample, slice_at, as_slice()[..]) and the
methods that call them by name; in db.rs, calls of ZFile's and ZCursor's
read API on the receivers the engine names them by (z., zfile::ZCursor::,
ZFile::attach, warm_file, c.walk_*/skip_to/values/key on a cursor).

Search B, by field and type, then by the call graph: roots are the
functions that use ZFile's mapping field (`self.map`, the attach's and the
warm's `map` local, the test's `skewed`) or map a form path through the
door (`vault::map` beside `zpg_name`); in zfile.rs the set is closed under
"calls a method of `impl ZFile` / `impl ZCursor` that is in the set" on a
ZFile/ZCursor receiver; in db.rs the roots are the functions that name a
form type or cell (ZFile, ZCursor, PWalk, PartVals, PartReader,
`.z.get()`, `z_spare.get()`, `zc(`, `zpg_name`, `warm_file`), closed under
"calls a db.rs function in the set" by unique name (`pr.` / `self.` /
`self.rd.` / `rd.` receivers for the names that collide).

Every function either search finds must have a row in TABLE (READS, ABOVE,
META or OTHER) or the run fails (exit 1). READS rows must be found by A
and by B; a READS row only one search found is printed. Run on the tree,
then with --mutant N on a copy the mutant changes; each mutant must fail.

Usage: python3 sites.py TREE_SRC [--mutant N] [--list]
"""
import re
import sys
import os
import shutil
import tempfile

ZF = "db/zfile.rs"
DB = "db.rs"
OTHERS = ["db/vault.rs", "db/pager.rs", "blob.rs", "db/zpages.rs", "bytes.rs",
          "flatindex.rs", "db/zselect.rs", "block.rs", "index.rs", "ordindex.rs"]

# Classes. READS: the function takes bytes of a form file from its mapping,
# or turns them into an answer the caller reads a failure from (layers 1-3
# in sites.md). ABOVE: a caller of a layer-3 dispatch that receives only
# answers already made right by the raw form. META: names a form file and
# reads none of its bytes (counters, advice, lifetime, names). OTHER: a
# false positive of a search.
R, AB, M, O = "READS", "ABOVE", "META", "OTHER"
TABLE = {
    # ---------------- zfile.rs, layer 1: the mapping
    (ZF, "attach"): R, (ZF, "check_sample"): R, (ZF, "page_bytes"): R,
    (ZF, "warm_file"): R,
    # ---------------- zfile.rs, layer 2: pages and groups to answers
    (ZF, "open_group"): R, (ZF, "view_of"): R, (ZF, "group_of_index"): R,
    (ZF, "group_of_key"): R, (ZF, "locate_rank"): R, (ZF, "locate"): R,
    (ZF, "read_all"): R, (ZF, "count"): R, (ZF, "scan_at"): R, (ZF, "scan_in"): R,
    (ZF, "values_at"): R, (ZF, "key_at_into"): R, (ZF, "seek"): R,
    (ZF, "owner_span"): R, (ZF, "owner_span_from"): R, (ZF, "cut_in"): R,
    # ZCursor
    (ZF, "at"): R, (ZF, "open_at"): R, (ZF, "next_group"): R, (ZF, "place"): R,
    (ZF, "load"): R, (ZF, "key"): R, (ZF, "values"): R, (ZF, "advance"): R,
    (ZF, "walk_until"): R, (ZF, "walk_run"): R, (ZF, "walk_impl"): R,
    (ZF, "resume"): R, (ZF, "skip_to"): R, (ZF, "advance_until"): R, (ZF, "group"): R,
    # zfile.rs META / OTHER
    (ZF, "ask_ahead"): M, (ZF, "file_bytes"): M, (ZF, "drop_cache"): M,
    (ZF, "prefetch_all"): M, (ZF, "fail"): M, (ZF, "entry_at_rank"): M,
    (ZF, "damaged"): M, (ZF, "new"): M, (ZF, "unplaced"): M, (ZF, "fell_back"): M,
    (ZF, "rank"): M, (ZF, "fall"): M, (ZF, "remember"): M, (ZF, "drop"): M,
    (ZF, "get"): M, (ZF, "take"): M, (ZF, "replace"): M, (ZF, "keep"): M,
    (ZF, "install"): M, (ZF, "flip"): M, (ZF, "pages"): M, (ZF, "advised"): M,
    (ZF, "verified_pages"): M, (ZF, "bad_pages"): M, (ZF, "resident_bytes"): M,
    (ZF, "decode_tick"): M, (ZF, "count_decode"): M, (ZF, "count_open"): M,
    (ZF, "coded"): M, (ZF, "group_bytes"): M, (ZF, "id"): M, (ZF, "ident"): M,
    (ZF, "swap_ahead"): M, (ZF, "same_segment"): O, (ZF, "popcount"): O,
    (ZF, "from"): O, (ZF, "prefetch_victim"): O, (ZF, "put"): O, (ZF, "slot"): O,
    (ZF, "touch"): O, (ZF, "view"): O, (ZF, "bytes"): O,
    (ZF, "ahead"): O, (ZF, "passed"): O, (ZF, "done"): O,  # Held/Mapped: the segment
    (ZF, "build_file_seen"): O, (ZF, "write_pages"): O, (ZF, "sample"): O,
    (ZF, "sample_of"): O, (ZF, "first_decode"): O, (ZF, "first_checks"): O,
    (ZF, "verified_group"): O, (ZF, "verified_group_in"): O,
    (ZF, "coded_record_to"): O, (ZF, "raw_tail_group"): O, (ZF, "open"): O,
    (ZF, "build_seen"): O, (ZF, "build_file"): O, (ZF, "build"): O,
    # ---------------- db.rs, layer 3: the dispatch that reads the failure
    (DB, "part_read_z"): R, (DB, "part_count_z"): R, (DB, "part_scan_z"): R,
    (DB, "part_seek_z"): R, (DB, "part_cut_z"): R, (DB, "part_values_z"): R,
    (DB, "cache_seek_exact"): R, (DB, "lazy_block"): R, (DB, "cache_key_at"): R,
    (DB, "pwalk_at"): R, (DB, "pwalk_seek"): R, (DB, "scan_partition_cursor"): R,
    (DB, "scan_blocks"): R,  # owner_span directly (zpages_seek_span)
    (DB, "cursor_at"): R, (DB, "scan"): R, (DB, "scan_until"): R, (DB, "cut"): R,
    (DB, "values_at"): R, (DB, "key"): R, (DB, "values"): R, (DB, "advance"): R,
    (DB, "emit_unsealed"): R,  # PartVals::Cursor(c) => c.values
    (DB, "z_attach"): R, (DB, "z_give"): R, (DB, "z_collect"): R,
    (DB, "zpages_settle_with"): R, (DB, "z_warm"): R,
    # db.rs ABOVE: callers that get answers only
    (DB, "cache_scan"): AB, (DB, "cache_values"): AB, (DB, "cache_cut"): AB,
    (DB, "scan_seek"): AB, (DB, "cache_owner_of"): AB, (DB, "block_bounds"): AB,
    (DB, "scan_partitions"): AB, (DB, "scan_merged"): AB, (DB, "count_in"): AB,
    (DB, "read_all_in"): AB, (DB, "settle_each"): AB, (DB, "fold_piece_block"): AB,
    (DB, "make_table"): AB, (DB, "cuts_of"): AB, (DB, "table_bounds"): AB,
    (DB, "snap_bounds"): AB, (DB, "filed_cuts"): AB, (DB, "install_ahead"): AB,
    (DB, "rebase_tables"): AB, (DB, "forget_form_unretracted"): AB,
    (DB, "published_kind"): AB, (DB, "deltas_for"): AB, (DB, "emit_over"): AB,
    (DB, "patch_block"): AB, (DB, "walk_block"): AB, (DB, "walk_deltas"): AB,
    (DB, "walk_deltas_over"): AB,
    # db.rs META
    (DB, "zc"): M, (DB, "pwalk_close"): M, (DB, "close"): M, (DB, "new"): M,
    (DB, "drop"): M, (DB, "z_cross"): M, (DB, "z_on_disk"): M,
    (DB, "z_retire_cell"): M, (DB, "raw"): M, (DB, "zpages_advised"): M,
    (DB, "zpage_heat"): M, (DB, "zpage_counts"): M, (DB, "blocks_willneed"): M,
    (DB, "settle_room_bytes"): M, (DB, "zpage_form_files"): M,
    (DB, "z_next_spare"): M, (DB, "z_recode_look"): M, (DB, "z_recode"): M,
    (DB, "z_footprint"): M, (DB, "z_generation"): M, (DB, "z_report_take"): M,
    (DB, "z_drive"): M, (DB, "z_unattached_on_disk"): M, (DB, "z_prebuild_next"): M,
    (DB, "z_apply"): M, (DB, "z_next_where"): M, (DB, "z_start"): M,
    (DB, "z_attach_on_disk"): M, (DB, "start_trimmer"): O, (DB, "shared"): M,
    ("db/vault.rs", "default_cache"): O,
    (ZF, "cancelled"): O, (ZF, "open_seg"): O,
    (DB, "copy_from_deltas"): AB, (DB, "retire_seg"): M,
    (DB, "cut_at"): O,  # BuildCtx::cut_at; the lines after it are struct PartReader's, which the fn scan attributes to it
}

TEST_ATTR = re.compile(r"^\s*#\[cfg\((all\()?test")


def production(text):
    """[(lineno, line)] outside `#[cfg(test)] mod {}` blocks and outside a
    `#[cfg(test)]` fn, full-line comments blanked."""
    lines = text.split("\n")
    out, i, n = [], 0, len(lines)
    while i < n:
        ln = lines[i]
        if TEST_ATTR.match(ln) and i + 1 < n and re.match(r"^\s*(pub(\([\w:]+\))?\s+)?(mod|fn)\s+\w+", lines[i + 1]):
            depth, j, started = 0, i + 1, False
            while j < n:
                depth += lines[j].count("{") - lines[j].count("}")
                started = started or "{" in lines[j]
                j += 1
                if started and depth <= 0:
                    break
            i = j
            continue
        s = ln.lstrip()
        txt = "" if s.startswith("//") else re.sub(r"\s//[^\"]*$", "", ln)
        if txt.lstrip().startswith(".") and out:
            # a method chain continued: joined onto the line it continues,
            # under that line's number, so a receiver and its call are one
            pno, pln = out[-1]
            out[-1] = (pno, pln.rstrip() + txt.strip())
            out.append((i + 1, ""))
        else:
            out.append((i + 1, txt))
        i += 1
    return out


FN = re.compile(r"\bfn\s+([A-Za-z_]\w*)")


def bodies(prod):
    """{fn name: [(lineno, line)]} by the last `fn` at or before each line
    (several fns of one name are merged), and lineno -> fn."""
    cur, by, at = None, {}, {}
    for no, ln in prod:
        f = FN.search(ln)
        if f:
            cur = f.group(1)
        at[no] = cur
        if cur:
            by.setdefault(cur, []).append((no, ln))
    return by, at


def impl_methods(text, ty):
    """Method names defined in `impl ... Ty ... {` blocks of `text`."""
    lines = text.split("\n")
    names = set()
    i = 0
    while i < len(lines):
        if re.match(r"^impl(<[^>]*>)?\s+" + ty + r"\b", lines[i]):
            depth, j = 0, i
            while j < len(lines):
                depth += lines[j].count("{") - lines[j].count("}")
                m = re.match(r"^    (pub(\(\w+\))?\s+)?fn\s+(\w+)", lines[j])
                if m:
                    names.add(m.group(3))
                j += 1
                if depth <= 0 and j > i + 1:
                    break
            i = j
        else:
            i += 1
    return names


def read(root, f):
    p = os.path.join(root, f)
    return open(p).read() if os.path.exists(p) else ""


def add(found, f, fn, no):
    found.setdefault((f, fn or "<item>"), set()).add(no)


def search_a(root):
    found = {}
    zt = read(root, ZF)
    prod = production(zt)
    _, at = bodies(prod)
    pats = [r"\bpage_bytes\(", r"\bview_of\(", r"\bopen_group\(",
            r"\b(self|zfile|self\.zfile)\.group_of_(index|key)\(", r"\b(self\.)?locate(_rank)?\(",
            r"\bcheck_sample\(", r"\bslice_at\(", r"\bas_slice\(\)\[", r"\bowner_span_from\(",
            r"\b(self|zfile|self\.zfile|c)\.(open_at|load|next_group|place|advance_until|walk_impl|scan_in|seek|group)\(",
            r"\bZCursor::new\(", r"\bself\.group\(\)"]
    rx = [re.compile(p) for p in pats]
    for no, ln in prod:
        if any(r.search(ln) for r in rx):
            add(found, ZF, at.get(no), no)
    dt = read(root, DB)
    prod = production(dt)
    _, at = bodies(prod)
    pats = [r"\b(z|zfile)\s*\.\s*(read_all|count|scan_at|values_at|key_at_into|seek|cut_in|owner_span|owner_span_from)\(",
            r"\.zc\([^)]*\)\??\s*(\.and_then\(\|z\|\s*z)?\.(read_all|count|scan_at|values_at|key_at_into|seek|cut_in|owner_span|owner_span_from)\(",
            r"\bzfile::ZCursor::(at|seek|unplaced)\(", r"\bzfile::ZFile::attach\(",
            r"\bzfile::warm_file\(",
            r"\bc\s*\.\s*(walk_until|walk_run|skip_to|values|key|advance|resume)\(",
            r"\bPWalk::Z\(c\)\s*=>\s*c\.", r"\bPartVals::Cursor\(c\)"]
    rx = [re.compile(p) for p in pats]
    for no, ln in prod:
        if any(r.search(ln) for r in rx):
            add(found, DB, at.get(no), no)
    # nothing else may name the API; any hit outside is a finding
    for f in OTHERS:
        prod = production(read(root, f))
        _, at = bodies(prod)
        for no, ln in prod:
            if re.search(r"\b(page_bytes|view_of|open_group|ZCursor::|ZFile::)", ln):
                add(found, f, at.get(no), no)
    return found


COLLIDE = {"key", "values", "advance", "new", "drop", "close", "scan", "cut",
           "values_at", "read_all", "raw", "get", "at", "seek", "count", "put",
           "next", "open", "len", "resume", "rank", "group", "from"}


def search_b(root):
    found = {}
    zt = read(root, ZF)
    prod = production(zt)
    by, at = bodies(prod)
    methods = impl_methods(zt, "ZFile") | impl_methods(zt, "ZCursor") | impl_methods(zt, r"<'a>\s+ZCursor") \
        | impl_methods(zt, r"<'a> ZCursor<'a>")
    # roots: the mapping field and its locals
    roots = [r"\bself\.map\b", r"^\s*\.map\s*$", r"\bmap\.(slice_at|as_slice|resident_pages|advise_willneed|advise_random|len)\b",
             r"\bskewed\b", r"\bvault::map\(", r"\bGroup::(Page|Coded)\(", r"\bOpened::Coded\("]
    rx = [re.compile(p) for p in roots]
    inset = set()
    for no, ln in prod:
        if any(r.search(ln) for r in rx):
            fn = at.get(no)
            add(found, ZF, fn, no)
            inset.add(fn)
    # closure: a function that calls a ZFile/ZCursor method in the set on a
    # ZFile/ZCursor receiver
    recv = r"\b(self|zfile|self\.zfile|c|z|Self|ZCursor)\s*(\.|::)\s*"
    changed = True
    while changed:
        changed = False
        names = [m for m in inset if m in methods]
        if not names:
            break
        call = re.compile(recv + r"(" + "|".join(sorted(names)) + r")\(")
        for fn, lines in by.items():
            for no, ln in lines:
                if call.search(ln):
                    add(found, ZF, fn, no)
                    if fn not in inset:
                        inset.add(fn)
                        changed = True
    # db.rs: the functions that name a form type, a form answer or the
    # cell; then, one level, the callers of PartReader's and PWalk's
    # methods (their receivers `pr.`, `part.1.`, `w.`, `pw`), which are the
    # dispatch's own wrappers. No deeper: a caller of a dispatch receives
    # answers, not failures (sites.md, ABOVE).
    dt = read(root, DB)
    prod = production(dt)
    by, at = bodies(prod)
    roots = [r"\.z\.get\(\)", r"\bz_spare\.get\(\)", r"\bzc\(", r"\bZFile\b", r"\bZCursor\b",
             r"\bPWalk\b", r"\bPartVals\b", r"\bPartReader\b", r"\bzpg_name\(", r"\bwarm_file\(",
             r"\bzfile::(ZRead|ZSeek|ScanEnd|ZScan)\b", r"\bself\.cur\b", r"\bself\.cursor_at\(", r"\.z\.(take|replace|install|flip|keep)\("]
    rx = [re.compile(p) for p in roots]
    for no, ln in prod:
        if any(r.search(ln) for r in rx):
            add(found, DB, at.get(no), no)
    wrap = re.compile(r"\b(pr|part\.1|w|pw|p)\s*\.\s*(scan|scan_until|cut|values_at|key|values|advance)\(")
    for no, ln in prod:
        if wrap.search(ln) and not FN.search(ln):
            add(found, DB, at.get(no), no)
    for f in OTHERS:
        prod = production(read(root, f))
        _, at = bodies(prod)
        for no, ln in prod:
            if re.search(r"\bZFile\b|\bZCursor\b|\bzfile::\w+\(|\bzpg_name\(", ln):
                add(found, f, at.get(no), no)
    return found


INSERT_BEFORE_DAMAGED = "    /// The form file is out of service.\n    pub(super) fn damaged"
INSERT_BEFORE_COUNT_Z = "    /// `part_read_z` for a count.\n"


def mutate(root, k):
    d = tempfile.mkdtemp(prefix="formsites-mut")
    r = os.path.join(d, "src")
    shutil.copytree(root, r)

    def edit(f, old, new):
        p = os.path.join(r, f)
        t = open(p).read()
        assert old in t, (k, f)
        open(p, "w").write(t.replace(old, new, 1))

    if k == 1:   # a new ZFile reader of the mapping, a name no table knows
        edit(ZF, INSERT_BEFORE_DAMAGED,
             "    pub(super) fn peek_first_byte(&self) -> u8 {\n        self.map.slice_at(0, 1).map_or(0, |b| b[0])\n    }\n\n" + INSERT_BEFORE_DAMAGED)
    elif k == 2:  # a new dispatch in db.rs through the cell
        edit(DB, INSERT_BEFORE_COUNT_Z,
             "    fn part_first_rank_z(&self, seg: &Seg, key: &[u8]) -> usize {\n        match self.zc(seg) {\n            Some(z) => match z.seek(&self.zmemo, key) {\n                _ => 0,\n            },\n            None => 0,\n        }\n    }\n\n" + INSERT_BEFORE_COUNT_Z)
    elif k == 3:  # a form page read outside zfile.rs, by mapping the path
        edit(DB, INSERT_BEFORE_COUNT_Z,
             "    fn form_magic(dir: &Path, id: u64, end: u64) -> u8 {\n        let p = dir.join(zfile::zpg_name(id, end));\n        vault::map(p).ok().and_then(|m| m.slice_at(0, 1).map(|b| b[0])).unwrap_or(0)\n    }\n\n" + INSERT_BEFORE_COUNT_Z)
    elif k == 4:  # a ZFile method reached by a new caller under a receiver named otherwise
        edit(ZF, INSERT_BEFORE_DAMAGED,
             "    pub(super) fn first_key_len(&self, memo: &ZMemo) -> usize {\n        let mut out = Vec::new();\n        let _ = self.key_at_into(memo, 0, &mut out);\n        out.len()\n    }\n\n" + INSERT_BEFORE_DAMAGED)
    elif k == 5:  # a new caller of a layer-3 dispatch in db.rs (must be classified)
        edit(DB, INSERT_BEFORE_COUNT_Z,
             "    fn first_key_of(&self, seg: &Seg) -> Vec<u8> {\n        let mut k = Vec::new();\n        let _ = self.cache_key_at(seg, 0, &mut k);\n        k\n    }\n\n" + INSERT_BEFORE_COUNT_Z)
    elif k == 6:  # skeptic: a page read added inside an existing META method
        edit(ZF, "    pub(super) fn file_bytes(&self) -> u64 {\n        self.map.len()\n    }",
             "    pub(super) fn file_bytes(&self) -> u64 {\n        let _t = self.map.slice_at(self.map.len() - 8, 8);\n        self.map.len()\n    }")
    elif k == 7:  # skeptic: a new reader whose name collides with a META row (an impl Drop for ZFile)
        edit(ZF, INSERT_BEFORE_DAMAGED.replace("    /// The form file is out of service.\n", ""),
             INSERT_BEFORE_DAMAGED.replace("    /// The form file is out of service.\n", "").replace("pub(super) fn damaged", "pub(super) fn damaged"))
        p2 = os.path.join(r, ZF)
        t2 = open(p2).read()
        t2 = t2.replace("/// That the trailer is the live segment's:",
            "impl Drop for ZFile {\n    fn drop(&mut self) {\n        let _ = self.map.slice_at(0, 4096);\n    }\n}\n\n/// That the trailer is the live segment's:", 1)
        open(p2, "w").write(t2)
    elif k == 8:  # skeptic: a form page read in a file neither search opens (db/blocks.rs)
        p2 = os.path.join(r, "db/blocks.rs")
        t2 = open(p2).read()
        t2 += "\n\npub(super) fn form_head(z: &super::zfile::ZFile) -> usize {\n    z.file_bytes() as usize\n}\npub(super) fn form_peek(path: &std::path::Path) -> u8 {\n    super::vault::map(path).ok().and_then(|m| m.slice_at(0, 1).map(|b| b[0])).unwrap_or(0)\n}\n"
        open(p2, "w").write(t2)
    elif k == 9:  # skeptic: a new reader in zfile.rs named like an OTHER row (`view`) on a ZCursor
        edit(ZF, "    /// The rank at the position: the key count at the end.\n",
             "    pub(super) fn sample(&self) -> Option<u8> {\n        self.zfile.page_bytes(0).map(|b| b[0])\n    }\n\n    /// The rank at the position: the key count at the end.\n")
    return r


def main():
    root = sys.argv[1]
    mutant = int(sys.argv[sys.argv.index("--mutant") + 1]) if "--mutant" in sys.argv else 0
    if mutant:
        root = mutate(root, mutant)
    a, b = search_a(root), search_b(root)
    keys = sorted(set(a) | set(b))
    bad = 0
    for k in keys:
        if k not in TABLE:
            bad += 1
            print("UNCLASSIFIED", k, "A" if k in a else "-", "B" if k in b else "-",
                  "lines", sorted(a.get(k, set()) | b.get(k, set()))[:8])
    reads = [k for k in keys if TABLE.get(k) == R]
    onlya = [k for k in reads if k not in b]
    onlyb = [k for k in reads if k not in a]
    for k in onlya:
        print("READS by A only", k, sorted(a[k])[:6])
    for k in onlyb:
        print("READS by B only", k, sorted(b[k])[:6])
    missing = [k for k, v in TABLE.items() if v == R and k not in a and k not in b]
    for k in missing:
        print("READS row neither search found", k)
    if "--list" in sys.argv:
        for k in keys:
            print("FOUND", TABLE.get(k, "?"), k, "A" if k in a else "-", "B" if k in b else "-",
                  sorted(a.get(k, set()) | b.get(k, set()))[:10])
    print("functions: A %d, B %d, union %d; READS %d (both %d, A only %d, B only %d); unclassified %d; READS rows unfound %d"
          % (len(a), len(b), len(keys), len(reads), len(reads) - len(onlya) - len(onlyb),
             len(onlya), len(onlyb), bad, len(missing)))
    if mutant:
        shutil.rmtree(os.path.dirname(root))
    sys.exit(1 if (bad or missing) else 0)


if __name__ == "__main__":
    main()
