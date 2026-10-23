#!/usr/bin/env python3
"""The memo's size as an option (`Options::zpages_memo_bytes`, 8 MiB as
shipped), with arms at 16 and 32 MiB on the combined arm, for counting the
decodes E and the point-read mixes make under each (deterministic), before
any pairing prices the page cache the memo takes."""
def edit(path, pairs):
    s = open(path).read()
    for old, new in pairs:
        n = s.count(old)
        assert n == 1, (path, n, old[:90])
        s = s.replace(old, new)
    open(path, 'w').write(s)

Z = '/home/user/supdb/src/db/zfile.rs'
edit(Z, [
('''impl ZMemo {
    pub(super) fn new() -> ZMemo {
        ZMemo::of(Memo::new(ZMEMO_SLOTS, ZMEMO_BYTES))
    }
''',
 '''impl ZMemo {
    pub(super) fn new() -> ZMemo {
        ZMemo::of(Memo::new(ZMEMO_SLOTS, ZMEMO_BYTES))
    }

    /// A memo capped at `bytes` of decoded groups (`Options::zpages_memo_bytes`),
    /// its slots scaled with the cap at the shipped ratio -- a slot per two
    /// kibibytes, a group's decoded size at the suite's shape -- so the cap
    /// and not the slot count bounds what it holds, as `ZMemo::new` has it.
    pub(super) fn sized(bytes: usize) -> ZMemo {
        let slots = (bytes / (ZMEMO_BYTES / ZMEMO_SLOTS)).max(1);
        ZMemo::of(Memo::new(slots, bytes))
    }
'''),
])
D = '/home/user/supdb/src/db.rs'
s = open(D).read()
# the option, after the cut cursor's (lever C) or the trusted views' field
anchor = '''    #[doc(hidden)]
    pub zpages_cut_cursor: bool,
''' if 'pub zpages_cut_cursor: bool,' in s else '''    #[doc(hidden)]
    pub zpages_trusted_views: bool,
'''
assert s.count(anchor) == 1
s = s.replace(anchor, anchor + '''    /// The decoded bytes a handle's memo of groups holds at most
    /// (`zfile::ZMemo`), its slots scaled with it: eight mebibytes as
    /// shipped, which E's Zipfian head outgrows at five million keys (a 54%
    /// hit rate over four looks a scan). A larger memo trades the page
    /// cache a bound leaves the form files for decodes saved; the arms
    /// `supdb-cache64tofitautozpagesmemo16` and `...memo32` count the
    /// decodes under sixteen and thirty-two before any rate is read.
    #[doc(hidden)]
    pub zpages_memo_bytes: usize,
''')
d_anchor = '''            zpages_cut_cursor: true,
''' if 'zpages_cut_cursor: true,' in s else '''            zpages_trusted_views: true,
'''
assert s.count(d_anchor) == 1
s = s.replace(d_anchor, d_anchor + '''            zpages_memo_bytes: 8 << 20,
''')
# the six handles: the two that move a local `opts` read it first, the rest self.opts
n = s.count('            zmemo: zfile::ZMemo::new(),\n')
assert n == 6, n
lines = s.split('\n')
out = []
for i, line in enumerate(lines):
    if line == '            zmemo: zfile::ZMemo::new(),':
        # find the opts expression in the next 40 lines of this struct literal
        window = '\n'.join(lines[i:i+40])
        if '            opts,' in window:
            out.append('            zmemo: zfile::ZMemo::sized(opts.zpages_memo_bytes),')
        else:
            out.append('            zmemo: zfile::ZMemo::sized(self.opts.zpages_memo_bytes),')
    else:
        out.append(line)
open(D, 'w').write('\n'.join(out))
print("memo size: engine edits applied")
