#!/usr/bin/env python3
r"""The options ledger: one CSV row per option, mechanical columns only.

    python3 release/tools/options_ledger.py > release/options.csv
    python3 release/tools/options_ledger.py --sites Options.sync >/dev/null
    python3 release/tools/options_ledger.py --summary release/options.csv

Run from the repository root. Standard library only; it reads the files git
tracks in the working tree (`git ls-files`, or a walk where git is absent)
and builds nothing. Two runs over one tree print the same bytes. What was
parsed is counted on stderr, and the tool exits 1 where a field, a default,
a struct literal or an arm's policy cannot be placed. `--sites` prints, on
stderr, every site counted for the named options with its file and line:
the check of a row is that list read against grep. `--summary` reads a
ledger written earlier and prints its tallies, tab-separated: a line a type
(`by_type`), and the cross-tabulation of EXPERIMENT, `#[doc(hidden)]` and
"an arm sets it" (`cross`), for `Options` and for every type together.

THE OPTION TYPES. `Options` and `SegmentOptions` (src/db.rs), `BlobOptions`
(src/blob.rs), `EncryptOptions` and `ReadOptions` (src/crypt.rs): one row a
field, and the number of fields is asserted equal to the number of entries
of the type's `impl Default`, name for name. `StoreCipher`
(src/db/vault.rs) has private fields, no `impl Default`, and builder
methods: one row a setter (a method taking `mut self` whose body assigns
one field), named by the setter. Its fields are asserted equal, name for
name, to the entries of the literal in `StoreCipher::new`; the fields no
setter assigns are the constructor's own arguments and have no row.

HOW A SOURCE IS READ. Comments and rustdoc are blanked, and for every count
of code so are string and char literals. Test code in src/ and in
bench/src/engines.rs is the item under each `#[cfg(test)]`,
`#[cfg(all(test, ..))]` or `#[test]` (`#[cfg(not(test))]` is not).

COLUMNS.

struct, name
    The option type and the field (StoreCipher: the setter).
type
    The field's declared type. StoreCipher: `(the setter's parameters) ->
    field: type`, naming the private field the setter assigns.
default
    The field's expression in the type's `impl Default`, whitespace
    collapsed and comments dropped, otherwise verbatim. StoreCipher: the
    backing field's expression in `StoreCipher::new`.
doc_hidden
    1 where the field (or setter) carries `#[doc(hidden)]`, else 0.
doc_markers
    Matched on the rustdoc with the `///` prefixes dropped and the lines
    joined by single spaces. Semicolon-separated, any of:
      EXPERIMENT   the word, case as written:  \bEXPERIMENT\b
      refuted      the word in any case (Refuted, refuted, REFUTED):
                   (?i)\brefuted\b
      is_the_arm   `is the arm`, `are the arms`:  \b(is|are) the arms?\b
      Off          a stated default of off: the capitalised word opening
                   the doc or following `. `, `; `, `: ` or `) `, itself
                   followed by `;`, `.`, ` by default` or ` as shipped`;
                   or, anywhere and in either case, `off by default`,
                   `off as shipped`, `as shipped, off`, `and off:`.
      On           the same with on.
    `Off: ...`, `Off, ...` and `Off is ...` are not matched: in this
    tree they describe what the store does with the option off, whichever
    way it ships (of the bool fields whose doc has `Off:`, more default
    to true than to false). The `default` column is the authority; a bool
    whose marker disagrees with it raises `doc_default_mismatch`.
doc_lines
    Lines of `///` rustdoc on the field (or setter).
arms_in_doc
    Every token `supdb-[a-z0-9]+(-[a-z0-9]+)*` in the rustdoc, in order of
    first appearance, each once. A name that is no arm of `open` in
    bench/src/engines.rs at this commit is suffixed `?` and raises
    `doc_arm_missing`; a doc quotes pairing commands, so such a name may be
    an arm of the commit the command ran at.
read_sites_src
    Field reads in src/**/*.rs outside test code: occurrences of `.name`
    as a whole word, not preceded by another `.` (so `a..name` and
    `..name` are not counted), not followed by an assignment (`=` but not
    `==` or `=>`, or a compound assignment) and not followed by `(` (a
    method call). The struct definition and the type's `impl Default` are
    excluded. Struct-literal initialisers (`name: value`) are not reads
    and are never counted. For a private field (StoreCipher) only the
    defining file is searched, `new` and the setters' bodies excluded.
    OVER-COUNTS: the receiver is not typed, so `.name` on any other struct
    with a field of that name is counted: a copy of an option kept in
    another struct and read there, or an unrelated field (`plan.sync`,
    `shared.keep_bytes`, `self.heads_owned` of the ordered index). Two
    names are fields of two option types (`prefault`, `key_id`); each
    type's row counts both. UNDER-COUNTS: a read through a destructuring
    pattern (`let Options { name, .. } = opts`); the tool counts those
    patterns per type and exits 1 if one stands in src/ outside tests, so
    a 0 here is no textual read at all. A local or a method of the same
    name is never counted, having no `.` before it or a `(` after it.
read_sites_recv
    The subset of read_sites_src whose receiver is known for the option
    type: the token before the `.` is an identifier declared somewhere in
    src/ as `ident: T`, `ident: &T`, `ident: &mut T`, `ident: Option<T>`
    (or Arc, Box, Rc), or bound by `let ident = T {` / `T::`; or is `self`
    inside an `impl T`; or is a call `f()` of a function declared
    `-> T` / `-> &T`. The receivers found are listed on stderr. Typed by
    name, not by scope: `opts` is a receiver of five of the six types.
    UNDER-COUNTS a read through an untyped local (`let o = &self.opts;
    o.name`). read_sites_src > 0 with this at 0 raises `recv_unread`.
read_sites_cfgtest
    The same occurrences as read_sites_src, inside test code of src/.
read_files
    The src files holding read_sites_src, `path:count`.
write_sites_src, write_sites_recv
    Sets in src/**/*.rs outside test code and outside the type's
    `impl Default` (StoreCipher: outside `new` and the setters' bodies):
    assignments `.name = ...` and compound assignments, by name as the
    reads are; plus `name: value` or shorthand `name` entries of a struct
    literal of the type, which are exact; plus, for StoreCipher, calls
    `.setter(arg)` with an argument. write_sites_recv keeps the literal
    entries and the assignments and calls whose receiver is known for the
    type, as read_sites_recv does.
bench_policy_field
    How `Supdb::with_policy` in bench/src/engines.rs sets the option: from
    the literal `let opts = supdb::Options { ... }` (its nested
    `supdb::SegmentOptions { ... }` for that type), any later
    `supdb::Options { ..opts }` literal and any `opts.name = ...`
    assignment in that function.
      x  or  !x      the Policy field of that name, or its negation
      ?x             `x.unwrap_or(supdb::<Type>::default().name)`: an
                     `Option` an arm pins, the engine's default otherwise
      fn(a,b)        any other expression: the Policy fields it reads, in
                     order, through the function's `let` locals (for an
                     assignment, the fields read by its value and by the
                     condition of its innermost enclosing block)
      lit(expr)      an expression that reads no Policy field: every supdb
                     arm runs with it
    Several, separated by ` | `, where the option is set in several places.
    StoreCipher: the Policy field behind the argument of the builder call
    in `encryption_of`, through the `...Shape { .. }` literal in
    `with_policy` or the `match` the argument is bound by. An option set
    in non-test code of that file outside those two functions reads
    `direct(<condition of the innermost enclosing block>)`. Empty: the
    suite never sets it, and every supdb arm runs the engine's default.
bench_arms_setting, bench_arm_names
    The supdb arms of `open` (bench/src/engines.rs) that change a Policy
    field the option's expression reads, and their names in `open`'s order,
    the first twelve and then `+N`. An arm's policy is evaluated
    statically: the `Policy { .., ..base }` literal in its `open` entry or
    in the `Supdb::create_*` function the entry names (through a
    constructor that hands on to another), its base through
    `Policy::default()` and the `Policy::name()` helpers, a constructor's
    parameters and a destructured `SharedShape` substituted from the call.
    A field is changed where its value's text differs from the text of its
    `impl Default for Policy` entry (`supdb::` prefixes and whitespace
    dropped). For `?x` an arm counts only where it pins `Some(v)` with `v`
    textually different from the engine's default. For `direct(...)`, the
    arms whose `open` entry names a path constant the condition tests.
    OVER-COUNTS for `fn(...)`: an arm that changes one field the expression
    reads is counted whether or not the expression's value moves (`a && b`
    with `a` alone changed; an `Option` pinned to the engine's own value).
    The arms `supdb-segment*` are not `Supdb`, have no policy, and are
    counted only through `direct(...)`.
tests_setting
    Sets, defined as for write_sites_src, in tests/**/*.rs and in test code
    of src/. The same over-count by name for assignments
    (`c.sync = ..` on a test's own struct).
fuzz_drawn, fuzz_how
    tests/fuzz.rs. fuzz_how is the strongest of, over every place that file
    sets the option:
      optvary:<f>   set from `c.<f>` where `Cfg` field `f` is initialised
                    in `pick_cfg` by an expression naming `vary` or
                    `ZFUZZ_OPTVARY`: drawn by seed under ZFUZZ_OPTVARY
      cfg:<f>       set from a `Cfg` field otherwise: drawn at every seed,
                    or filled behind another switch
      switch        set with no `Cfg` field read: a constant, or behind an
                    environment switch (ZFUZZ_...)
    For an assignment or a setter call the text read is its value and the
    conditions of every block enclosing it in its function. fuzz_drawn is
    1 for optvary and cfg, else 0.
other_setting
    Sets, defined as for write_sites_src, in every other tracked .rs file:
    examples/, bench/examples/, bench/tests/, bench/src/ (the test code of
    engines.rs included; the bodies of `with_policy` and `encryption_of`,
    and the `direct` sets, excluded), experiments/, web/. `dir:count`.
docs_mentions, docs_files
    Mentions in README.md, docs/*.md, ROADMAP.md, CLAUDE.md,
    bench/CLAUDE.md, bench/DESIGN.md, web/README.md: `Type::name` as a
    whole word, plus the bare name alone between backticks. docs_files is
    `path:qualified+bare`. OVER-COUNTS: a bare backticked name that is
    also a method or a word (`sync`, `compact`, `upkeep`) is counted.
rs_mentions
    `Type::name` as a whole word anywhere in a tracked .rs file, the
    option's own rustdoc excluded. A field is not written `Type::name` in
    code, so these are comments, rustdoc and strings: the references a
    deletion leaves dangling.
gate_mentions
    `Type::name` as a whole word in scripts/, bench/scripts/, tests/data/
    and .github/, `path:count`.
flags
    Semicolon-separated, each a function of the columns above:
      unread                read_sites_src == 0
      recv_unread           read_sites_src > 0 and read_sites_recv == 0
      no_arm                bench_arms_setting == 0
      bench_fixed           bench_policy_field has a `lit(..)` whose text
                            differs from the default's
      policy_unused         bench_policy_field reads Policy fields and no
                            arm changes one
      never_set             no arm, write_sites_recv == 0, no
                            other_setting, no bench_fixed, and
                            tests_setting == 0
      tests_only            the same with tests_setting > 0
      doc_default_mismatch  a bool whose default is true with marker Off,
                            or false with marker On
      doc_refuted           doc_markers has refuted
      doc_experiment        doc_markers has EXPERIMENT
      doc_arm_missing       arms_in_doc has a name `open` does not
      no_doc                doc_lines == 0
category, decision, note
    Empty: the judgment pass fills them.
"""
import bisect
import csv
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path.cwd()
ENGINES = "bench/src/engines.rs"
FUZZ = "tests/fuzz.rs"
DOCS = ["README.md", "ROADMAP.md", "CLAUDE.md", "bench/CLAUDE.md", "bench/DESIGN.md", "web/README.md"]
TYPES = [
    ("Options", "src/db.rs"),
    ("SegmentOptions", "src/db.rs"),
    ("BlobOptions", "src/blob.rs"),
    ("StoreCipher", "src/db/vault.rs"),
    ("EncryptOptions", "src/crypt.rs"),
    ("ReadOptions", "src/crypt.rs"),
]
FOREIGN = ("rocksdb::", "heed::", "std::", "memmap2::")
ARM = re.compile(r"supdb-[a-z0-9]+(?:-[a-z0-9]+)*")


def die(msg):
    sys.stderr.write("options_ledger: FAILED: " + msg + "\n")
    sys.exit(1)


def say(msg):
    sys.stderr.write("options_ledger: " + msg + "\n")


# ---------------------------------------------------------------- sources

TOKEN = re.compile(
    r"""//[^\n]*|/\*.*?\*/|b?r(#*)".*?"\1|b?"(?:[^"\\]|\\.)*"|"""
    r"""b?'(?:[^'\\\n]|\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|[^xu\n]))'""",
    re.S,
)
BRACE = re.compile(r"[{}]")
SEP = re.compile(r"[()\[\]{},]")


class Src:
    """A Rust source as three aligned texts: `raw`; `nocom`, comments
    blanked; `code`, comments and string and char literals blanked."""

    def __init__(self, path, text=None):
        self.path = path
        self.raw = (ROOT / path).read_text() if text is None else text
        nocom, code, last = [], [], 0
        for m in TOKEN.finditer(self.raw):
            s = m.group(0)
            blank = re.sub(r"[^\n]", " ", s)
            head = self.raw[last : m.start()]
            nocom.append(head)
            code.append(head)
            nocom.append(blank if s[0] == "/" else s)
            code.append(blank)
            last = m.end()
        nocom.append(self.raw[last:])
        code.append(self.raw[last:])
        self.nocom = "".join(nocom)
        self.code = "".join(code)
        self.starts = [0] + [m.end() for m in re.finditer(r"\n", self.raw)]
        self._tests = self._lines = None

    def lines(self):
        if self._lines is None:
            self._lines = self.raw.split("\n")
        return self._lines

    def line(self, pos):
        return bisect.bisect_right(self.starts, pos)

    def text(self, a, b):
        """`nocom` between two offsets, whitespace collapsed."""
        return " ".join(self.nocom[a:b].split())

    def close(self, i):
        """The offset of the `}` matching the `{` at `i`."""
        if self.code[i] != "{":
            die(f"{self.path}:{self.line(i)}: no brace where one was expected")
        d = 0
        for m in BRACE.finditer(self.code, i):
            if m.group() == "{":
                d += 1
            else:
                d -= 1
                if d == 0:
                    return m.start()
        die(f"{self.path}:{self.line(i)}: unbalanced brace")

    def close_paren(self, i):
        d = 0
        for j in range(i, len(self.code)):
            c = self.code[j]
            if c == "(":
                d += 1
            elif c == ")":
                d -= 1
                if d == 0:
                    return j
        die(f"{self.path}:{self.line(i)}: unbalanced parenthesis")

    def split(self, lo, hi):
        """The spans between top-level commas of code[lo:hi]."""
        out, d, start = [], 0, lo
        for m in SEP.finditer(self.code, lo, hi):
            c = m.group()
            if c in "([{":
                d += 1
            elif c in ")]}":
                d -= 1
            elif d == 0:
                out.append((start, m.start()))
                start = m.end()
        out.append((start, hi))
        return [(a, b) for a, b in out if self.code[a:b].strip()]

    def tests(self):
        """Spans of test code: the item under each `#[cfg(test)]`,
        `#[cfg(all(test, ..))]` or `#[test]`."""
        if self._tests is None:
            self._tests = merge_spans(test_spans(self))
        return self._tests

    def in_test(self, pos):
        return in_spans(self.tests(), pos)


def merge_spans(spans):
    out = []
    for a, b in sorted(spans):
        if out and a <= out[-1][1]:
            out[-1][1] = max(out[-1][1], b)
        else:
            out.append([a, b])
    return out


def in_spans(spans, pos):
    i = bisect.bisect_right(spans, [pos, float("inf")]) - 1
    return i >= 0 and spans[i][0] <= pos <= spans[i][1]


ITEM = {
    "fn", "mod", "impl", "struct", "enum", "use", "const", "static", "type",
    "trait", "unsafe", "extern", "async", "union", "macro_rules",
}
TEST_ATTR = re.compile(r"#\[\s*(cfg\s*\(|test\s*\])")


def test_spans(src):
    code, n = src.code, len(src.code)
    spans = []
    for m in TEST_ATTR.finditer(code):
        if m.group(1).startswith("cfg"):
            end = bracket_end(code, m.start() + 1)
            pred = code[m.start() : end]
            while True:
                cut = re.sub(r"\bnot\s*\([^()]*\)", "", pred)
                if cut == pred:
                    break
                pred = cut
            if not re.search(r"\btest\b", pred):
                continue
        else:
            end = m.end()
        i = end
        while True:
            while i < n and code[i].isspace():
                i += 1
            if code.startswith("#[", i):
                i = bracket_end(code, i + 1)
            else:
                break
        if i >= n:
            continue
        if code[i] == "{":
            spans.append((m.start(), src.close(i)))
            continue
        w = re.match(r"(?:pub(?:\s*\([^)]*\))?\s+)?(\w+)(\s*!)?", code[i : i + 200])
        kw = w.group(1) if w else ""
        d, j, stop = 0, i, None
        if w and w.group(2) and kw != "macro_rules":
            # A macro invocation: to the close of its first delimiter.
            j = i + w.end()
            while j < n and code[j].isspace():
                j += 1
            if code[j] == "{":
                stop = src.close(j)
            else:
                stop = src.close_paren(j) if code[j] == "(" else bracket_end(code, j) - 1
        elif kw in ITEM:
            while j < n:
                c = code[j]
                if c in "([":
                    d += 1
                elif c in ")]":
                    d -= 1
                elif c == "{" and d == 0:
                    stop = src.close(j)
                    break
                elif c == ";" and d == 0:
                    stop = j
                    break
                j += 1
        else:
            while j < n:
                c = code[j]
                if c in "([{":
                    d += 1
                elif c in ")]}":
                    d -= 1
                    if d < 0:
                        stop = j - 1
                        break
                    if c == "}" and d == 0:
                        # A block that closes the item, unless the
                        # expression goes on after it.
                        if not re.match(r"\s*(?:else\b|as\b|[.?;,)\]+\-*/&|=<>])", code[j + 1 : j + 80]):
                            stop = j
                            break
                elif c in ",;" and d == 0:
                    stop = j
                    break
                j += 1
        if stop is None:
            die(f"{src.path}:{src.line(m.start())}: a test attribute's item has no end")
        spans.append((m.start(), stop))
    return spans


def bracket_end(code, i):
    """The offset past the `]` matching the `[` at `i`."""
    d = 0
    for j in range(i, len(code)):
        if code[j] == "[":
            d += 1
        elif code[j] == "]":
            d -= 1
            if d == 0:
                return j + 1
    die("unbalanced attribute")


def tracked(suffix):
    try:
        out = subprocess.run(
            ["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True
        ).stdout.decode()
        files = [f for f in out.split("\0") if f]
    except (OSError, subprocess.CalledProcessError):
        files = []
        for d, ds, fs in os.walk(ROOT):
            ds[:] = [x for x in ds if not x.startswith(("target", ".git", "node_modules"))]
            for f in fs:
                files.append(os.path.relpath(os.path.join(d, f), ROOT))
    return sorted(f for f in files if f.endswith(suffix) and (ROOT / f).is_file())


# ----------------------------------------------------------- struct parse

ATTRS = re.compile(r"\s*(?:#\[[^\]]*\]\s*)*")


def parse_literal(src, open_idx):
    """The entries of the struct literal (or pattern) whose `{` is at
    `open_idx`: [(name, value or None for shorthand, offset)], its `..base`
    text or None, and the offset of its `}`. An entry that is no
    `name: value` and no bare name has name None."""
    close = src.close(open_idx)
    entries, base = [], None
    for a, b in src.split(open_idx + 1, close):
        a = ATTRS.match(src.code, a, b).end()
        seg = src.code[a:b]
        if seg.lstrip().startswith(".."):
            base = src.text(a, b)[2:].strip()
            continue
        m = re.match(r"\s*(?:ref\s+|mut\s+)*(\w+)\s*(?:(:)(?!:))?", seg)
        if not m:
            entries.append((None, src.text(a, b), a))
        elif m.group(2):
            entries.append((m.group(1), src.text(a + m.end(), b), a + m.start(1)))
        elif seg[m.end() :].strip():
            entries.append((None, src.text(a, b), a))
        else:
            entries.append((m.group(1), None, a + m.start(1)))
    return entries, base, close


def angle_depth(s):
    s = s.replace("->", "  ").replace("=>", "  ")
    return sum(s.count(o) - s.count(c) for o, c in ("()", "[]", "{}", "<>"))


def parse_struct(src, name):
    """The fields of `struct name { .. }`: dicts of name, type, vis, doc
    (lines), attrs, pos; and the struct's span."""
    m = re.search(r"^(?:pub(?:\([^)]*\))?\s+)?struct %s\b[^{;]*\{" % name, src.code, re.M)
    if not m:
        die(f"{src.path}: no struct {name}")
    open_idx = m.end() - 1
    close = src.close(open_idx)
    rl = src.raw[open_idx + 1 : close].split("\n")
    cl = src.code[open_idx + 1 : close].split("\n")
    offs, o = [], open_idx + 1
    for l in rl:
        offs.append(o)
        o += len(l) + 1
    fields, doc, attrs, k = [], [], [], 0
    while k < len(rl):
        s = rl[k].strip()
        if not s:
            pass
        elif s.startswith("///"):
            doc.append(s[4:] if s.startswith("/// ") else s[3:])
        elif s.startswith("//"):
            pass
        elif s.startswith("#["):
            while s.count("[") > s.count("]"):
                k += 1
                s += " " + rl[k].strip()
            attrs.append(s)
        else:
            at, decl = k, cl[k].strip()
            while (not decl.endswith(",") or angle_depth(decl)) and k + 1 < len(rl):
                k += 1
                decl += " " + cl[k].strip()
            f = re.fullmatch(r"(pub(?:\([^)]*\))?\s+)?(\w+)\s*:\s*(.+?),?", decl)
            if not f or angle_depth(decl):
                die(f"{src.path}:{src.line(offs[at])}: struct {name}: cannot place `{decl[:80]}`")
            fields.append(
                {
                    "name": f.group(2),
                    "type": " ".join(f.group(3).split()),
                    "vis": (f.group(1) or "").strip(),
                    "doc": doc,
                    "attrs": attrs,
                    "pos": offs[at],
                }
            )
            doc, attrs = [], []
        k += 1
    if doc or attrs:
        die(f"{src.path}: struct {name}: rustdoc or an attribute after the last field")
    return fields, (m.start(), close)


def parse_default(src, name):
    """The entries of the literal in `impl Default for name`, and the
    impl's span; None where the type has no such impl."""
    m = re.search(r"^impl\s+Default\s+for\s+%s\s*\{" % name, src.code, re.M)
    if not m:
        return None
    iopen = m.end() - 1
    iclose = src.close(iopen)
    f = re.compile(r"\bfn\s+default\s*\(\s*\)\s*->\s*\w+\s*\{").search(src.code, iopen, iclose)
    if not f:
        die(f"{src.path}: impl Default for {name}: no fn default")
    lit = re.compile(r"\b(?:%s|Self)\s*\{" % name).search(src.code, f.end(), iclose)
    if not lit:
        die(f"{src.path}: impl Default for {name}: no literal")
    entries, base, _ = parse_literal(src, lit.end() - 1)
    if base is not None:
        die(f"{src.path}: impl Default for {name}: a `..{base}` base")
    return entries, (m.start(), iclose)


def fns_in(src, lo, hi):
    """Every `fn` of code[lo:hi]: name -> dict(params [(name, type)],
    body (open, close), start, doc, attrs)."""
    out = {}
    for m in re.compile(r"\bfn\s+(\w+)\s*(?:<[^>(]*>)?\s*\(").finditer(src.code, lo, hi):
        pclose = src.close_paren(m.end() - 1)
        b, d = None, 0
        for b in re.compile(r"[{;()\[\]]").finditer(src.code, pclose + 1, hi):
            if b.group() in "([":
                d += 1
            elif b.group() in ")]":
                d -= 1
            elif d == 0:
                break
        else:
            b = None
        if not b or b.group() == ";":
            continue
        params = []
        for a, e in src.split(m.end(), pclose):
            p = src.text(a, e)
            pm = re.fullmatch(r"(?:mut\s+)?(\w+)\s*:\s*(.+)", p)
            params.append((pm.group(1), pm.group(2)) if pm else (p, ""))
        ls = src.raw.rfind("\n", 0, m.start()) + 1
        doc, attrs, k = [], [], src.line(ls) - 2
        lines = src.lines()
        while k >= 0:
            s = lines[k].strip()
            if s.startswith("///"):
                doc.insert(0, s[4:] if s.startswith("/// ") else s[3:])
            elif s.startswith("#["):
                attrs.insert(0, s)
            else:
                break
            k -= 1
        out.setdefault(
            m.group(1),
            {
                "params": params,
                "body": (b.start(), src.close(b.start())),
                "start": ls,
                "doc": doc,
                "attrs": attrs,
            },
        )
    return out


def impl_spans(src, name):
    """Spans of every `impl .. name {` and `impl .. for name {`."""
    out = []
    pat = r"^impl\b[^{;\n]*?\b%s\s*(?:<[^{]*>)?\s*\{" % name
    for m in re.finditer(pat, src.code, re.M):
        head = m.group(0)
        if re.search(r"\bfor\b", head) and not re.search(r"\bfor\s+%s\b" % name, head):
            continue
        out.append((m.start(), src.close(m.end() - 1)))
    return out


# ------------------------------------------------------------ the options


class Opt:
    def __init__(self, struct, name, file):
        self.struct, self.name, self.file = struct, name, file
        self.field = name  # the field `.x` is read by
        self.private = False
        self.type = self.default = ""
        self.doc, self.attrs = [], []
        self.own_doc_span = None
        self.reads = self.reads_recv = self.reads_test = self.writes = self.writes_recv = 0
        self.read_files = {}
        self.tests = 0
        self.other = {}
        self.fuzz = []
        self.bench = []  # (form, text, [policy fields])
        self.arms = []
        self.direct = []


def load_types(srcs):
    opts, spans, per = [], {}, {}
    for struct, file in TYPES:
        src = srcs[file]
        fields, sspan = parse_struct(src, struct)
        excl = [sspan]
        if struct == "StoreCipher":
            ispans = impl_spans(src, struct)
            fns = {}
            for a, b in ispans:
                for k, v in fns_in(src, a, b).items():
                    fns.setdefault(k, v)
            if "new" not in fns:
                die("StoreCipher: no fn new")
            nb = fns["new"]["body"]
            lit = re.compile(r"\bStoreCipher\s*\{").search(src.code, nb[0], nb[1])
            entries, base, _ = parse_literal(src, lit.end() - 1)
            defaults = {n: v for n, v, _ in entries}
            say(f"StoreCipher: {len(fields)} fields, {len(entries)} entries in StoreCipher::new (no impl Default: it is built by `new` and its setters)")
            if base is not None or len(fields) != len(entries) or {f['name'] for f in fields} != set(defaults):
                die("StoreCipher: the fields and the literal in `new` differ")
            excl.append(nb)
            ftype = {f["name"]: f["type"] for f in fields}
            backed = set()
            for fname, fn in fns.items():
                if not fn["params"] or fn["params"][0][0] != "mut self":
                    continue
                sets = set(re.findall(r"\bself\s*\.\s*(\w+)\s*=(?![=>])", src.code[fn["body"][0] : fn["body"][1]]))
                if len(sets) != 1 or not sets <= set(ftype):
                    die(f"StoreCipher::{fname}: a `mut self` method that assigns {sorted(sets)}")
                (field,) = sets
                backed.add(field)
                o = Opt(struct, fname, file)
                o.field, o.private = field, True
                ps = ", ".join(f"{n}: {t}" for n, t in fn["params"][1:])
                o.type = f"({ps}) -> {field}: {ftype[field]}"
                o.default = defaults[field]
                o.doc, o.attrs = fn["doc"], fn["attrs"]
                o.order = fn["start"]
                o.setter_body = fn["body"]
                excl.append(fn["body"])
                opts.append(o)
            rest = sorted(set(ftype) - backed)
            say(f"StoreCipher: {len(backed)} setters make {len(backed)} rows; fields with no setter, the constructor's arguments: {', '.join(rest)}")
            per[struct] = (len(fields), len(entries), len(backed))
        else:
            d = parse_default(src, struct)
            if d is None:
                die(f"{struct}: no impl Default")
            entries, dspan = d
            excl.append(dspan)
            defaults = {}
            for n, v, pos in entries:
                if n is None or v is None or n in defaults:
                    die(f"{file}:{src.line(pos)}: impl Default for {struct}: cannot place `{n or v}`")
                defaults[n] = v
            names = [f["name"] for f in fields]
            say(f"{struct}: {len(fields)} fields, {len(entries)} entries in impl Default")
            if len(names) != len(set(names)):
                die(f"{struct}: a field twice")
            if len(fields) != len(entries) or set(names) != set(defaults):
                miss = sorted(set(names) ^ set(defaults))
                die(f"{struct}: fields and Default entries differ: {miss[:10]}")
            for f in fields:
                o = Opt(struct, f["name"], file)
                o.private = not f["vis"].startswith("pub")
                o.type, o.default = f["type"], defaults[f["name"]]
                o.doc, o.attrs = f["doc"], f["attrs"]
                o.order = f["pos"]
                opts.append(o)
            per[struct] = (len(fields), len(entries), len(fields))
        spans[struct] = merge_spans(excl)
    return opts, spans, per


# -------------------------------------------------------- reads and sets

DOT = re.compile(r"(?<!\.)\.\s*([A-Za-z_]\w*)\b")
ASSIGN = re.compile(r"\s*(?:=(?![=>])|[-+*/%|&^]=(?!=)|<<=|>>=)")
CALL = re.compile(r"\s*(?:::\s*<[^<>()]*>\s*)?\(")


def receiver(code, dot):
    j = dot - 1
    while j >= 0 and code[j].isspace():
        j -= 1
    if j < 0:
        return None
    if code[j] == ")":
        d = 0
        while j >= 0:
            if code[j] == ")":
                d += 1
            elif code[j] == "(":
                d -= 1
                if d == 0:
                    break
            j -= 1
        k = j - 1
        while k >= 0 and code[k].isspace():
            k -= 1
        e = k + 1
        while k >= 0 and (code[k].isalnum() or code[k] == "_"):
            k -= 1
        return code[k + 1 : e] + "()" if e > k + 1 else None
    e = j + 1
    while j >= 0 and (code[j].isalnum() or code[j] == "_"):
        j -= 1
    return code[j + 1 : e] or None


def typed_receivers(srcs, struct):
    """Identifiers and getter calls typed as `struct` anywhere in src/."""
    wrap = r"(?:&\s*(?:'\w+\s+)?(?:mut\s+)?)?(?:(?:Arc|Box|Option|Rc)\s*<\s*&?\s*)?(?:\w+::)*"
    decl = re.compile(r"\b(\w+)\s*:\s*%s%s\b(?![\w<]|::)" % (wrap, struct))
    let = re.compile(r"\blet\s+(?:mut\s+)?(\w+)\s*=\s*(?:\w+::)*%s\s*(?:\{|::)" % struct)
    ret = re.compile(r"->\s*&?\s*(?:'\w+\s+)?(?:mut\s+)?(?:\w+::)*%s\b(?![\w<]|::)" % struct)
    out = set()
    for path, src in srcs.items():
        if not path.startswith("src/"):
            continue
        out |= set(decl.findall(src.code)) | set(let.findall(src.code))
        for m in ret.finditer(src.code):
            head = src.code[max(0, m.start() - 600) : m.start()]
            f = None
            for f in re.finditer(r"\bfn\s+(\w+)", head):
                pass
            if f and not re.search(r"[{}]", head[f.end() :]):
                out.add(f.group(1) + "()")
    return out


LIT = None


def literals(src, structs):
    """Every struct literal or pattern of an option type in `src`:
    (struct, entries, base, open, close, is_pattern)."""
    global LIT
    if LIT is None:
        LIT = re.compile(r"(?<![\w.])((?:\w+::)*)(%s)\s*\{" % "|".join(structs))
    out = []
    for m in LIT.finditer(src.code):
        if m.group(1).startswith(FOREIGN) or any(x in m.group(1) for x in FOREIGN):
            continue
        before = src.code[max(0, m.start() - 120) : m.start()].rstrip()
        if re.search(r"(?:\bstruct|\benum|\bimpl|\bfor|\bunion|\btrait|\bmod|->\s*&?\s*(?:'\w+\s+)?(?:mut)?)$", before):
            continue
        entries, base, close = parse_literal(src, m.end() - 1)
        after = src.code[close + 1 : close + 60].lstrip()
        pat = after.startswith("=>") or (after.startswith("=") and not after.startswith("=="))
        out.append((m.group(2), entries, base, m.end() - 1, close, pat, m.group(1)))
    return out


def headers(src, start, pos):
    """The text before each `{` that encloses `pos`, outermost first,
    scanning from `start` (the enclosing function's start)."""
    stack, last = [], start
    for m in re.compile(r"[{};]").finditer(src.code, start, pos):
        c = m.group()
        if c == "{":
            stack.append(src.text(last, m.start()))
        elif c == "}":
            if stack:
                stack.pop()
        last = m.end()
    return stack


def fn_start(src, pos):
    """The offset of the last `fn` at an indent of four or fewer before
    `pos`."""
    best = 0
    for m in re.finditer(r"^ {0,4}(?:pub(?:\([^)]*\))?\s+)?fn\s", src.code[:pos], re.M):
        best = m.start()
    return best


def statement_end(src, pos):
    m = re.compile(r";").search(src.code, pos)
    return m.start() if m else len(src.code)


# ------------------------------------------------------------- the suite

IDENT = re.compile(r"(?<![\w.:])([a-z_][a-z_0-9]*)\b(?!\s*(?:\(|::|!|:(?!:)))")


def idents(text):
    return [m.group(1) for m in IDENT.finditer(text)]


def norm(v):
    return re.sub(r"\s+", "", (v or "").replace("supdb::", ""))


class Suite:
    """bench/src/engines.rs: the Policy, the mapping in `with_policy`, and
    every supdb arm's policy."""

    def __init__(self, src, opts):
        self.src = src
        self.byname = {}
        for o in opts:
            self.byname.setdefault((o.struct, o.name), o)
        fields, _ = parse_struct(src, "Policy")
        d = parse_default(src, "Policy")
        self.pdef = {n: v for n, v, _ in d[0]}
        say(f"Policy: {len(fields)} fields, {len(d[0])} entries in impl Default")
        if len(fields) != len(d[0]) or {f["name"] for f in fields} != set(self.pdef):
            die("Policy: fields and Default entries differ")
        self.pfields = [f["name"] for f in fields]
        self.pset = set(self.pfields)
        self.impls = {}
        for t in ("Policy", "Supdb"):
            for a, b in impl_spans(src, t):
                if src.in_test(a):
                    continue
                for k, v in fns_in(src, a, b).items():
                    self.impls.setdefault(f"{t}::{k}", v)
        self.free = fns_in(src, 0, len(src.code))
        self.mapping()
        self.arms()

    # -- the mapping

    def refs(self, text):
        out = []
        for i in idents(text):
            for f in self.alias.get(i, [i] if i in self.pset else []):
                if f not in out:
                    out.append(f)
        return out

    def form(self, struct, name, value):
        v = value.strip()
        if v in self.pset:
            return ("x", v, [v])
        if v.startswith("!") and v[1:] in self.pset:
            return ("x", v, [v[1:]])
        m = re.fullmatch(r"(\w+)\s*\.\s*unwrap_or\(\s*supdb::(\w+)::default\(\)\s*\.\s*(\w+)\s*,?\s*\)", v)
        if m and m.group(1) in self.pset and m.group(2) == struct and m.group(3) == name:
            return ("?", "?" + m.group(1), [m.group(1)])
        r = self.refs(v)
        if r:
            return ("fn", "fn(" + ",".join(r) + ")", r)
        return ("=", "lit(" + v + ")", [])

    def mapping(self):
        src = self.src
        wp = self.impls.get("Supdb::with_policy")
        if not wp:
            die("engines.rs: no Supdb::with_policy")
        lo, hi = wp["body"]
        self.wp = (lo, hi)
        pat = re.compile(r"\blet\s+Policy\s*\{").search(src.code, lo, hi)
        if not pat:
            die("with_policy: no `let Policy { .. } = policy`")
        entries, base, pclose = parse_literal(src, pat.end() - 1)
        got = [n for n, v, _ in entries]
        say(f"with_policy: destructures {len(got)} Policy fields")
        if base is not None or any(v is not None for _, v, _ in entries) or set(got) != self.pset:
            die("with_policy: the destructuring does not name every Policy field once")
        lit = re.compile(r"\blet\s+opts\s*=\s*supdb::Options\s*\{").search(src.code, pclose, hi)
        if not lit:
            die("with_policy: no `let opts = supdb::Options {`")
        self.alias = {}
        for m in re.compile(r"\blet\s+(?:mut\s+)?(\w+)\s*=").finditer(src.code, pclose, lit.start()):
            end = statement_end(src, m.end())
            self.alias[m.group(1)] = self.refs(src.text(m.end(), end))
        say("with_policy: locals " + ", ".join(f"{k}<-({','.join(v)})" for k, v in self.alias.items()))
        entries, base, lclose = parse_literal(src, lit.end() - 1)
        n_main = n_seg = 0
        for n, v, pos in entries:
            o = self.byname.get(("Options", n))
            if not o or v is None:
                die(f"{ENGINES}:{src.line(pos)}: the Options literal sets `{n}`, which cannot be placed")
            o.bench.append(self.form("Options", n, v))
            n_main += 1
            if n == "segment":
                inner = re.compile(r"\bsupdb::SegmentOptions\s*\{").search(src.code, pos, lclose)
                e2, b2, _ = parse_literal(src, inner.end() - 1)
                for n2, v2, p2 in e2:
                    o2 = self.byname.get(("SegmentOptions", n2))
                    if not o2 or v2 is None:
                        die(f"{ENGINES}:{src.line(p2)}: the SegmentOptions literal sets `{n2}`, which cannot be placed")
                    o2.bench.append(self.form("SegmentOptions", n2, v2))
                    n_seg += 1
        n_late = 0
        for s, e2, b2, op, cl, is_pat, _ in literals(src, [t for t, _ in TYPES]):
            if not (lclose < op < hi) or is_pat:
                continue
            hd = headers(src, lo, op)
            for n2, v2, p2 in e2:
                o2 = self.byname.get((s, n2))
                if not o2:
                    die(f"{ENGINES}:{src.line(p2)}: a late literal sets `{n2}`, which cannot be placed")
                r = self.refs((v2 or n2) + " " + (hd[-1] if hd else ""))
                o2.bench.append(("fn", "fn(" + ",".join(r) + ")", r) if r else ("=", "lit(" + (v2 or n2) + ")", []))
                n_late += 1
        for m in re.compile(r"\bopts\s*\.\s*(\w+)\s*=(?![=>])").finditer(src.code, lclose, hi):
            o2 = self.byname.get(("Options", m.group(1)))
            if not o2:
                die(f"{ENGINES}:{src.line(m.start())}: `opts.{m.group(1)} =` cannot be placed")
            end = statement_end(src, m.end())
            hd = headers(src, lo, m.start())
            inner = hd[-1] if len(hd) > 1 else ""
            r = self.refs(src.text(m.end(), end) + " " + inner)
            o2.bench.append(("fn", "fn(" + ",".join(r) + ")", r) if r else ("=", "lit(" + src.text(m.end(), end) + ")", []))
            n_late += 1
        say(f"with_policy: the Options literal sets {n_main} fields, its SegmentOptions literal {n_seg}; {n_late} set after it")
        self.n_mapped = (n_main, n_seg, n_late)
        # StoreCipher: the builder calls of `encryption_of`, through the
        # `...Shape { field: policy_field }` literal in `with_policy`.
        shape = {}
        for m in re.compile(r"\b(\w*Shape)\s*\{").finditer(src.code, lclose, hi):
            for n2, v2, _ in parse_literal(src, m.end() - 1)[0]:
                shape[n2] = v2 or n2
        eo = self.free.get("encryption_of")
        self.eo = eo["body"] if eo else (0, 0)
        setters = {o.name: o for o in self.byname.values() if o.struct == "StoreCipher"}
        if eo:
            for m in re.compile(r"\.\s*(\w+)\s*\(").finditer(src.code, *eo["body"]):
                o2 = setters.get(m.group(1))
                if not o2:
                    continue
                arg = src.text(m.end(), src.close_paren(m.end() - 1))
                if not arg:
                    continue
                text = re.sub(r"\b\w+\s*\.\s*(\w+)", lambda g: shape.get(g.group(1), g.group(0)), arg)
                r = self.refs(text)
                if not r:
                    back = src.code[eo["body"][0] : m.start()]
                    mm = list(re.finditer(r"\bmatch\s+(\w+)\s*\{", back))
                    if mm:
                        r = self.refs(mm[-1].group(1))
                if len(r) == 1:
                    o2.bench.append(("x", r[0], r))
                elif r:
                    o2.bench.append(("fn", "fn(" + ",".join(r) + ")", r))
                else:
                    o2.bench.append(("=", "lit(" + arg + ")", []))

    # -- the arms

    def frag(self, text):
        return Src("<expr>", text)

    def subst(self, v, env):
        if v is None or not env:
            return v
        return re.sub(r"(?<![\w.:])([A-Za-z_]\w*)\b(?!\s*(?:\(|::|:(?!:)))", lambda m: env.get(m.group(1), m.group(1)), v)

    def policy(self, text, env, depth=0):
        """The Policy fields the expression `text` sets, with their value
        texts."""
        if depth > 12:
            die("an arm's policy does not resolve")
        t = text.strip().rstrip(",").strip()
        if t in env:
            return self.policy(env[t], {}, depth + 1)
        if re.fullmatch(r"Policy::default\(\s*\)", t):
            return {}
        m = re.fullmatch(r"Policy::(\w+)\((.*)\)", t, re.S)
        if m:
            fn = self.impls.get("Policy::" + m.group(1))
            if not fn:
                die(f"an arm's policy calls Policy::{m.group(1)}, which is not found")
            f = self.frag(m.group(2))
            args = [self.subst(f.text(a, b), env) for a, b in f.split(0, len(f.code))]
            names = [n for n, _ in fn["params"]]
            if len(args) != len(names):
                die(f"Policy::{m.group(1)}: {len(names)} parameters, {len(args)} arguments")
            body = self.src.text(fn["body"][0] + 1, fn["body"][1])
            return self.policy(body, dict(zip(names, args)), depth + 1)
        m = re.match(r"Policy\s*\{", t)
        if m:
            f = self.frag(t)
            entries, base, close = parse_literal(f, m.end() - 1)
            if f.code[close + 1 :].strip() or base is None:
                die(f"an arm's policy literal has no base or a tail: `{t[:80]}`")
            out = self.policy(base, env, depth + 1)
            for n, v, _ in entries:
                if n not in self.pset:
                    die(f"an arm's policy names `{n}`, no field of Policy")
                out[n] = self.subst(v if v is not None else n, env)
            return out
        die(f"an arm's policy cannot be evaluated: `{t[:100]}`")

    def with_policy_arg(self, src, lo, hi):
        m = re.compile(r"\bSupdb::with_policy\s*\(").search(src.code, lo, hi)
        if not m:
            return None
        parts = src.split(m.end(), src.close_paren(m.end() - 1))
        if len(parts) != 2:
            die("a call of with_policy without two arguments")
        return src.text(*parts[1])

    def struct_default(self, name):
        src = self.src
        m = re.search(r"((?:#\[[^\]]*\]\s*)*)(?:pub\s+)?struct %s\b" % name, src.code)
        if not m or "Default" not in m.group(1):
            die(f"{name}: not derive(Default); its default cannot be read")
        out = {}
        for f in parse_struct(src, name)[0]:
            t = f["type"]
            out[f["name"]] = "false" if t == "bool" else "None" if t.startswith("Option<") else "0"
        return out

    def create(self, name, args, depth=0):
        fn = self.impls.get("Supdb::" + name)
        if not fn:
            die(f"an arm names Supdb::{name}, which is not found")
        src, (lo, hi) = self.src, fn["body"]
        params = fn["params"][1:]
        if len(args) != len(params):
            die(f"Supdb::{name}: {len(params)} parameters after the path, {len(args)} arguments")
        env = {p: a for (p, _), a in zip(params, args)}
        for m in re.compile(r"\blet\s+(\w+)\s*\{").finditer(src.code, lo, hi):
            entries, base, close = parse_literal(src, m.end() - 1)
            rhs = re.match(r"\s*=\s*(\w+)\s*;", src.code[close + 1 :])
            if not rhs or rhs.group(1) not in env:
                die(f"Supdb::{name}: a destructuring the tool cannot follow")
            vals = self.struct_default(m.group(1))
            arg = env[rhs.group(1)].strip()
            f = self.frag(arg)
            lm = re.match(r"%s\s*\{" % m.group(1), f.code)
            if lm:
                e2, b2, _ = parse_literal(f, lm.end() - 1)
                if not b2 or not re.fullmatch(r"%s::default\(\s*\)" % m.group(1), b2):
                    die(f"Supdb::{name}: a {m.group(1)} literal without `..{m.group(1)}::default()`")
                for n2, v2, _ in e2:
                    vals[n2] = v2 if v2 is not None else n2
            elif not re.fullmatch(r"%s::default\(\s*\)" % m.group(1), arg):
                die(f"Supdb::{name}: argument `{arg[:60]}` cannot be evaluated")
            for n2, v2, _ in entries:
                env[v2 or n2] = vals[n2]
        text = self.with_policy_arg(src, lo, hi)
        if text is None:
            # A constructor that hands on to another one.
            c = re.compile(r"\bSupdb::(\w+)\s*\(").search(src.code, lo, hi)
            if not c or depth > 6:
                die(f"Supdb::{name}: no call of with_policy")
            pc = src.close_paren(c.end() - 1)
            args = [self.subst(src.text(a, b), env) for a, b in src.split(c.end(), pc)][1:]
            return self.create(c.group(1), args, depth + 1)
        return self.policy(text, env)

    def arms(self):
        src = self.src
        op = self.free.get("open")
        if not op:
            die("engines.rs: no fn open")
        lo, hi = op["body"]
        marks = list(re.compile(r'^ {8}((?:#\[[^\]\n]*\]\s*)*)((?:"[a-z0-9-]+"\s*\|?\s*)+)=>', re.M).finditer(src.nocom, lo, hi))
        self.arm_text, self.arm_policy, self.all_arms = {}, {}, []
        for i, m in enumerate(marks):
            end = marks[i + 1].start() if i + 1 < len(marks) else hi
            names = re.findall(r'"([a-z0-9-]+)"', m.group(2))
            for n in names:
                if n not in self.all_arms:
                    self.all_arms.append(n)
            for n in names:
                if not n.startswith("supdb") or n in self.arm_text:
                    continue
                self.arm_text[n] = (m.end(), end)
                text = self.with_policy_arg(src, m.end(), end)
                if text is not None:
                    pol = self.policy(text, {})
                else:
                    c = re.compile(r"\bSupdb::(\w+)\s*(\()?").search(src.code, m.end(), end)
                    if not c:
                        continue
                    args = []
                    if c.group(2):
                        pc = src.close_paren(c.end() - 1)
                        args = [src.text(a, b) for a, b in src.split(c.end(), pc)][1:]
                    pol = self.create(c.group(1), args)
                self.arm_policy[n] = {f: v for f, v in pol.items() if norm(v) != norm(self.pdef[f])}
        nopol = [n for n in self.arm_text if n not in self.arm_policy]
        say(f"open: {len(self.all_arms)} arms, {len(self.arm_text)} of them supdb arms, {len(self.arm_policy)} with a policy; without one: {', '.join(nopol)}")

    def arms_for(self, o):
        out = []
        for n, pol in self.arm_policy.items():
            hit = False
            for form, _, refs in o.bench:
                for f in refs:
                    if f not in pol:
                        continue
                    if form == "?":
                        m = re.fullmatch(r"Some\((.*)\)", pol[f].strip(), re.S)
                        if not m or norm(m.group(1)) == norm(o.default):
                            continue
                    hit = True
            if hit:
                out.append(n)
        for cond in o.direct:
            consts = re.findall(r"\b[A-Z]\w*::[A-Z]\w*\b", cond)
            for n, (a, b) in self.arm_text.items():
                if consts and any(c in self.src.code[a:b] for c in consts) and n not in out:
                    out.append(n)
        order = {n: i for i, n in enumerate(self.all_arms)}
        return sorted(out, key=order.get)


# ---------------------------------------------------------------- the fuzz


def fuzz(src, opts, byfield):
    cfg, _ = parse_struct(src, "Cfg")
    cset = {f["name"] for f in cfg}
    fns = fns_in(src, 0, len(src.code))
    pc = fns.get("pick_cfg")
    if not pc:
        die("tests/fuzz.rs: no fn pick_cfg")
    lit = None
    for m in re.compile(r"(?<!\w)Cfg\s*\{").finditer(src.code, *pc["body"]):
        lit = m
    entries, base, _ = parse_literal(src, lit.end() - 1)
    vary = {}
    for n, v, pos in entries:
        if v is None:
            lm = re.compile(r"\blet\s+(?:mut\s+)?%s\s*=" % n).search(src.code, *pc["body"])
            v = src.text(lm.end(), statement_end(src, lm.end())) if lm else ""
        vary[n] = bool(re.search(r"\bvary\b|ZFUZZ_OPTVARY", v))
    if set(vary) != cset or base is not None:
        die("tests/fuzz.rs: pick_cfg's literal does not name every Cfg field")
    say(f"fuzz: Cfg has {len(cset)} fields, {sum(vary.values())} of them drawn under ZFUZZ_OPTVARY")

    def cls(text):
        fs = [f for f in re.findall(r"\b(?:c|cfg)\s*\.\s*(\w+)", text) if f in cset]
        v = [f for f in fs if vary[f]]
        if v:
            return (2, "optvary:" + "+".join(dict.fromkeys(v)))
        if fs:
            return (1, "cfg:" + "+".join(dict.fromkeys(fs)))
        return (0, "switch")

    for s, entries, base, op, cl, is_pat, _ in literals(src, [t for t, _ in TYPES]):
        if is_pat:
            continue
        for n, v, pos in entries:
            for o in opts:
                if o.struct == s and o.name == n:
                    o.fuzz.append(cls(v or n))
    for m in DOT.finditer(src.code):
        am = ASSIGN.match(src.code, m.end())
        cm = CALL.match(src.code, m.end())
        for o in byfield.get(m.group(1), []):
            if o.struct != "StoreCipher" and am:
                end = statement_end(src, am.end())
                hd = headers(src, fn_start(src, m.start()), m.start())
                o.fuzz.append(cls(src.text(am.end(), end) + " " + " ".join(hd[1:])))
        if cm:
            for o in opts:
                if o.struct == "StoreCipher" and o.name == m.group(1):
                    arg = src.text(cm.end(), src.close_paren(cm.end() - 1))
                    if arg:
                        hd = headers(src, fn_start(src, m.start()), m.start())
                        o.fuzz.append(cls(arg + " " + " ".join(hd[1:])))


# ------------------------------------------------------------------- docs

OFF_ON = (
    r"(?:(?:^|(?<=[.;:)] ))%(W)s(?:[;.]| by default| as shipped))"
    r"|(?i:\b%(w)s by default\b|\b%(w)s as shipped\b|\bas shipped, %(w)s\b|\band %(w)s:)"
)
MARKERS = [
    ("EXPERIMENT", re.compile(r"\bEXPERIMENT\b")),
    ("refuted", re.compile(r"(?i)\brefuted\b")),
    ("is_the_arm", re.compile(r"\b(?:is|are) the arms?\b")),
    ("Off", re.compile(OFF_ON % {"W": "Off", "w": "off"})),
    ("On", re.compile(OFF_ON % {"W": "On", "w": "on"})),
]


SITES = set()


def summary(path):
    """Tallies of a ledger written earlier, tab-separated."""
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f))
    order = [t for t, _ in TYPES]

    def has(r, col, v):
        return v in r[col].split(";")

    def kind(r):
        return r["default"] if r["default"] in ("true", "false") else "other"

    tallies = [
        ("rows", lambda r: True),
        ("doc_hidden", lambda r: r["doc_hidden"] == "1"),
        ("experiment", lambda r: has(r, "doc_markers", "EXPERIMENT")),
        ("refuted", lambda r: has(r, "doc_markers", "refuted")),
        ("is_the_arm", lambda r: has(r, "doc_markers", "is_the_arm")),
        ("doc_names_arm", lambda r: bool(r["arms_in_doc"])),
        ("bench_mapped", lambda r: bool(r["bench_policy_field"])),
        ("arm_sets", lambda r: int(r["bench_arms_setting"]) > 0),
        ("no_arm", lambda r: has(r, "flags", "no_arm")),
        ("unread", lambda r: has(r, "flags", "unread")),
        ("recv_unread", lambda r: has(r, "flags", "recv_unread")),
        ("read_once", lambda r: r["read_sites_src"] == "1"),
        ("tests_only", lambda r: has(r, "flags", "tests_only")),
        ("never_set", lambda r: has(r, "flags", "never_set")),
        ("fuzz_drawn", lambda r: r["fuzz_drawn"] == "1"),
        ("fuzz_optvary", lambda r: r["fuzz_how"].startswith("optvary")),
        ("doc_arm_missing", lambda r: has(r, "flags", "doc_arm_missing")),
        ("doc_default_mismatch", lambda r: has(r, "flags", "doc_default_mismatch")),
        ("bench_fixed", lambda r: has(r, "flags", "bench_fixed")),
        ("no_doc", lambda r: has(r, "flags", "no_doc")),
        ("bool", lambda r: r["type"] == "bool"),
        ("default_true", lambda r: r["default"] == "true"),
        ("default_false", lambda r: r["default"] == "false"),
    ]
    w = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
    w.writerow(["by_type", "type"] + [n for n, _ in tallies])
    for t in order + ["ALL"]:
        sel = [r for r in rows if t == "ALL" or r["struct"] == t]
        w.writerow(["by_type", t] + [sum(1 for r in sel if f(r)) for _, f in tallies])
    cols = [
        ("rows", lambda r: True),
        ("default_true", lambda r: kind(r) == "true"),
        ("default_false", lambda r: kind(r) == "false"),
        ("default_other", lambda r: kind(r) == "other"),
        ("refuted", lambda r: has(r, "doc_markers", "refuted")),
        ("doc_names_arm", lambda r: bool(r["arms_in_doc"])),
        ("tests_only", lambda r: has(r, "flags", "tests_only")),
        ("never_set", lambda r: has(r, "flags", "never_set")),
        ("fuzz_drawn", lambda r: r["fuzz_drawn"] == "1"),
        ("read_once", lambda r: r["read_sites_src"] == "1"),
    ]
    for scope in ("Options", "ALL"):
        w.writerow(["cross", "scope", "experiment", "doc_hidden", "arm_sets"] + [n for n, _ in cols])
        for e in ("1", "0"):
            for h in ("1", "0"):
                for a in ("1", "0"):
                    sel = [
                        r
                        for r in rows
                        if (scope == "ALL" or r["struct"] == scope)
                        and str(int(has(r, "doc_markers", "EXPERIMENT"))) == e
                        and r["doc_hidden"] == h
                        and str(int(int(r["bench_arms_setting"]) > 0)) == a
                    ]
                    w.writerow(["cross", scope, e, h, a] + [sum(1 for r in sel if f(r)) for _, f in cols])


def main():
    args = sys.argv[1:]
    if len(args) == 2 and args[0] == "--summary":
        return summary(args[1])
    if not (ROOT / "src/db.rs").is_file() or not (ROOT / ENGINES).is_file():
        die("run from the repository root")
    while args:
        if args[0] == "--sites" and len(args) > 1:
            SITES.update(args[1].split(","))
            args = args[2:]
        else:
            die("usage: options_ledger.py [--sites Type.name[,Type.name...]] > options.csv | --summary options.csv")
    rs = tracked(".rs")
    srcs = {p: Src(p) for p in rs}
    opts, excl, per = load_types(srcs)
    structs = [t for t, _ in TYPES]
    byfield, bysetter = {}, {}
    for o in opts:
        if o.struct == "StoreCipher":
            byfield.setdefault(o.field, []).append(o)
            bysetter[o.name] = o
        else:
            byfield.setdefault(o.name, []).append(o)
    shared = {k: [o.struct for o in v] for k, v in byfield.items() if len(v) > 1}
    if shared:
        say("field names in more than one option type (counted for each): " + "; ".join(f"{k}: {'/'.join(v)}" for k, v in sorted(shared.items())))
    recv = {s: typed_receivers(srcs, s) for s in structs}
    for s in structs:
        say(f"{s}: typed receivers {', '.join(sorted(recv[s])) or '(none)'}")
    impls = {(p, s): merge_spans(impl_spans(srcs[p], s)) for p in srcs if p.startswith("src/") for s in structs}

    suite = Suite(srcs[ENGINES], opts)
    key = {(o.struct, o.name): o for o in opts}
    patterns = {s: 0 for s in structs}
    prefixes = {s: set() for s in structs}
    unplaced = []

    def trace(o, kind, path, pos, src):
        if f"{o.struct}.{o.name}" in SITES:
            line = src.raw[src.starts[src.line(pos) - 1] :].split("\n", 1)[0].strip()
            say(f"site {o.struct}.{o.name} {kind} {path}:{src.line(pos)}: {line[:110]}")

    def where(path, pos, src):
        """src | test | other:<dir> | bench (the mapping) | direct"""
        if path.startswith("src/"):
            return "test" if src.in_test(pos) else "src"
        if path.startswith("tests/"):
            return "test"
        if path == ENGINES and not src.in_test(pos):
            if suite.wp[0] <= pos <= suite.wp[1] or suite.eo[0] <= pos <= suite.eo[1]:
                return "bench"
            return "direct"
        return "other:" + path.split("/")[0] + ("/" + path.split("/")[1] if path.startswith("bench/") else "")

    def record_set(o, path, pos, src, typed=True):
        w = where(path, pos, src)
        if w == "src" and path == o.file and in_spans(excl[o.struct], pos):
            return
        trace(o, "set:" + w, path, pos, src)
        if w == "src":
            o.writes += 1
            o.writes_recv += int(typed)
        elif w == "test":
            o.tests += 1
        elif w == "direct":
            hd = headers(src, fn_start(src, pos), pos)
            o.direct.append(hd[-1] if len(hd) > 1 else "")
        elif w.startswith("other:"):
            o.other[w[6:]] = o.other.get(w[6:], 0) + 1

    for path, src in srcs.items():
        for s, entries, base, op, cl, is_pat, prefix in literals(src, structs):
            prefixes[s].add(prefix)
            if path.startswith("src/") and in_spans(excl[s], op) and path == dict(TYPES)[s]:
                continue
            if is_pat:
                if path.startswith("src/") and not src.in_test(op):
                    patterns[s] += 1
                continue
            for n, v, pos in entries:
                if s == "StoreCipher":
                    continue
                o = key.get((s, n))
                if o is None:
                    unplaced.append(f"{path}:{src.line(pos)}: {s} literal entry `{n if n else v}`")
                    continue
                record_set(o, path, pos, src)
        code = src.code
        for m in DOT.finditer(code):
            name = m.group(1)
            if name in bysetter:
                cm = CALL.match(code, m.end())
                if cm and code[cm.end() : src.close_paren(cm.end() - 1)].strip():
                    r0 = receiver(code, m.start()) or ""
                    record_set(bysetter[name], path, m.start(), src, r0 in recv["StoreCipher"])
            os_ = byfield.get(name)
            if not os_:
                continue
            assign = ASSIGN.match(code, m.end())
            call = CALL.match(code, m.end())
            if call and not assign:
                continue
            r = None
            for o in os_:
                if o.private and path != o.file:
                    continue
                if path == o.file and in_spans(excl[o.struct], m.start()):
                    continue
                if r is None:
                    r = receiver(code, m.start()) or ""
                typed = r in recv[o.struct] or (
                    r == "self" and path.startswith("src/") and in_spans(impls[(path, o.struct)], m.start())
                )
                if assign:
                    record_set(o, path, m.start(), src, typed)
                    continue
                if not path.startswith("src/"):
                    continue
                if src.in_test(m.start()):
                    o.reads_test += 1
                    trace(o, "read:test", path, m.start(), src)
                    continue
                o.reads += 1
                o.read_files[path] = o.read_files.get(path, 0) + 1
                o.reads_recv += int(typed)
                trace(o, "read:" + ("recv" if typed else "other"), path, m.start(), src)
    for s in structs:
        say(f"{s}: literal path prefixes seen: {', '.join(sorted(repr(p) for p in prefixes[s])) or '(none)'}; destructuring patterns in src/ outside tests: {patterns[s]}")
    if unplaced:
        for u in unplaced[:40]:
            say("cannot place: " + u)
        die(f"{len(unplaced)} struct-literal entries name no field of their type")
    if any(patterns.values()):
        die("a destructuring pattern of an option type in src/: reads through it are not counted")

    fuzz(srcs[FUZZ], opts, byfield)

    # mentions
    docfiles = [d for d in DOCS + sorted(str(p.relative_to(ROOT)) for p in (ROOT / "docs").glob("*.md")) if (ROOT / d).is_file()]
    doctext = {d: (ROOT / d).read_text() for d in docfiles}
    gatefiles = [f for f in tracked("") if f.startswith(("scripts/", "bench/scripts/", "tests/data/", ".github/"))]
    gatetext = {}
    for f in gatefiles:
        try:
            gatetext[f] = (ROOT / f).read_text()
        except UnicodeDecodeError:
            pass
    qual = re.compile(r"\b(%s)::([a-z_][a-z_0-9]*)\b" % "|".join(structs))
    bare = re.compile(r"`([a-z_][a-z_0-9]*)`")

    def tally(texts, pat, two):
        out = {}
        for f, t in texts.items():
            for m in pat.finditer(t):
                k = (m.group(1), m.group(2)) if two else m.group(1)
                out.setdefault(k, {})
                out[k][f] = out[k].get(f, 0) + 1
        return out

    dq, db_ = tally(doctext, qual, True), tally(doctext, bare, False)
    gq = tally(gatetext, qual, True)
    rq = tally({p: s.raw for p, s in srcs.items()}, qual, True)

    arms = set(suite.all_arms)
    w = csv.writer(sys.stdout, lineterminator="\n")
    cols = [
        "struct", "name", "type", "default", "doc_hidden", "doc_markers", "doc_lines", "arms_in_doc",
        "read_sites_src", "read_sites_recv", "read_sites_cfgtest", "read_files", "write_sites_src",
        "write_sites_recv",
        "bench_policy_field", "bench_arms_setting", "bench_arm_names", "tests_setting", "fuzz_drawn",
        "fuzz_how", "other_setting", "docs_mentions", "docs_files", "rs_mentions", "gate_mentions",
        "flags", "category", "decision", "note",
    ]
    w.writerow(cols)
    order = {s: i for i, s in enumerate(structs)}
    n_rows = {s: 0 for s in structs}
    for o in sorted(opts, key=lambda o: (order[o.struct], o.order)):
        n_rows[o.struct] += 1
        doc = " ".join(" ".join(o.doc).split())
        markers = [n for n, p in MARKERS if p.search(doc)]
        named = list(dict.fromkeys(ARM.findall(doc)))
        hidden = int(any(re.fullmatch(r"#\[\s*doc\s*\(\s*hidden\s*\)\s*\]", a) for a in o.attrs))
        own = len(re.findall(r"\b%s::%s\b" % (o.struct, o.name), doc))
        if o.direct:
            o.bench.append(("direct", "direct(" + " ; ".join(dict.fromkeys(c or "unconditional" for c in o.direct)) + ")", []))
        setting = suite.arms_for(o)
        q, b = dq.get((o.struct, o.name), {}), db_.get(o.name, {})
        files = sorted(set(q) | set(b))
        rsm = sum(rq.get((o.struct, o.name), {}).values()) - own
        gm = gq.get((o.struct, o.name), {})
        best = max(o.fuzz) if o.fuzz else None
        flags = []
        if o.reads == 0:
            flags.append("unread")
        elif o.reads_recv == 0:
            flags.append("recv_unread")
        if not setting:
            flags.append("no_arm")
        fixed = any(f == "=" and norm(t[4:-1]) != norm(o.default) for f, t, _ in o.bench)
        if fixed:
            flags.append("bench_fixed")
        if o.bench and not setting and not fixed and any(r for _, _, r in o.bench):
            flags.append("policy_unused")
        if not setting and not o.writes_recv and not o.other and not fixed:
            flags.append("tests_only" if o.tests else "never_set")
        if o.default in ("true", "false") and ("Off" if o.default == "true" else "On") in markers:
            flags.append("doc_default_mismatch")
        if "refuted" in markers:
            flags.append("doc_refuted")
        if "EXPERIMENT" in markers:
            flags.append("doc_experiment")
        if any(a not in arms for a in named):
            flags.append("doc_arm_missing")
        if not o.doc:
            flags.append("no_doc")
        w.writerow(
            [
                o.struct, o.name, o.type, o.default, hidden, ";".join(markers), len(o.doc),
                ";".join(a if a in arms else a + "?" for a in named),
                o.reads, o.reads_recv, o.reads_test,
                ";".join(f"{f}:{n}" for f, n in sorted(o.read_files.items())),
                o.writes, o.writes_recv,
                " | ".join(dict.fromkeys(t for _, t, _ in o.bench)),
                len(setting),
                ";".join(setting[:12]) + (f";+{len(setting) - 12}" if len(setting) > 12 else ""),
                o.tests, int(bool(best and best[0] > 0)), best[1] if best else "",
                ";".join(f"{d}:{n}" for d, n in sorted(o.other.items())),
                sum(q.values()) + sum(b.values()),
                ";".join(f"{f}:{q.get(f, 0)}+{b.get(f, 0)}" for f in files),
                rsm,
                ";".join(f"{f}:{n}" for f, n in sorted(gm.items())),
                ";".join(flags), "", "", "",
            ]
        )
    for s in structs:
        f, d, r = per[s]
        say(f"{s}: {n_rows[s]} rows ({f} fields, {d} default entries)")
        if n_rows[s] != r:
            die(f"{s}: {n_rows[s]} rows written, {r} expected")
    say(f"{sum(n_rows.values())} rows")


if __name__ == "__main__":
    main()
