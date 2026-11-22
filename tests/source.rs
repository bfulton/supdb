//! Tests that read the crate's own source as text, over one lexer
//! (`tests/lex`).
//!
//! The wall: the forms -- the segment format, its readers, the compact form,
//! the engine -- and the encryption modules -- the door's vault, the pager,
//! the envelope, the kernel's key -- do not name each other, beyond the
//! couplings `tests/data/couplings.txt` lists. Each listed line is a scope's
//! count of one token, with the stage expected to take it to zero. The gate
//! holds the totals by (file, token, region): a count that grows anywhere
//! fails, plumbing included, and so does one that falls, until the file is
//! lowered with it, so the file only goes down within a tree; across a
//! change, `sh scripts/check.sh lists` holds it against the base
//! (scripts/ratchet.py), as it holds the door's lists and
//! `tests/data/cost-entries.txt`'s reasons. A coupling moved from one
//! function to another changes no total and passes; the scopes measured and
//! listed are printed beside a failure, to say where.
//!
//! Names are read as the language resolves them, not as a list of the other
//! side's words: on the encryption side every path from `super` or `crate`
//! is resolved against the module it is written in and counted where it
//! leaves the encryption modules and the seam, and so is every such path of
//! the seam's own, which both sides name; an `impl` of a type a file does
//! not define is counted on the encryption side, and one of a seam or
//! encryption type on the form side, its self type resolved through the
//! file's `use .. as` renames and `type` aliases, since a method is named by
//! a neutral word wherever it is called; a macro the encryption side calls
//! and does not define is counted, a std macro's name included where a file
//! under `src/` defines a macro of that name, which shadows the std one; and
//! code from another file (`include`, a `path` attribute, plain or under
//! `cfg_attr`) is counted on both sides.
//!
//! `SUPDB_WALL_PRINT=1` prints the measured table in the file's format. The
//! test never writes the file: a test that wrote it would make raising a
//! count silent. `SUPDB_WALL_ROOT` reads another tree than this crate's, for
//! a red input made by a script.

mod lex;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lex::{count_ci, is_word, regions, words};

/// The encryption side: the door, the cache, the envelope, the kernel's key.
const ENC: &[&str] = &[
    "src/crypt.rs",
    "src/db/dirkey.rs",
    "src/db/pager.rs",
    "src/db/vault.rs",
];

/// The seam both sides may name: `Bytes`, `MmapBytes`, `Paging`. Counted
/// against the cipher's tokens with every line staged `stays`, so a new
/// cipher-named predicate there is growth.
const SEAM: &[&str] = &["src/bytes.rs"];

/// The crate root: the modules and the re-exports of both sides. Counted
/// against the cipher's tokens with every line staged `stays`, since a
/// re-export is an alias: an encryption item re-exported under a neutral
/// name is named by that name in the forms, which the gate counts there
/// (`crate-root alias of an encryption item`).
const ROOT: &[&str] = &["src/lib.rs"];

/// The modules of the encryption side, as a path names them.
const ENC_MODS: &[&str] = &["vault", "pager", "crypt", "dirkey"];

/// The form side: everything that reads or writes a form.
const FORM: &[&str] = &[
    "src/blob.rs",
    "src/block.rs",
    "src/db.rs",
    "src/db/blocks.rs",
    "src/db/zfile.rs",
    "src/db/zlz4.rs",
    "src/db/zpages.rs",
    "src/db/zselect.rs",
    "src/flatindex.rs",
    "src/format.rs",
    "src/index.rs",
    "src/ordindex.rs",
    "src/reserve.rs",
    "src/wasmapi.rs",
];

/// The vault's functions that are the door every file operation of the
/// engine goes through, a plain store's included, and so name no cipher:
/// forms.md R0's verbs, the plain I/O verbs beside them, the directory's
/// guards and fsync counters, and the I/O trace the tests read. Any other
/// `vault::name` a form writes is counted under its own name. `vault::real`
/// is not one: it is the log's name passed through to the directory under a
/// cipher.
const DOOR: &[&str] = &[
    "create",
    "create_rw",
    "open",
    "open_write",
    "open_append",
    "open_slot",
    "open_dir",
    "map",
    "rename",
    "hard_link",
    "remove_file",
    "names",
    "len",
    "exists",
    "is_file",
    "read",
    "sync_dir",
    "copy",
    "dir_exists",
    "open_rw",
    "open_lock",
    "sync_file",
    "sync_slot",
    "sync_dir_as",
    "DirSync",
    "guard",
    "guarded",
    "adopt_guard",
    "forget_guards",
    "GUARD_SUFFIX",
    "dir_ticket",
    "dir_synced_after",
    "note",
    "note_file",
    "trace_io",
    "traced_descriptors",
    "watch_dir",
    "unwatch_dir",
    "hook_dir_sync",
    "DirHook",
];

/// The stages a line may name (forms.md section 4, design.md section 10).
const STAGES: &[&str] = &[
    "G1",
    "G3",
    "R4",
    "S1",
    "S4 (conditional; else stays)",
    "S5",
    "S6",
    "stays",
];

fn word_in(s: &[char], set: &[&str]) -> usize {
    words(s)
        .iter()
        .filter(|(_, w)| set.contains(&w.as_str()))
        .count()
}

fn starts(s: &[char], i: usize, lit: &str) -> bool {
    lit.chars()
        .enumerate()
        .all(|(k, c)| s.get(i + k) == Some(&c))
}

fn boundary_before(s: &[char], i: usize) -> bool {
    i == 0 || !is_word(s[i - 1])
}

fn boundary_at(s: &[char], i: usize) -> bool {
    i >= s.len() || !is_word(s[i])
}

/// An alternative of a pattern: the length it matches at an index, if any.
type Alt<'a> = &'a dyn Fn(&[char], usize) -> Option<usize>;

/// A side's token classes over one line of code, given the crate root's
/// aliases of encryption items: (class, occurrences).
type Tokens = fn(&str, &[String]) -> Vec<(String, usize)>;

/// Leftmost, non-overlapping matches of alternatives at each position,
/// first alternative first: each answers the length it matched at `i`.
fn leftmost(s: &[char], alts: &[Alt]) -> usize {
    let (mut i, mut n) = (0, 0);
    'pos: while i < s.len() {
        for a in alts {
            if let Some(len) = a(s, i) {
                n += 1;
                i += len.max(1);
                continue 'pos;
            }
        }
        i += 1;
    }
    n
}

/// A literal at `i`, with a word boundary before it if `before` and after
/// it if `after`.
fn lit(s: &[char], i: usize, l: &str, before: bool, after: bool) -> Option<usize> {
    let len = l.chars().count();
    (starts(s, i, l) && (!before || boundary_before(s, i)) && (!after || boundary_at(s, i + len)))
        .then_some(len)
}

/// The cipher's vocabulary with no crate-root aliases, for the unit tests.
fn cipher_tokens(code: &str) -> Vec<(String, usize)> {
    cipher_tokens_in(code, &[])
}

/// `k` moved past any whitespace.
fn skip_ws(s: &[char], mut k: usize) -> usize {
    while k < s.len() && s[k].is_whitespace() {
        k += 1;
    }
    k
}

/// The index just past the last non-whitespace character before `k`.
fn back_ws(s: &[char], mut k: usize) -> usize {
    while k > 0 && s[k - 1].is_whitespace() {
        k -= 1;
    }
    k
}

/// The word that starts at `k`, if one does.
fn word_at(s: &[char], k: usize) -> Option<String> {
    if k >= s.len() || !is_word(s[k]) || !boundary_before(s, k) {
        return None;
    }
    let mut e = k;
    while e < s.len() && is_word(s[e]) {
        e += 1;
    }
    Some(s[k..e].iter().collect())
}

/// The word that ends at `k`, if one does.
fn word_before(s: &[char], k: usize) -> Option<String> {
    if k == 0 || !is_word(s[k - 1]) {
        return None;
    }
    let mut b = k;
    while b > 0 && is_word(s[b - 1]) {
        b -= 1;
    }
    Some(s[b..k].iter().collect())
}

/// The index of the bracket that closes the one at `open`.
fn close_of(s: &[char], open: usize) -> usize {
    let (o, c) = match s[open] {
        '{' => ('{', '}'),
        '(' => ('(', ')'),
        '[' => ('[', ']'),
        _ => unreachable!("not an opening bracket"),
    };
    let mut d = 0;
    for (k, &ch) in s.iter().enumerate().skip(open) {
        if ch == o {
            d += 1;
        } else if ch == c {
            d -= 1;
            if d == 0 {
                return k;
            }
        }
    }
    s.len()
}

/// A file's lines as one text, flat (literals blanked) or as code, with
/// the line each character is on.
fn joined(lines: &[lex::Line], flat: bool) -> (Vec<char>, Vec<usize>) {
    let mut text: Vec<char> = Vec::new();
    let mut line_of: Vec<usize> = Vec::new();
    for (ln, l) in lines.iter().enumerate() {
        let t = if flat { &l.flat } else { &l.code };
        for c in t.chars().chain(std::iter::once('\n')) {
            text.push(c);
            line_of.push(ln);
        }
    }
    (text, line_of)
}

/// Code a file takes from another file than itself, by where each is
/// written in a file's flat text: the word `include` (the macro, called or
/// renamed), and a module's `path` attribute, plain or as an argument of a
/// `cfg_attr`, so code the lists do not name is no way round them.
fn outside_at(s: &[char]) -> Vec<usize> {
    let mut out: Vec<usize> = words(s)
        .into_iter()
        .filter(|(_, w)| w == "include")
        .map(|(at, _)| at)
        .collect();
    for i in 0..s.len() {
        if s[i] != '#' {
            continue;
        }
        let mut k = skip_ws(s, i + 1);
        if s.get(k) == Some(&'!') {
            k = skip_ws(s, k + 1);
        }
        if s.get(k) != Some(&'[') {
            continue;
        }
        let end = close_of(s, k);
        let k = skip_ws(s, k + 1);
        match word_at(s, k).as_deref() {
            Some("path") => out.push(i),
            Some("cfg_attr") => {
                for (at, w) in words(&s[k..end]) {
                    let e = skip_ws(s, k + at + 4);
                    if w == "path" && s.get(e) == Some(&'=') {
                        out.push(k + at);
                    }
                }
            }
            _ => {}
        }
    }
    out.sort();
    out
}

/// How many places code is taken from outside the file, for the unit tests.
fn outside(s: &[char]) -> usize {
    outside_at(s).len()
}

/// Every `vault::NAME` whose NAME is not a door verb, by the place of its
/// `vault`, whitespace and line breaks allowed around the `::`.
fn vault_named(s: &[char]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (at, w) in words(s) {
        if w != "vault" {
            continue;
        }
        let j = skip_ws(s, at + 5);
        if !starts(s, j, "::") {
            continue;
        }
        if let Some(name) = word_at(s, skip_ws(s, j + 2)) {
            if !DOOR.contains(&name.as_str()) {
                out.push((at, format!("vault::{name}")));
            }
        }
    }
    out
}

/// A file's module path from its place under `src/`.
fn module_of(file: &str) -> Vec<String> {
    let rel = file
        .strip_prefix("src/")
        .and_then(|f| f.strip_suffix(".rs"))
        .unwrap_or(file);
    let mut p: Vec<String> = rel.split('/').map(String::from).collect();
    if matches!(p.last().map(String::as_str), Some("lib" | "mod")) {
        p.pop();
    }
    p
}

/// The inline modules of a flat text: (open brace, close brace, name).
fn inline_mods(s: &[char]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (at, w) in words(s) {
        if w != "mod" {
            continue;
        }
        let k = skip_ws(s, at + 3);
        let Some(name) = word_at(s, k) else { continue };
        let o = skip_ws(s, k + name.chars().count());
        if s.get(o) == Some(&'{') {
            out.push((o, close_of(s, o), name));
        }
    }
    out
}

/// Every place a path of an encryption file leaves the encryption modules
/// and the seam: a path from `super` or `crate` (`$crate` too), resolved
/// against the module it is written in, that names something outside them
/// -- a module, an item, a glob, an element of a group -- or ends above
/// them; `use crate as x` and `extern crate`.
fn paths_out(file: &str, s: &[char]) -> Vec<usize> {
    let allowed: Vec<Vec<String>> = ENC.iter().chain(SEAM).map(|f| module_of(f)).collect();
    let mods = inline_mods(s);
    let here = |p: usize| {
        let mut m = module_of(file);
        for (o, c, name) in &mods {
            if *o < p && p < *c {
                m.push(name.clone());
            }
        }
        m
    };
    let mut hits = Vec::new();
    // A group's elements are resolved by the group's walk, from the path
    // before the group, and never again from the module.
    let mut seen = BTreeSet::new();
    for (at, w) in words(s) {
        if w != "super" && w != "crate" || seen.contains(&at) {
            continue;
        }
        let b = back_ws(s, at);
        if b >= 2 && s[b - 1] == ':' && s[b - 2] == ':' {
            // Inside a longer path, but for `self::super`.
            if !(w == "super" && word_before(s, back_ws(s, b - 2)).as_deref() == Some("self")) {
                continue;
            }
        }
        if w == "crate" && word_before(s, b).as_deref() == Some("extern") {
            hits.push(at);
            continue;
        }
        let p = if w == "crate" {
            Vec::new()
        } else {
            let mut h = here(at);
            h.pop();
            h
        };
        let k = skip_ws(s, at + w.len());
        if starts(s, k, "::") {
            walk_path(s, k, p, at, &allowed, &mut hits, &mut seen);
        } else if word_at(s, k).as_deref() == Some("as") && !within(&p, &allowed) {
            hits.push(at);
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

fn within(p: &[String], allowed: &[Vec<String>]) -> bool {
    allowed.iter().any(|a| p.starts_with(a))
}

fn above(p: &[String], allowed: &[Vec<String>]) -> bool {
    allowed.iter().any(|a| a.starts_with(p))
}

/// Follows a path from `k` (at a `::` or past a segment) with `p` the
/// module reached so far; a path that leaves `allowed` is a hit at `at`.
fn walk_path(
    s: &[char],
    mut k: usize,
    mut p: Vec<String>,
    at: usize,
    allowed: &[Vec<String>],
    hits: &mut Vec<usize>,
    seen: &mut BTreeSet<usize>,
) {
    loop {
        // A `super` after any segment climbs before anything is judged.
        let j = skip_ws(s, k);
        if starts(s, j, "::") {
            let w = skip_ws(s, j + 2);
            if word_at(s, w).as_deref() == Some("super") {
                p.pop();
                k = w + 5;
                continue;
            }
        }
        if within(&p, allowed) {
            return;
        }
        if !above(&p, allowed) {
            hits.push(at);
            return;
        }
        let j = skip_ws(s, k);
        if !starts(s, j, "::") {
            // The path ends at the crate or at a module above the allowed ones.
            hits.push(at);
            return;
        }
        let j = skip_ws(s, j + 2);
        if s.get(j) == Some(&'{') {
            path_group(s, j, &p, allowed, hits, seen);
            return;
        }
        let Some(w) = word_at(s, j) else {
            // A glob, or something no path continues with.
            hits.push(at);
            return;
        };
        if w != "self" {
            p.push(w.clone());
        }
        k = j + w.chars().count();
    }
}

/// The elements of a `{..}` group at `open`, each a path from `p`.
fn path_group(
    s: &[char],
    open: usize,
    p: &[String],
    allowed: &[Vec<String>],
    hits: &mut Vec<usize>,
    seen: &mut BTreeSet<usize>,
) {
    let close = close_of(s, open);
    let mut e = open + 1;
    while e < close {
        let k = skip_ws(s, e);
        if k >= close {
            break;
        }
        match s[k] {
            '{' => path_group(s, k, p, allowed, hits, seen),
            '*' if !within(p, allowed) => hits.push(k),
            _ => {
                if let Some(w) = word_at(s, k) {
                    seen.insert(k);
                    let mut q = p.to_vec();
                    match w.as_str() {
                        "self" => {
                            if !within(&q, allowed) {
                                hits.push(k);
                            }
                        }
                        "super" => {
                            q.pop();
                            walk_path(s, k + 5, q, k, allowed, hits, seen);
                        }
                        _ => {
                            q.push(w.clone());
                            walk_path(s, k + w.chars().count(), q, k, allowed, hits, seen);
                        }
                    }
                }
            }
        }
        // On to the element after the next comma at this group's depth.
        let mut d = 0;
        let mut m = k;
        while m < close {
            match s[m] {
                '{' | '(' | '[' => d += 1,
                '}' | ')' | ']' => d -= 1,
                ',' if d == 0 => break,
                _ => {}
            }
            m += 1;
        }
        e = m + 1;
    }
}

/// The names a file defines as types: after `struct`, `enum`, `union`,
/// `trait` and `type`.
fn types_defined(s: &[char]) -> BTreeSet<String> {
    let ws = words(s);
    let mut out = BTreeSet::new();
    for (at, w) in &ws {
        if ["struct", "enum", "union", "trait", "type"].contains(&w.as_str()) {
            if let Some(name) = word_at(s, skip_ws(s, at + w.len())) {
                out.insert(name);
            }
        }
    }
    out
}

/// The leaves of every `use` tree of a flat text: each path's segments
/// from its root, groups expanded, and the name a leaf is renamed to.
fn use_leaves(s: &[char]) -> Vec<(Vec<String>, Option<String>)> {
    fn tree(
        s: &[char],
        mut k: usize,
        end: usize,
        mut segs: Vec<String>,
        out: &mut Vec<(Vec<String>, Option<String>)>,
    ) -> usize {
        loop {
            k = skip_ws(s, k);
            if starts(s, k, "::") {
                k += 2;
                continue;
            }
            if k >= end {
                out.push((segs, None));
                return k;
            }
            match s[k] {
                '{' => {
                    let close = close_of(s, k).min(end);
                    let mut e = k + 1;
                    while e < close {
                        e = skip_ws(s, tree(s, e, close, segs.clone(), out));
                        if s.get(e) == Some(&',') {
                            e += 1;
                        } else {
                            break;
                        }
                    }
                    return close + 1;
                }
                '*' => {
                    segs.push("*".into());
                    out.push((segs, None));
                    return k + 1;
                }
                '$' => k += 1,
                _ => {
                    let Some(w) = word_at(s, k) else {
                        out.push((segs, None));
                        return k;
                    };
                    k += w.chars().count();
                    if w == "as" {
                        let a = skip_ws(s, k);
                        let alias = word_at(s, a);
                        let past = a + alias.as_ref().map_or(0, |x| x.chars().count());
                        out.push((segs, alias));
                        return past;
                    }
                    segs.push(w);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (at, w) in words(s) {
        if w != "use" {
            continue;
        }
        let end = (at + 3..s.len()).find(|&k| s[k] == ';').unwrap_or(s.len());
        tree(s, at + 3, end, Vec::new(), &mut out);
    }
    out
}

/// The names a file gives a seam or encryption type: a `use` that renames
/// a path through the seam or an encryption module, or naming such a type,
/// and a `type` alias whose right side names one, to a fixed point.
fn enc_aliases(s: &[char], enc_types: &BTreeSet<String>) -> BTreeSet<String> {
    let through = |segs: &[String]| {
        segs.iter()
            .any(|w| w == "bytes" || ENC_MODS.contains(&w.as_str()))
            || segs.last().is_some_and(|w| enc_types.contains(w))
    };
    let mut out: BTreeSet<String> = use_leaves(s)
        .into_iter()
        .filter_map(|(segs, alias)| {
            let segs: Vec<String> = segs.into_iter().filter(|w| w != "self").collect();
            alias.filter(|_| through(&segs))
        })
        .collect();
    let ws = words(s);
    let mut rhs: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    for (at, w) in &ws {
        if w != "type" {
            continue;
        }
        let Some(name) = word_at(s, skip_ws(s, at + 4)) else {
            continue;
        };
        let end = (at + 4..s.len()).find(|&k| s[k] == ';').unwrap_or(s.len());
        let Some(eq) = (at + 4..end).find(|&k| s[k] == '=') else {
            continue;
        };
        rhs.push((
            name,
            words(&s[eq..end])
                .into_iter()
                .map(|(o, w)| (eq + o, w))
                .collect(),
        ));
    }
    loop {
        let before = out.len();
        for (name, ws) in &rhs {
            let names = ws.iter().any(|(o, w)| {
                enc_types.contains(w)
                    || out.contains(w)
                    || ((w == "bytes" || ENC_MODS.contains(&w.as_str()))
                        && starts(s, skip_ws(s, o + w.chars().count()), "::"))
            });
            if names {
                out.insert(name.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Every `impl` item of a flat text: (its place, the name of its self
/// type, the names its generics declare).
fn impls(s: &[char]) -> Vec<(usize, String, BTreeSet<String>)> {
    let mut out = Vec::new();
    for (at, w) in words(s) {
        if w != "impl" {
            continue;
        }
        let b = back_ws(s, at);
        let item = b == 0
            || "};{]".contains(s[b - 1])
            || matches!(word_before(s, b).as_deref(), Some("unsafe" | "default"));
        if !item {
            continue;
        }
        let mut k = skip_ws(s, at + 4);
        let mut generics = BTreeSet::new();
        if s.get(k) == Some(&'<') {
            let mut d = 0;
            let mut e = k;
            while e < s.len() {
                match s[e] {
                    '<' => d += 1,
                    '>' if s[e - 1] != '-' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                e += 1;
            }
            generics.extend(words(&s[k..e]).into_iter().map(|(_, w)| w));
            k = e + 1;
        }
        // The header's words at depth zero, to its body or its `where`.
        let mut d = 0i32;
        let mut top: Vec<String> = Vec::new();
        let mut e = k;
        while e < s.len() {
            let ch = s[e];
            if d == 0 && ch == '{' {
                break;
            }
            match ch {
                '<' | '(' | '[' => d += 1,
                '>' if s[e - 1] != '-' => d -= 1,
                ')' | ']' => d -= 1,
                _ => {}
            }
            if let Some(w) = word_at(s, e) {
                let n = w.chars().count();
                if d == 0 {
                    if w == "where" {
                        break;
                    }
                    if !(e > 0 && s[e - 1] == '\'') {
                        top.push(w);
                    }
                }
                e += n;
                continue;
            }
            e += 1;
        }
        // The self type is the header's last path, after any `for`.
        let name = top
            .iter()
            .rev()
            .find(|w| !["mut", "dyn", "const", "crate", "super", "self"].contains(&w.as_str()));
        if let Some(name) = name {
            out.push((at, name.clone(), generics));
        }
    }
    out
}

/// The macros the standard library gives every file.
const STD_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "column",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "file",
    "format",
    "format_args",
    "include_bytes",
    "include_str",
    "line",
    "matches",
    "module_path",
    "option_env",
    "panic",
    "print",
    "println",
    "stringify",
    "thread_local",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// The keywords a `!` can follow in an expression (`if !x`, `return !x`).
const KEYWORDS: &[&str] = &[
    "as", "await", "box", "break", "const", "continue", "else", "for", "if", "in", "let", "loop",
    "match", "move", "mut", "ref", "return", "static", "unsafe", "where", "while", "yield",
];

/// The names a flat text's `macro_rules` define.
fn macros_defined(s: &[char]) -> BTreeSet<String> {
    words(s)
        .iter()
        .filter(|(at, w)| w == "macro_rules" && s.get(skip_ws(s, at + 11)) == Some(&'!'))
        .filter_map(|(at, _)| word_at(s, skip_ws(s, skip_ws(s, at + 11) + 1)))
        .collect()
}

/// Every macro call of a flat text to a macro neither the standard library
/// nor the file defines: a macro defined in a parent module before the
/// file's `mod` is in scope there by its bare name, and carries its body,
/// and under a std macro's name it shadows the std one, so a std name is
/// exempt only where no file under `src/` (`shadowed`) defines it.
fn macros_outside(s: &[char], shadowed: &BTreeSet<String>) -> Vec<usize> {
    let ws = words(s);
    let own = macros_defined(s);
    ws.iter()
        .filter(|(at, w)| {
            let k = skip_ws(s, at + w.chars().count());
            s.get(k) == Some(&'!')
                && matches!(s.get(skip_ws(s, k + 1)), Some('(' | '[' | '{'))
                && w != "include"
                && !KEYWORDS.contains(&w.as_str())
                && !(STD_MACROS.contains(&w.as_str()) && !shadowed.contains(w))
                && !own.contains(w)
        })
        .map(|(at, _)| *at)
        .collect()
}

/// The cipher's vocabulary, counted in the form files, the seam and the
/// crate root.
fn cipher_tokens_in(code: &str, aliases: &[String]) -> Vec<(String, usize)> {
    let s: Vec<char> = code.chars().collect();
    let ws = words(&s);
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut add = |name: &str, n: usize| {
        if n > 0 {
            out.push((name.to_string(), n));
        }
    };
    add("cipher", count_ci(code, "cipher"));
    add("crypt", count_ci(code, "crypt"));
    add("envelope", count_ci(code, "envelope"));
    add(
        "pager",
        ws.iter()
            .filter(|(_, w)| w.starts_with("pager") || w.starts_with("Pager"))
            .count(),
    );
    add(
        "paged",
        ws.iter()
            .filter(|(at, w)| {
                w == "Paged"
                    || w == "Paging"
                    || (w.starts_with("paged_") && w.len() > 6)
                    || (w == "paged" && *at >= 2 && s[at - 2] == ':' && s[at - 1] == ':')
            })
            .count(),
    );
    add(
        "attn/rest word",
        word_in(&s, &["ATTN_PAGED", "REST_HERE", "AHEAD_REST"]),
    );
    add(
        "unlent/unlend",
        word_in(&s, &["unlent", "unlend", "Unlent"]),
    );
    add("MergeRest/NoRest", word_in(&s, &["MergeRest", "NoRest"]));
    add(
        "piecewise/ASK_PIECES",
        word_in(&s, &["piecewise", "ASK_PIECES"]),
    );
    add(
        "run logged",
        word_in(&s, &["RUN_LOGGED", "log_run", "log_run_again", "logged"]),
    );
    add(
        "sealed log/manifest",
        word_in(
            &s,
            &[
                "WAL_SEALED_LEN",
                "log_begin",
                "log_resume",
                "seal_into",
                "open_records",
                "seal_manifest",
                "open_manifest",
                "adopt_all",
                "FileKey",
            ],
        ),
    );
    add(
        "vault/pager method",
        leftmost(
            &s,
            &[
                &|s, i| lit(s, i, ".pager()", false, false),
                &|s, i| lit(s, i, ".set_grace(", false, false),
                &|s, i| lit(s, i, ".crowded(", false, false),
                &|s, i| lit(s, i, ".swamped(", false, false),
                &|s, i| {
                    if !starts(s, i, ".breathe") {
                        return None;
                    }
                    let mut k = i + 8;
                    while k < s.len() && is_word(s[k]) {
                        k += 1;
                    }
                    (s.get(k) == Some(&'(')).then_some(k + 1 - i)
                },
                &|s, i| lit(s, i, "v.persist(", true, false),
            ],
        ) + ws
            .iter()
            .filter(|(at, w)| {
                // `vault` called as a method or by its path: `.vault(..)`,
                // `Held::vault(h)`, spaces allowed where Rust allows them.
                if w != "vault" {
                    return false;
                }
                let mut b = *at;
                while b > 0 && s[b - 1].is_whitespace() {
                    b -= 1;
                }
                let called =
                    b >= 1 && s[b - 1] == '.' || b >= 2 && s[b - 2] == ':' && s[b - 1] == ':';
                let mut k = at + 5;
                while k < s.len() && s[k].is_whitespace() {
                    k += 1;
                }
                called && s.get(k) == Some(&'(')
            })
            .count(),
    );
    add(
        "held build",
        ws.iter()
            .filter(|(at, w)| {
                w == "Walked"
                    && !(*at >= 2 && s[at - 2] == ':' && s[at - 1] == ':')
                    && s.get(at + 6) != Some(&'(')
            })
            .count()
            + leftmost(&s, &[&|s, i| lit(s, i, "SegSrc::Kept", false, false)]),
    );
    add("dirkey", word_in(&s, &["dirkey", "DirKey"]));
    add(
        "crate-root alias of an encryption item",
        ws.iter().filter(|(_, w)| aliases.contains(w)).count(),
    );
    // `use` .. a module of the encryption side, before the statement's `;`.
    let mut uses = 0;
    let mut from = 0;
    for (at, w) in &ws {
        if w != "use" || *at < from {
            continue;
        }
        let end = (at + 3..s.len()).find(|&k| s[k] == ';').unwrap_or(s.len());
        let last = ws.iter().rfind(|(b, x)| {
            *b >= at + 3
                && b + x.len() <= end
                && ["vault", "pager", "crypt", "dirkey"].contains(&x.as_str())
        });
        if let Some((b, x)) = last {
            uses += 1;
            from = b + x.len();
        }
    }
    add("use of an encryption module", uses);
    add(
        "vault (field or module word)",
        ws.iter()
            .filter(|(at, w)| {
                if w != "vault" {
                    return false;
                }
                let k = skip_ws(&s, at + 5);
                !starts(&s, k, "::") && s.get(k) != Some(&'(')
            })
            .count(),
    );
    out
}

/// The forms' vocabulary, counted in the encryption files.
fn form_tokens(code: &str) -> Vec<(String, usize)> {
    let s: Vec<char> = code.chars().collect();
    let ws = words(&s);
    let mut out = Vec::new();
    let mut add = |name: &str, n: usize| {
        if n > 0 {
            out.push((name.to_string(), n));
        }
    };
    add(
        "file name prefix/suffix",
        leftmost(
            &s,
            &[
                &|s, i| lit(s, i, "seg-", false, false),
                &|s, i| lit(s, i, "pcs-", false, false),
                &|s, i| lit(s, i, "par-", true, false),
                &|s, i| lit(s, i, "ord-", true, false),
                &|s, i| lit(s, i, "zpg", false, false),
                &|s, i| lit(s, i, "wal-", true, false),
                &|s, i| lit(s, i, "spare-", true, false),
                &|s, i| lit(s, i, "direct-", true, false),
                &|s, i| lit(s, i, "compact-", true, false),
                &|s, i| lit(s, i, "tier-", true, false),
                &|s, i| lit(s, i, "seal-", true, false),
                &|s, i| lit(s, i, ".sup", false, true),
                &|s, i| lit(s, i, ".oidx", false, true),
            ],
        ),
    );
    add("manifest", count_ci(code, "manifest"));
    add(
        "kind of form file",
        leftmost(
            &s,
            &[
                &|s, i| lit(s, i, "Kind::Seg", false, true),
                &|s, i| lit(s, i, "Kind::Ord", false, true),
                &|s, i| lit(s, i, "Kind::Form", false, true),
                &|s, i| lit(s, i, "\".o\"", false, false),
                &|s, i| lit(s, i, "\".z\"", false, false),
            ],
        ),
    );
    add(
        "seg/ord/form ident",
        word_in(&s, &["seg", "segs", "ord", "form"]),
    );
    add(
        "form type",
        word_in(
            &s,
            &[
                "Blob",
                "SegmentWriter",
                "ZFile",
                "OrdIndex",
                "Wal",
                "MemTable",
                "Manifest",
                "FlatIndex",
            ],
        ) + leftmost(&s, &[&|s, i| lit(s, i, "Db::", true, false)]),
    );
    add(
        "form module path",
        ws.iter()
            .filter(|(at, w)| {
                [
                    "zfile",
                    "zpages",
                    "zselect",
                    "ordindex",
                    "flatindex",
                    "blob",
                    "block",
                    "index",
                    "format",
                    "reserve",
                ]
                .contains(&w.as_str())
                    && starts(&s, at + w.chars().count(), "::")
            })
            .count(),
    );
    const LOG: &[&str] = &[
        "LogCipher",
        "log_begin",
        "log_resume",
        "seal_into",
        "open_records",
        "RECORD_HEAD",
    ];
    add(
        "log record",
        leftmost(
            &s,
            &[
                &|s, i| {
                    if !boundary_before(s, i) || !is_word(s[i]) {
                        return None;
                    }
                    let mut k = i;
                    while k < s.len() && is_word(s[k]) {
                        k += 1;
                    }
                    let w: String = s[i..k].iter().collect();
                    LOG.contains(&w.as_str()).then_some(k - i)
                },
                // A quoted run, as the characters fall, holding the word `log`.
                &|s, i| {
                    let q = if s[i] == 'b' && s.get(i + 1) == Some(&'"') {
                        i + 1
                    } else if s[i] == '"' {
                        i
                    } else {
                        return None;
                    };
                    let r = (q + 1..s.len()).find(|&k| s[k] == '"')?;
                    let inner = &s[q + 1..r];
                    let has = (0..inner.len()).any(|k| {
                        starts(inner, k, "log")
                            && boundary_before(s, q + 1 + k)
                            && boundary_at(s, q + 1 + k + 3)
                    });
                    has.then_some(r + 1 - i)
                },
            ],
        ),
    );
    add(
        "passes(",
        ws.iter()
            .filter(|(at, w)| w == "passes" && s.get(at + 6) == Some(&'('))
            .count(),
    );
    // A macro carries what its body names to wherever it is called, and its
    // call names none of it.
    add("macro", word_in(&s, &["macro_export", "macro_rules"]));
    out
}

/// The items `use` statements take from an encryption module, by the line
/// each is named on: every name after the module's word, the name it is
/// given where it is renamed, `self` as the module; a statement that ends
/// at the module is the module.
fn enc_use_items(lines: &[lex::Line]) -> Vec<(usize, String)> {
    let (text, line_of) = joined(lines, false);
    let ws = words(&text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < ws.len() {
        if ws[i].1 != "use" {
            i += 1;
            continue;
        }
        let end = (ws[i].0..text.len())
            .find(|&k| text[k] == ';')
            .unwrap_or(text.len());
        let stmt: Vec<&(usize, String)> =
            ws[i + 1..].iter().take_while(|(at, _)| *at < end).collect();
        i += 1 + stmt.len();
        let Some(m) = stmt
            .iter()
            .position(|(_, w)| ENC_MODS.contains(&w.as_str()))
        else {
            continue;
        };
        let after = &stmt[m + 1..];
        if after.is_empty() {
            out.push((line_of[stmt[m].0], stmt[m].1.clone()));
        }
        for (j, (at, w)) in after.iter().enumerate() {
            if w == "as" || after.get(j + 1).is_some_and(|(_, x)| x == "as") {
                continue;
            }
            let name = if w == "self" {
                stmt[m].1.clone()
            } else {
                w.clone()
            };
            out.push((line_of[*at], name));
        }
    }
    out
}

/// The names the crate root gives encryption items that no other cipher
/// token already catches wherever they are written.
fn root_aliases(src: &BTreeMap<String, String>) -> Vec<String> {
    let lines = regions(&src[ROOT[0]]);
    let mut out: Vec<String> = enc_use_items(&lines)
        .into_iter()
        .map(|(_, w)| w)
        .filter(|w| cipher_tokens(w).is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

type Key = (String, String, String, String); // file, scope, token, region

/// One counted occurrence, for the message.
struct Hit {
    file: String,
    line: usize,
    scope: String,
    token: String,
    region: String,
    n: usize,
    code: String,
}

/// The files the wall reads: every file under `src/`, by its path, a file
/// that is not UTF-8 read lossily (only a `.rs` file may be there).
fn sources(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("src/ lists")
            .map(|e| e.expect("an entry").path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(&p).expect("a file under src/ reads");
                out.insert(rel, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&root.join("src"), root, &mut out);
    out
}

/// Every coupling the sources hold, by (file, scope, token, region), with
/// each hit; or the files no list names, or a listed file that is gone.
fn measure(src: &BTreeMap<String, String>) -> Result<(BTreeMap<Key, usize>, Vec<Hit>), String> {
    let mut errs = Vec::new();
    for f in src.keys().filter(|f| !f.ends_with(".rs")) {
        errs.push(format!(
            "{f} is not a .rs file the wall reads: code under src/ is in a listed .rs file"
        ));
    }
    for f in src.keys().filter(|f| f.ends_with(".rs")) {
        let listed = [ENC, SEAM, ROOT, FORM]
            .iter()
            .any(|l| l.contains(&f.as_str()));
        if !listed {
            errs.push(format!(
                "{f} is on neither side of the wall: name it in tests/source.rs (ENC, FORM, SEAM or ROOT)"
            ));
        }
    }
    for f in ENC.iter().chain(SEAM).chain(ROOT).chain(FORM) {
        if !src.contains_key(*f) {
            errs.push(format!(
                "{f} is listed in tests/source.rs and is not in src/"
            ));
        }
    }
    if !errs.is_empty() {
        return Err(errs.join("\n"));
    }
    let mut counts = BTreeMap::new();
    let mut hits = Vec::new();
    let aliases = root_aliases(src);
    // The types the seam and the encryption side define, which a form may
    // not give methods to: a method a form gives a seam type is the form's
    // code that the encryption side calls by a neutral name.
    let enc_types: BTreeSet<String> = ENC
        .iter()
        .chain(SEAM)
        .flat_map(|f| {
            let lines = regions(&src[*f]);
            types_defined(&joined(&lines, true).0)
        })
        .collect();
    // The macros defined under src/, which shadow a std macro of the same
    // name wherever their textual scope reaches (one on the encryption side
    // is counted there as a `macro` already).
    let shadowed: BTreeSet<String> = src
        .iter()
        .filter(|(f, _)| f.ends_with(".rs"))
        .flat_map(|(_, t)| macros_defined(&joined(&regions(t), true).0))
        .collect();
    let sides: [(&[&str], Tokens, bool); 4] = [
        (FORM, cipher_tokens_in, true),
        (SEAM, cipher_tokens_in, true),
        (ROOT, cipher_tokens_in, true),
        (ENC, |c, _| form_tokens(c), false),
    ];
    for (files, tokens, cipher_side) in sides {
        for f in files {
            let lines = regions(&src[*f]);
            let mut per_line: Vec<Vec<(String, usize)>> =
                lines.iter().map(|l| tokens(&l.code, &aliases)).collect();
            let mut bump =
                |ln: usize, tok: &str| match per_line[ln].iter_mut().find(|(t, _)| t == tok) {
                    Some(e) => e.1 += 1,
                    None => per_line[ln].push((tok.to_string(), 1)),
                };
            let (flat, line_flat) = joined(&lines, true);
            for at in outside_at(&flat) {
                bump(line_flat[at], "code from outside the file");
            }
            let own = types_defined(&flat);
            if cipher_side {
                for (ln, _) in enc_use_items(&lines) {
                    bump(ln, "item used from an encryption module");
                }
                // `extern crate self as x` names the crate by a word no path
                // from `super` or `crate` starts with.
                for (at, w) in words(&flat) {
                    if w == "crate"
                        && word_before(&flat, back_ws(&flat, at)).as_deref() == Some("extern")
                    {
                        bump(line_flat[at], "extern crate");
                    }
                }
                let (code, line_code) = joined(&lines, false);
                for (at, tok) in vault_named(&code) {
                    bump(line_code[at], &tok);
                }
                // A self type is resolved through the file's renames and
                // aliases first: `impl M` of `use crate::bytes::MmapBytes as
                // M` or `type M = crate::bytes::MmapBytes` is MmapBytes's.
                let aliased = enc_aliases(&flat, &enc_types);
                for (at, name, generics) in impls(&flat) {
                    let enc = enc_types.contains(&name) && !own.contains(&name)
                        || aliased.contains(&name);
                    if enc && !generics.contains(&name) {
                        bump(line_flat[at], "impl of a seam or encryption type");
                    }
                }
                // The seam is named by both sides, so a path of its own out
                // of the seam and the encryption modules is the vault's way
                // to the forms by one re-export.
                if SEAM.contains(f) {
                    for at in paths_out(f, &flat) {
                        bump(line_flat[at], "path out of the encryption modules");
                    }
                }
            } else {
                for at in paths_out(f, &flat) {
                    bump(line_flat[at], "path out of the encryption modules");
                }
                for (at, name, generics) in impls(&flat) {
                    if !own.contains(&name) && !generics.contains(&name) {
                        bump(line_flat[at], "impl of a type the file does not define");
                    }
                }
                for at in macros_outside(&flat, &shadowed) {
                    bump(line_flat[at], "macro from outside the file");
                }
            }
            for (ln, (l, toks)) in lines.into_iter().zip(per_line).enumerate() {
                let region = if l.test { "test" } else { "code" };
                for (tok, n) in toks {
                    let k = (
                        f.to_string(),
                        l.scope.clone(),
                        tok.clone(),
                        region.to_string(),
                    );
                    *counts.entry(k).or_insert(0) += n;
                    hits.push(Hit {
                        file: f.to_string(),
                        line: ln + 1,
                        scope: l.scope.clone(),
                        token: tok,
                        region: region.into(),
                        n,
                        code: l.code.trim().chars().take(160).collect(),
                    });
                }
            }
        }
    }
    Ok((counts, hits))
}

/// A listed line: its count, stage and reason.
struct Listed {
    count: usize,
    stage: String,
    reason: String,
}

fn parse(text: &str) -> Result<BTreeMap<Key, Listed>, String> {
    let mut out = BTreeMap::new();
    for (i, l) in text.lines().enumerate() {
        if l.starts_with('#') || l.trim().is_empty() {
            continue;
        }
        let c: Vec<&str> = l.split('\t').collect();
        let bad = |why: &str| format!("tests/data/couplings.txt line {}: {why}: {l}", i + 1);
        if c.len() < 6 {
            return Err(bad("fewer than six columns"));
        }
        let count: usize = c[4]
            .parse()
            .map_err(|_| bad("a count that is not a number"))?;
        if !["code", "test"].contains(&c[3]) {
            return Err(bad("a region that is neither code nor test"));
        }
        if !STAGES.contains(&c[5]) {
            return Err(bad("a stage not in tests/source.rs's STAGES"));
        }
        let k = (c[0].into(), c[1].into(), c[2].into(), c[3].into());
        let v = Listed {
            count,
            stage: c[5].into(),
            reason: c.get(6).unwrap_or(&"").to_string(),
        };
        if out.insert(k, v).is_some() {
            return Err(bad("a key listed twice"));
        }
    }
    Ok(out)
}

/// The wall's verdict: the measured totals by (file, token, region) against
/// the listed ones.
fn check(src: &BTreeMap<String, String>, listed: &str) -> Result<(), String> {
    let listed = parse(listed)?;
    let (measured, hits) = measure(src)?;
    let total = |m: &mut BTreeMap<(String, String, String), usize>, k: &Key, n: usize| {
        *m.entry((k.0.clone(), k.2.clone(), k.3.clone()))
            .or_insert(0) += n;
    };
    let mut have = BTreeMap::new();
    for (k, n) in &measured {
        total(&mut have, k, *n);
    }
    let mut want = BTreeMap::new();
    for (k, l) in &listed {
        total(&mut want, k, l.count);
    }
    let keys: BTreeSet<_> = have.keys().chain(want.keys()).cloned().collect();
    let mut errs = Vec::new();
    for k in keys {
        let (h, w) = (
            have.get(&k).copied().unwrap_or(0),
            want.get(&k).copied().unwrap_or(0),
        );
        if h == w {
            continue;
        }
        let (file, tok, region) = &k;
        let mut msg = if h > w {
            format!("{file} {tok} {region}: {h} > {w}: a coupling grew")
        } else {
            let stages: BTreeSet<&str> = listed
                .iter()
                .filter(|(lk, _)| (&lk.0, &lk.2, &lk.3) == (file, tok, region))
                .map(|(_, l)| l.stage.as_str())
                .collect();
            format!(
                "{file} {tok} {region}: {h} < {w}: lower tests/data/couplings.txt (stage {})",
                stages.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        msg.push_str("\n  measured by scope:");
        for (mk, n) in measured
            .iter()
            .filter(|(mk, _)| (&mk.0, &mk.2, &mk.3) == (file, tok, region))
        {
            let w = listed.get(mk).map_or(0, |l| l.count);
            msg.push_str(&format!("\n    {}: {n} (listed {w})", mk.1));
        }
        for (lk, l) in listed
            .iter()
            .filter(|(lk, _)| (&lk.0, &lk.2, &lk.3) == (file, tok, region))
        {
            if !measured.contains_key(lk) {
                msg.push_str(&format!(
                    "\n    {}: 0 (listed {}, stage {})",
                    lk.1, l.count, l.stage
                ));
            }
        }
        msg.push_str("\n  hits:");
        for x in hits
            .iter()
            .filter(|x| (&x.file, &x.token, &x.region) == (file, tok, region))
        {
            msg.push_str(&format!(
                "\n    {}:{} [{}] x{}: {}",
                x.file, x.line, x.scope, x.n, x.code
            ));
        }
        errs.push(msg);
    }
    if std::env::var_os("SUPDB_WALL_PRINT").is_some() {
        for (k, n) in &measured {
            let (st, why) = listed
                .get(k)
                .map_or(("?", ""), |l| (l.stage.as_str(), l.reason.as_str()));
            eprintln!("{}\t{}\t{}\t{}\t{n}\t{st}\t{why}", k.0, k.1, k.2, k.3);
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn root() -> PathBuf {
    std::env::var_os("SUPDB_WALL_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn couplings(root: &Path) -> String {
    std::fs::read_to_string(root.join("tests/data/couplings.txt"))
        .expect("tests/data/couplings.txt reads")
}

#[test]
fn the_forms_and_the_cipher_do_not_name_each_other() {
    let root = root();
    if let Err(e) = check(&sources(&root), &couplings(&root)) {
        panic!("the wall between the forms and the encryption modules:\n{e}");
    }
}

#[test]
fn every_file_closes_at_depth_zero() {
    for (f, text) in sources(&root()) {
        if !f.ends_with(".rs") {
            continue;
        }
        if let Err(e) = lex::closes(&text) {
            panic!("{f}: {e}");
        }
    }
}

/// The real tree with `edit` applied to one file's text.
fn edited(file: &str, edit: impl Fn(&str) -> String) -> BTreeMap<String, String> {
    let mut src = sources(&root());
    let t = edit(&src[file]);
    src.insert(file.to_string(), t);
    src
}

/// `text` with `line` put as the first statement of fn `name`.
fn first_statement(text: &str, name: &str, line: &str) -> String {
    let at = text
        .find(&format!("fn {name}("))
        .unwrap_or_else(|| panic!("fn {name} is in the file"));
    let open = at + text[at..].find("{\n").expect("the fn has a body") + 2;
    format!("{}    {line}\n{}", &text[..open], &text[open..])
}

#[test]
fn a_coupling_added_to_a_form_fails_as_growth() {
    let root = root();
    let listed = couplings(&root);
    let src = edited("src/db/zfile.rs", |t| {
        first_statement(t, "warm_file", "super::vault::kept(path);")
    });
    let e = check(&src, &listed).expect_err("a new vault::kept in warm_file is growth");
    assert!(e.contains("src/db/zfile.rs vault::kept code: 2 > 1"), "{e}");
    assert!(
        e.contains("warm_file: 1 (listed 0)"),
        "the scope is named: {e}"
    );

    let src = edited("src/db/zfile.rs", |t| {
        first_statement(t, "open_seg", "let _ = super::vault::kept(path);")
    });
    let e = check(&src, &listed).expect_err("a second vault::kept in open_seg is growth");
    assert!(e.contains("open_seg: 2 (listed 1)"), "{e}");

    let src = edited("src/db/zfile.rs", |t| {
        first_statement(t, "warm_file", "// super::vault::kept(path);")
    });
    check(&src, &listed).expect("a comment is not a coupling");
}

#[test]
fn a_coupling_moved_between_functions_passes() {
    let listed = couplings(&root());
    // `open_seg`'s one `vault::kept` taken out, one put in `warm_file`.
    let src = edited("src/db/zfile.rs", |t| {
        let t = t.replacen("if super::vault::kept(path) {", "if true {", 1);
        first_statement(&t, "warm_file", "super::vault::kept(path);")
    });
    check(&src, &listed).expect("a move changes no total");
}

#[test]
fn a_coupling_removed_fails_until_the_file_is_lowered() {
    let listed = couplings(&root());
    let src = edited("src/db/zfile.rs", |t| {
        t.replacen("if super::vault::kept(path) {", "if true {", 1)
    });
    let e = check(&src, &listed).expect_err("a removal the file still lists");
    assert!(
        e.contains("src/db/zfile.rs vault::kept code: 0 < 1: lower tests/data/couplings.txt"),
        "{e}"
    );
    // Lowered with it, the same tree passes.
    let lowered: String = listed
        .lines()
        .filter(|l| !l.starts_with("src/db/zfile.rs\topen_seg\tvault::kept\tcode\t"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_ne!(lowered.len(), listed.len(), "the line was listed");
    check(&src, &lowered).expect("the lowered file holds the lowered tree");
}

#[test]
fn a_test_coupling_is_counted_apart_from_code() {
    let listed = couplings(&root());
    let src = edited("src/db/zfile.rs", |t| {
        format!("{t}\n#[cfg(all(test, target_os = \"linux\"))]\nmod wall_probe {{\n    fn p() {{ let _ = \"cipher\"; }}\n}}\n")
    });
    let e = check(&src, &listed).expect_err("a cipher word in a form's test");
    assert!(e.contains("src/db/zfile.rs cipher test: 1 > 0"), "{e}");
    assert!(!e.contains("cipher code"), "{e}");
}

#[test]
fn a_file_on_neither_side_fails() {
    let listed = couplings(&root());
    let mut src = sources(&root());
    src.insert("src/db/pin.rs".into(), "pub fn idle() {}\n".into());
    let e = check(&src, &listed).expect_err("an unclassified file");
    assert!(
        e.contains("src/db/pin.rs is on neither side of the wall"),
        "{e}"
    );
    let mut src = sources(&root());
    src.remove("src/db/zlz4.rs");
    let e = check(&src, &listed).expect_err("a listed file gone");
    assert!(e.contains("src/db/zlz4.rs is listed"), "{e}");
}

#[test]
fn the_encryption_side_naming_a_form_fails() {
    let listed = couplings(&root());
    let src = edited("src/db/pager.rs", |t| {
        format!("{t}\npub fn probe() -> &'static str {{ \"seg-\" }}\n")
    });
    let e = check(&src, &listed).expect_err("a form's file name in the pager");
    assert!(
        e.contains("src/db/pager.rs file name prefix/suffix code"),
        "{e}"
    );
}

#[test]
fn a_line_of_the_file_the_gate_cannot_read_fails() {
    let line = "src/db/zfile.rs\topen_seg\tvault::kept\tcode\t1\t";
    assert!(parse(&format!("{line}R4\tx\n")).is_ok());
    let e = parse(&format!("{line}S9\tx\n"))
        .err()
        .expect("a stage nobody planned");
    assert!(e.contains("a stage not in"), "{e}");
    let e = parse(&format!("{line}R4\tx\n{line}S1\ty\n"))
        .err()
        .expect("a key twice");
    assert!(e.contains("a key listed twice"), "{e}");
    let e = parse("src/db/zfile.rs\topen_seg\tvault::kept\tcode\tone\tR4\n")
        .err()
        .expect("a count");
    assert!(e.contains("not a number"), "{e}");
}

#[test]
fn the_tokens_count_as_their_patterns_say() {
    let c = |s: &str| cipher_tokens(s).into_iter().collect::<BTreeMap<_, _>>();
    let line =
        "let p = vault::kept(x) && vault::open(y) && self.vault.is_some() && Encryption::Cipher;";
    let t = c(line);
    let named = vault_named(&line.chars().collect::<Vec<_>>());
    assert_eq!(
        named,
        vec![(8, "vault::kept".to_string())],
        "vault::open is a door verb"
    );
    assert_eq!(t.get("vault (field or module word)"), Some(&1));
    assert_eq!(t.get("crypt"), Some(&1));
    assert_eq!(t.get("cipher"), Some(&1));
    let t = c(
        "PagerGrace m.paged_view(0) MmapBytes::paged(x) Walked::Held PartWalk::Walked Walked(3)",
    );
    assert_eq!(t.get("pager"), Some(&1));
    assert_eq!(t.get("paged"), Some(&2));
    assert_eq!(t.get("held build"), Some(&1));
    let t = c("use super::{pager, vault}; use crate::x;");
    assert_eq!(t.get("use of an encryption module"), Some(&1));
    let f = |s: &str| form_tokens(s).into_iter().collect::<BTreeMap<_, _>>();
    let t = f(
        "let n = format!(\"seg-{x}.sup\"); Db::open(d); \"the log\" + log; Kind::Form passes(x)",
    );
    assert_eq!(t.get("file name prefix/suffix"), Some(&2));
    assert_eq!(t.get("form type"), Some(&1));
    assert_eq!(t.get("log record"), Some(&1));
    assert_eq!(t.get("kind of form file"), Some(&1));
    assert_eq!(t.get("passes("), Some(&1));
}

/// An edit of one file's text.
type Edit<'a> = &'a dyn Fn(&str) -> String;

/// The real tree with each `(file, edit)` applied.
fn edited_all(edits: &[(&str, Edit)]) -> BTreeMap<String, String> {
    let mut src = sources(&root());
    for (file, edit) in edits {
        let t = edit(&src[*file]);
        src.insert(file.to_string(), t);
    }
    src
}

#[test]
fn a_renamed_re_export_at_the_crate_root_fails() {
    let listed = couplings(&root());
    let lib = |t: &str| format!("{t}\npub(crate) use db::vault::kept as keep_named;\n");
    let use_it = |t: &str| first_statement(t, "warm_file", "crate::keep_named(path);");
    let e = check(
        &edited_all(&[("src/lib.rs", &lib), ("src/db/zfile.rs", &use_it)]),
        &listed,
    )
    .expect_err("a vault item re-exported under a neutral name and used by a form");
    assert!(e.contains("src/lib.rs vault::kept code: 1 > 0"), "{e}");
    assert!(
        e.contains("src/db/zfile.rs crate-root alias of an encryption item code: 1 > 0"),
        "{e}"
    );
    // The same name added inside the braces of a re-export the root has.
    let braced = |t: &str| {
        t.replacen(
            "CipherCounts, PagerCounts,",
            "kept as keep_named, CipherCounts, PagerCounts,",
            1,
        )
    };
    let src = edited_all(&[("src/lib.rs", &braced)]);
    assert_ne!(
        src["src/lib.rs"],
        sources(&root())["src/lib.rs"],
        "the edit applied"
    );
    let e = check(&src, &listed).expect_err("one more item re-exported from the vault");
    assert!(
        e.contains("src/lib.rs item used from an encryption module code: 9 > 8"),
        "{e}"
    );
}

#[test]
fn a_crate_root_alias_named_by_a_form_fails() {
    let listed = couplings(&root());
    let src = edited("src/db/zfile.rs", |t| {
        first_statement(t, "warm_file", "let _: Option<crate::DirState> = None;")
    });
    let e = check(&src, &listed).expect_err("the kernel key's state named through the root");
    assert!(
        e.contains("src/db/zfile.rs crate-root alias of an encryption item code: 1 > 0"),
        "{e}"
    );
}

#[test]
fn code_from_outside_a_listed_file_fails() {
    let listed = couplings(&root());
    let mut src = edited("src/db/zfile.rs", |t| {
        first_statement(t, "warm_file", "include!(\"zz.in\");")
    });
    src.insert("src/db/zz.in".into(), "super::vault::kept(path);\n".into());
    let e = check(&src, &listed).expect_err("an included file of code");
    assert!(e.contains("src/db/zz.in is not a .rs file"), "{e}");
    src.remove("src/db/zz.in");
    let e = check(&src, &listed).expect_err("an include! in a form");
    assert!(
        e.contains("src/db/zfile.rs code from outside the file code: 1 > 0"),
        "{e}"
    );
    let src = edited("src/db.rs", |t| {
        format!("{t}\n#[path = \"../elsewhere.rs\"]\nmod elsewhere;\n")
    });
    let e = check(&src, &listed).expect_err("a module read from another path");
    assert!(
        e.contains("src/db.rs code from outside the file code: 1 > 0"),
        "{e}"
    );
    let src = edited("src/db/pager.rs", |t| format!("{t}\ninclude!(\"x.rs\");\n"));
    let e = check(&src, &listed).expect_err("an include! on the encryption side");
    assert!(
        e.contains("src/db/pager.rs code from outside the file code: 1 > 0"),
        "{e}"
    );
}

#[test]
fn a_macro_exported_by_the_encryption_side_fails() {
    let listed = couplings(&root());
    let mac = |t: &str| {
        format!("{t}\n#[macro_export]\nmacro_rules! kk {{ ($p:expr) => {{ $crate::db::vault::kept($p) }} }}\n")
    };
    let call = |t: &str| first_statement(t, "warm_file", "crate::kk!(path);");
    let e = check(
        &edited_all(&[("src/db/vault.rs", &mac), ("src/db/zfile.rs", &call)]),
        &listed,
    )
    .expect_err("a macro carrying a vault call into a form");
    assert!(e.contains("src/db/vault.rs macro code: 2 > 0"), "{e}");
}

#[test]
fn a_vault_call_spelled_as_a_path_fails() {
    let listed = couplings(&root());
    let src = edited("src/db.rs", |t| {
        first_statement(t, "open_refusals", "let _ = Held::vault(h).counts();")
    });
    let e = check(&src, &listed).expect_err("a vault accessor called by its path");
    assert!(e.contains("src/db.rs vault/pager method code:"), "{e}");
}

#[test]
fn the_file_lines_the_gate_cannot_read_are_named() {
    let e = parse("src/db/zfile.rs\topen_seg\tvault::kept\tcod\t1\tR4\n")
        .err()
        .expect("a region");
    assert!(e.contains("a region that is neither"), "{e}");
    let e = parse("src/db/zfile.rs\topen_seg\tvault::kept\tcode\t1\n")
        .err()
        .expect("five columns");
    assert!(e.contains("fewer than six columns"), "{e}");
}

#[test]
fn the_newer_token_patterns_count_as_they_say() {
    let f = |s: &str| form_tokens(s).into_iter().collect::<BTreeMap<_, _>>();
    let t = f("record- xpar- xord- xwal- respare- xdirect- xcompact- xtier- unseal-");
    assert_eq!(t.get("file name prefix/suffix"), None, "{t:?}");
    let t = f("par- ord- wal- spare- direct- compact- tier- seal-");
    assert_eq!(t.get("file name prefix/suffix"), Some(&8), "{t:?}");
    let c = |s: &str| cipher_tokens(s).into_iter().collect::<BTreeMap<_, _>>();
    let t = c("vault (x); vault\t(y)");
    assert_eq!(t.get("vault (field or module word)"), None, "{t:?}");
    let t = c("Held::vault(h) . vault (x) self.vault() Held::vault");
    assert_eq!(t.get("vault/pager method"), Some(&3), "{t:?}");
    assert_eq!(t.get("vault (field or module word)"), Some(&1), "{t:?}");
}

#[test]
fn a_use_names_its_items_and_a_bare_module_is_one() {
    let lines = regions(
        "use super::vault;\nuse crate::db::{\n    dirkey::{self, DirKey},\n    vault::kept as k,\n};\nuse crate::x;\n",
    );
    let items: Vec<(usize, String)> = enc_use_items(&lines);
    let want: Vec<(usize, String)> = [
        (0, "vault"),
        (2, "dirkey"),
        (2, "DirKey"),
        (3, "vault"),
        (3, "k"),
    ]
    .iter()
    .map(|(l, w)| (*l, w.to_string()))
    .collect();
    assert_eq!(items, want);
}

#[test]
fn code_from_outside_is_every_spelling_of_the_two() {
    let n = |t: &str| outside(&t.chars().collect::<Vec<_>>());
    assert_eq!(
        n("include!(\"a\"); include !(\"b\"); include_str!(\"c\");"),
        2
    );
    assert_eq!(
        n("#[path = \"a.rs\"] mod a; # [ path=\"b\"] #![path = \"c\"] #[pathx]"),
        3
    );
    assert_eq!(
        n("#[cfg_attr(all(), path = \"a\")] #[cfg_attr(x, cfg_attr(y, path=\"b\"))]\n#[\n    path = \"c\"\n]"),
        3
    );
    assert_eq!(
        n("#[cfg_attr(x, doc = \"path\")] #[cfg_attr(x, derive(path))] path = q"),
        0
    );
    assert_eq!(n("use std::include as inc;"), 1);
}

#[test]
fn the_encryption_side_naming_the_engine_fails() {
    let listed = couplings(&root());
    // A method given to the engine in the vault, called by a form.
    let vault = |t: &str| {
        format!("{t}\nimpl super::Db {{\n    pub(crate) fn wall_probe(&self, p: &Path) -> bool {{\n        kept(p)\n    }}\n}}\n")
    };
    let call = |t: &str| {
        first_statement(
            t,
            "open_refusals",
            "let _ = self.wall_probe(Path::new(\"x\"));",
        )
    };
    let e = check(
        &edited_all(&[("src/db/vault.rs", &vault), ("src/db.rs", &call)]),
        &listed,
    )
    .expect_err("impl super::Db in the vault");
    assert!(
        e.contains("src/db/vault.rs path out of the encryption modules code: 3 > 2"),
        "{e}"
    );
    assert!(
        e.contains("src/db/vault.rs impl of a type the file does not define code: 1 > 0"),
        "{e}"
    );
    // The engine's items taken by a `use` of a group.
    let src = edited("src/db/vault.rs", |t| {
        format!("{t}\n#[allow(unused_imports)]\nuse super::{{Options, Shared}};\n")
    });
    let e = check(&src, &listed).expect_err("use super::{Options, Shared}");
    assert!(
        e.contains("src/db/vault.rs path out of the encryption modules code: 4 > 2"),
        "{e}"
    );
    // A method given to a seam type by the pager, named by a form.
    let pager = |t: &str| {
        format!("{t}\nimpl crate::bytes::MmapBytes {{\n    pub(crate) fn wall_probe(&self) -> bool {{\n        true\n    }}\n}}\n")
    };
    let e = check(&edited_all(&[("src/db/pager.rs", &pager)]), &listed)
        .expect_err("the pager gives MmapBytes a method");
    assert!(
        e.contains("src/db/pager.rs impl of a type the file does not define code: 1 > 0"),
        "{e}"
    );
    assert!(!e.contains("path out"), "crate::bytes is the seam: {e}");
    // The same, given by a form for the encryption side to call.
    let zfile = |t: &str| {
        format!("{t}\nimpl crate::bytes::MmapBytes {{\n    pub(crate) fn wall_probe(&self) -> bool {{\n        true\n    }}\n}}\n")
    };
    let e = check(&edited_all(&[("src/db/zfile.rs", &zfile)]), &listed)
        .expect_err("a form gives MmapBytes a method");
    assert!(
        e.contains("src/db/zfile.rs impl of a seam or encryption type code: 1 > 0"),
        "{e}"
    );
    // A macro of the engine's in scope by its bare name.
    let mac = |t: &str| {
        format!("{t}\nmacro_rules! wall_probe {{\n    () => {{\n        Db::ord_name_for\n    }};\n}}\n")
    };
    let use_mac = |t: &str| first_statement(t, "kept", "let _ = wall_probe!();");
    let e = check(
        &edited_all(&[("src/db/blocks.rs", &mac), ("src/db/vault.rs", &use_mac)]),
        &listed,
    )
    .expect_err("a macro the vault does not define");
    assert!(
        e.contains("src/db/vault.rs macro from outside the file code: 1 > 0"),
        "{e}"
    );
    // A path inside the encryption side passes.
    let src = edited("src/db/pager.rs", |t| {
        first_statement(t, "crowded", "let _ = super::vault::kept;")
    });
    check(&src, &listed).expect("the pager naming the vault");
}

#[test]
fn a_path_out_is_resolved_against_its_module() {
    let hits = |f: &str, t: &str| {
        let s: Vec<char> = t.chars().collect();
        paths_out(f, &s)
            .into_iter()
            .map(|at| {
                s[at..]
                    .iter()
                    .take_while(|c| is_word(**c))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    let t = "super::zfile::x(); crate::bytes::y(); use super::pager::{self, A};\n\
             use super::{Options, vault::kept, Shared};\n\
             mod tests { use super::*; fn f() { super::super::Db::x(); } }\n\
             use crate::db::{Db, pager, {vault}};\n\
             pub(crate) fn g() {} pub(super) fn h() {} pub(in crate::db::vault) fn i() {}\n\
             use crate as c; extern crate core; use super::*; use crate::db;\n\
             self::super::Reader::r(); $crate::db::vault::kept(p);\n\
             super :: \n Shared::x(); crate::db::{self};\n\
             use super::{pager, *};\n";
    assert_eq!(
        hits("src/db/vault.rs", t),
        [
            "super", "Options", "Shared", "super", "Db", "crate", "crate", "super", "crate",
            "super", "super", "self", ""
        ]
    );
    // From crypt.rs `super` is the crate: its bytes are the seam.
    assert_eq!(
        hits("src/crypt.rs", "super::bytes::B; super::db::Db;"),
        ["super"]
    );
    // A dirkey's inner module names dirkey by `super`.
    assert!(hits("src/db/dirkey.rs", "mod sys { use super::{DirKey}; }").is_empty());
}

#[test]
fn an_impl_names_its_self_type() {
    let names = |t: &str| {
        impls(&t.chars().collect::<Vec<_>>())
            .into_iter()
            .map(|(_, n, g)| (n, g.into_iter().collect::<Vec<_>>().join(",")))
            .collect::<Vec<_>>()
    };
    let t = "impl<A: Aead + ?Sized> Aead for &A {}\nimpl crate::bytes::MmapBytes {}\n\
             unsafe impl Send for Paged {}\nfn f() -> impl Iterator<Item = u8> { x }\n\
             impl<'a> Filling<'a> {}\nimpl<F: Fn() -> u8> X for Y<F> where for<'b> F: Z {}\n\
             fn g(x: impl AsRef<Path>) {}\n#[cfg(test)]\nimpl Encrypted<B, A> {}\nimpl dyn Probe + 'static {}\n";
    let got = names(t);
    let want: Vec<(String, String)> = [
        ("A", "A,Aead,Sized"),
        ("MmapBytes", ""),
        ("Paged", ""),
        ("Filling", "a"),
        ("Y", "F,Fn,u8"),
        ("Encrypted", ""),
        ("Probe", ""),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(got, want);
    let d = types_defined(
        &"pub struct S; enum E {} trait T {} type Y = u8; union U {}"
            .chars()
            .collect::<Vec<_>>(),
    );
    assert_eq!(d.into_iter().collect::<Vec<_>>(), ["E", "S", "T", "U", "Y"]);
}

#[test]
fn a_macro_from_outside_is_a_call_the_file_does_not_define() {
    let n = |t: &str| macros_outside(&t.chars().collect::<Vec<_>>(), &BTreeSet::new()).len();
    assert_eq!(n("if !(x) {} while !y() {} return !z; a != b; c ! = d;"), 0);
    assert_eq!(n("assert!(a); vec![1]; format!(\"\"); include!(\"x\");"), 0);
    assert_eq!(n("macro_rules! own { () => {} } own!(); own ! [];"), 0);
    assert_eq!(n("probe!(); probe ! {} crate::other!(x);"), 3);
}

#[test]
fn a_vault_name_may_be_spaced_or_broken_across_lines() {
    let n = |t: &str| vault_named(&t.chars().collect::<Vec<_>>());
    assert_eq!(
        n("vault:: kept(p); vault ::kept; vault::\n    kept; vault::open(x)").len(),
        3
    );
    assert_eq!(n("vault::{a}; vault(x); vault.kept").len(), 0);
    let c = |s: &str| cipher_tokens(s).into_iter().collect::<BTreeMap<_, _>>();
    assert_eq!(c("vault :: kept").get("vault (field or module word)"), None);
}

#[test]
fn the_crate_named_by_another_name_fails() {
    let listed = couplings(&root());
    let src = edited("src/lib.rs", |t| {
        format!("{t}\nextern crate self as wall_probe;\n")
    });
    let e = check(&src, &listed).expect_err("the crate under a second name");
    assert!(e.contains("src/lib.rs extern crate code: 1 > 0"), "{e}");
}

#[test]
fn a_crate_root_alias_is_a_whole_word() {
    let aliases = vec!["DirState".to_string()];
    let t = |s: &str| {
        cipher_tokens_in(s, &aliases)
            .into_iter()
            .collect::<BTreeMap<_, _>>()
            .get("crate-root alias of an encryption item")
            .copied()
    };
    assert_eq!(t("let a: MyDirState = DirStateX;"), None);
    assert_eq!(t("let a: crate::DirState = x;"), Some(1));
}

#[test]
fn a_use_of_a_nested_group_reads_to_its_semicolon() {
    let items = |t: &str| {
        enc_use_items(&regions(t))
            .into_iter()
            .map(|(_, w)| w)
            .collect::<Vec<_>>()
    };
    assert_eq!(items("use db::{x::{a}, vault::kept};\n"), ["kept"]);
    assert_eq!(items("use super::vault::kept as k;\n"), ["k"]);
}

#[test]
fn a_code_coupling_moved_into_a_test_fails_by_region() {
    let listed = couplings(&root());
    let src = edited("src/db/zfile.rs", |t| {
        let t = t.replacen("if super::vault::kept(path) {", "if true {", 1);
        format!("{t}\n#[cfg(test)]\nmod wall_probe {{\n    fn p(path: &std::path::Path) -> bool {{\n        super::super::vault::kept(path)\n    }}\n}}\n")
    });
    let e = check(&src, &listed).expect_err("the same total, another region");
    assert!(e.contains("src/db/zfile.rs vault::kept code: 0 < 1"), "{e}");
    assert!(e.contains("src/db/zfile.rs vault::kept test: 1 > 0"), "{e}");
}

#[test]
fn a_seam_type_renamed_or_aliased_by_a_form_fails() {
    let listed = couplings(&root());
    let body =
        "impl WallM {\n    pub(crate) fn wall_probe(&self) -> usize {\n        0\n    }\n}\n";
    for head in [
        "use crate::bytes::MmapBytes as WallM;",
        "type WallM = crate::bytes::MmapBytes;",
        "use crate::{bytes::{MmapBytes as WallM}};",
        "use crate::bytes::MmapBytes as WallN;\ntype WallM = WallN;",
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{t}\n{head}\n{body}"));
        let e = check(&src, &listed).expect_err(head);
        assert!(
            e.contains("src/db/zfile.rs impl of a seam or encryption type code: 1 > 0"),
            "{head}: {e}"
        );
    }
    // A form's own generic that shadows an encryption type's name is the
    // form's type, and so is a rename of a form's own type.
    for text in [
        "trait WallProbe {}\nimpl<Hold: Copy> WallProbe for Hold {}\n",
        "use crate::db::zpages::PageHeader as WallM;\nimpl WallM {}\n",
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{t}\n{text}"));
        check(&src, &listed).expect(text);
    }
}

#[test]
fn a_seam_re_export_of_the_engine_fails() {
    let listed = couplings(&root());
    let seam = |t: &str| format!("{t}\npub(crate) use crate::db::Options as Engine;\n");
    let vault = |t: &str| {
        format!("{t}\nfn wall_probe() -> crate::bytes::Engine {{\n    crate::bytes::Engine::default()\n}}\n")
    };
    let e = check(
        &edited_all(&[("src/bytes.rs", &seam), ("src/db/vault.rs", &vault)]),
        &listed,
    )
    .expect_err("the engine's Options re-exported through the seam");
    // One over what the file lists for the seam's paths out, whatever that is.
    let had: usize = listed
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .filter(|c| c.len() > 4)
        .filter(|c| c[0] == "src/bytes.rs" && c[2] == "path out of the encryption modules")
        .filter(|c| c[3] == "code")
        .map(|c| c[4].parse::<usize>().expect("a count"))
        .sum();
    let want = format!(
        "src/bytes.rs path out of the encryption modules code: {} > {had}",
        had + 1
    );
    assert!(e.contains(&want), "{want}: {e}");
}

#[test]
fn a_form_macro_named_like_a_std_one_fails() {
    let listed = couplings(&root());
    let src = edited("src/db.rs", |t| {
        t.replacen(
            "pub mod vault;",
            "macro_rules! matches {\n    ($e:expr, $p:pat) => {{\n        let _ = crate::db::Db::open_refusals;\n        match $e {\n            $p => true,\n            _ => false,\n        }\n    }};\n}\npub mod vault;",
            1,
        )
    });
    let e = check(&src, &listed).expect_err("a form macro shadowing matches! in the vault");
    assert!(
        e.contains("src/db/vault.rs macro from outside the file code: "),
        "{e}"
    );
}

#[test]
fn a_use_tree_is_read_to_its_leaves() {
    let leaves = |t: &str| {
        use_leaves(&t.chars().collect::<Vec<_>>())
            .into_iter()
            .map(|(p, a)| (p.join("::"), a.unwrap_or_default()))
            .collect::<Vec<_>>()
    };
    let want: Vec<(String, String)> = [
        ("crate::bytes::MmapBytes", "M"),
        ("crate::a", ""),
        ("crate::b::self", "B"),
        ("crate::b::c::*", ""),
        ("crate::d", "_"),
        ("std::x", ""),
        ("crate::bytes::X", "Y"),
    ]
    .iter()
    .map(|(p, a)| (p.to_string(), a.to_string()))
    .collect();
    assert_eq!(
        leaves("use crate::bytes::MmapBytes as M; use crate::{a, b::{self as B, c::*}, d as _};\nuse ::std::x; use $crate::bytes::X as Y;"),
        want
    );
    let types: BTreeSet<String> = ["MmapBytes".to_string()].into();
    let aliased = |t: &str| {
        enc_aliases(&t.chars().collect::<Vec<_>>(), &types)
            .into_iter()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        aliased("use crate::db::vault::{self as V, Vault as W}; use crate::x::MmapBytes as M;"),
        ["M", "V", "W"]
    );
    assert_eq!(
        aliased("type A = B; type B = Arc<C>; type C = crate::bytes::Q; type D = MmapBytes;"),
        ["A", "B", "C", "D"]
    );
    assert_eq!(
        aliased("use crate::zfile::ZFile as M; type T = u8; type U = bytes; trait X { type Y; }"),
        Vec::<String>::new()
    );
}

#[test]
fn every_std_macro_is_exempt_until_a_form_defines_one_by_its_name() {
    // Written out, not built from the list, so an entry dropped from the
    // list is a call this test makes and the list no longer exempts.
    let calls: String = [
        "assert!(x); assert_eq!(x); assert_ne!(x); cfg!(x); column!(x); ",
        "compile_error!(x); concat!(x); dbg!(x); debug_assert!(x); ",
        "debug_assert_eq!(x); debug_assert_ne!(x); env!(x); eprint!(x); ",
        "eprintln!(x); file!(x); format!(x); format_args!(x); ",
        "include_bytes!(x); include_str!(x); line!(x); matches!(x); ",
        "module_path!(x); option_env!(x); panic!(x); print!(x); println!(x); ",
        "stringify!(x); thread_local!(x); todo!(x); unimplemented!(x); ",
        "unreachable!(x); vec!(x); write!(x); writeln!(x); ",
    ]
    .concat();
    let s: Vec<char> = calls.chars().collect();
    assert_eq!(macros_outside(&s, &BTreeSet::new()).len(), 0);
    let all: BTreeSet<String> = STD_MACROS.iter().map(|m| m.to_string()).collect();
    assert_eq!(macros_outside(&s, &all).len(), STD_MACROS.len());
    assert_eq!(STD_MACROS.len(), 34, "a std macro added is added here too");
    let one: BTreeSet<String> = ["matches".to_string()].into();
    assert_eq!(macros_outside(&s, &one).len(), 1);
    // The file's own definition of a std name is the file's.
    let own: Vec<char> = "macro_rules! matches { () => {} } matches!();"
        .chars()
        .collect();
    assert_eq!(macros_outside(&own, &one).len(), 0);
    assert_eq!(
        macros_defined(
            &"macro_rules! a {} macro_rules ! b {} macro_rules c"
                .chars()
                .collect::<Vec<_>>()
        )
        .into_iter()
        .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn a_super_inside_a_group_climbs_and_a_super_alias_inside_is_inside() {
    let n = |f: &str, t: &str| paths_out(f, &t.chars().collect::<Vec<_>>()).len();
    // From the vault: `super` is db, a group's `super` is the crate.
    assert_eq!(n("src/db/vault.rs", "use super::{super::bytes::B};"), 0);
    assert_eq!(n("src/db/vault.rs", "use super::{super::zfile::Z};"), 1);
    // A module inside the vault naming the vault by `super as`.
    assert_eq!(n("src/db/vault.rs", "mod t { use super as v; }"), 0);
    assert_eq!(n("src/db/vault.rs", "mod t { use self::super as v; }"), 0);
    assert_eq!(
        n("src/db/vault.rs", "use super as d; use self::super as e;"),
        2
    );
}

// ---------------------------------------------------------------------------
// The door: what clippy cannot hold.
//
// The door itself is a clippy run over the library (`scripts/door/clippy.toml`,
// the lint group of `scripts/check.sh`): no file, mapping, advice, sync or
// process read outside the modules below, by symbol. What a symbol lint
// cannot say is held here, against `tests/data/door.txt`, each kind of line a
// ratchet whose counts are exact:
// - `allow`: where the lint is silenced. An `allow` or `expect` naming
//   `clippy::disallowed_*`, `clippy::style`, `clippy::all` or `warnings`,
//   plain or inside a `cfg_attr`, by (file, the scope it governs). A new one
//   fails, and so does one moved to another scope, since the list says why
//   each stands where it does.
// - `literal`: a `"/proc` or `"/sys` path written outside the door's modules,
//   by (file, scope), test code aside. A path handed to an allowed call is
//   not a symbol.
// - `thread`: the count of `thread::` per file, the door's modules included
//   (R1 lets the pager spawn and wait, and the count still holds its
//   growth), until the thread half of the door (S1's `pin::` wrappers;
//   decision 11).
//   A `use` that takes a function or a spawning type out of `std::thread`,
//   globs it or renames it would let calls go uncounted, so it fails
//   outright, in every file.
// - `cfgout`: a `cfg` attribute whose predicate does not hold in the door's
//   build (the library under `checked` on Linux x86_64 with glibc), by
//   (file, scope), outside the door's modules: the code under it is code
//   the lint never sees. Inside it, every path the door's config bars is
//   barred as text (`door_texts`), as the old text test of db.rs barred its
//   list. A predicate that names `clippy` fails outright, since the door's
//   run alone sets it, and so does a `cfg_attr` that applies a `cfg`.
// Beside them: no out-of-line module or `include!` in a door module, whose
// file would take the module's allow; no foreign block (`extern "C" { .. }`) outside `src/wasmapi.rs`,
// since a declaration of the crate's own escapes a lint that names libc's;
// and the ordered index's open maps through the door (`vault::map`).
//
// `SUPDB_DOOR_PRINT=1` prints the measured table in the file's format; the
// test never writes the file.
// ---------------------------------------------------------------------------

/// The door's modules: the vault, the byte source, the pager, the envelope,
/// the kernel's key, the engine's anonymous blocks. Each carries the lint's
/// allow at its top, which `tests/data/door.txt` lists like any other.
const DOOR_MODS: &[&str] = &[
    "src/bytes.rs",
    "src/crypt.rs",
    "src/db/blocks.rs",
    "src/db/dirkey.rs",
    "src/db/pager.rs",
    "src/db/vault.rs",
];

/// The kinds of line in `tests/data/door.txt`.
const DOOR_KINDS: &[&str] = &["allow", "literal", "thread", "cfgout"];

type DoorKey = (String, String, String); // kind, file, scope

/// The end of the bracketed span that opens at `open` (a `[` or a `(`).
fn bracket_close(s: &[char], open: usize) -> usize {
    let mut depth = 0i32;
    for (k, &c) in s.iter().enumerate().skip(open) {
        match c {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    s.len()
}

/// Whether an attribute's text silences the door: an `allow(..)` or an
/// `expect(..)` that names one of the door's lints, a group holding them,
/// or a macro's metavariable that may expand to either.
fn silences(attr: &[char]) -> bool {
    for (at, w) in words(attr) {
        if w != "allow" && w != "expect" {
            continue;
        }
        let k = skip_ws(attr, at + w.len());
        if attr.get(k) != Some(&'(') {
            continue;
        }
        let inner: String = attr[k + 1..bracket_close(attr, k)]
            .iter()
            .filter(|c| !c.is_whitespace())
            .collect();
        for name in inner.split(',') {
            // A macro's metavariable (`clippy::$l`, `$l`) may expand to any
            // of them.
            if name.starts_with("clippy::disallowed_")
                || name == "clippy::style"
                || name == "clippy::all"
                || name == "warnings"
                || name.contains('$')
            {
                return true;
            }
        }
    }
    false
}

/// Every attribute of a file that silences the door, by the scope it
/// governs: an inner attribute its enclosing scope (`<file>` at the top),
/// an outer one the scope of the line that holds the first `{` or `;` after
/// it -- the item's body or the statement's end.
fn door_allows(text: &str) -> Vec<(String, usize)> {
    let lines = regions(text);
    let (s, line_of) = joined(&lines, true);
    let mut out = Vec::new();
    let mut spans = Vec::new();
    let mut k = 0;
    while k < s.len() {
        if s[k] != '#' {
            k += 1;
            continue;
        }
        let mut j = k + 1;
        let inner = s.get(j) == Some(&'!');
        if inner {
            j += 1;
        }
        j = skip_ws(&s, j);
        if s.get(j) != Some(&'[') {
            k += 1;
            continue;
        }
        let close = bracket_close(&s, j);
        spans.push((k, close));
        // An attribute a macro's metavariable writes (`#[$a]`) may be any
        // allow at all.
        let metavar = s.get(skip_ws(&s, j + 1)) == Some(&'$');
        if metavar || silences(&s[j..close.min(s.len())]) {
            let ln = line_of[k];
            let scope = if inner {
                // At the top of the file the lexer names no item.
                match lines[ln].scope.as_str() {
                    "<item>" => String::new(),
                    s => s.to_string(),
                }
            } else {
                let at = (close..s.len())
                    .find(|&i| s[i] == '{' || s[i] == ';')
                    .unwrap_or(s.len() - 1);
                lines[line_of[at]].scope.clone()
            };
            let scope = if scope.is_empty() {
                "<file>".to_string()
            } else {
                scope
            };
            out.push((scope, ln + 1));
        }
        k = close + 1;
    }
    // Outside an attribute: an allow handed to a macro as tokens
    // (`quiet!(allow(clippy::disallowed_methods); ..)`), which the macro may
    // write into one, keyed by the scope it is written in.
    for (at, w) in words(&s) {
        if (w != "allow" && w != "expect") || spans.iter().any(|&(a, b)| a <= at && at <= b) {
            continue;
        }
        let o = skip_ws(&s, at + w.len());
        if s.get(o) == Some(&'(') && silences(&s[at..(bracket_close(&s, o) + 1).min(s.len())]) {
            let ln = line_of[at];
            let scope = match lines[ln].scope.as_str() {
                "" | "<item>" => "<file>".to_string(),
                s => s.to_string(),
            };
            out.push((scope, ln + 1));
        }
    }
    out
}

/// The `"/proc` and `"/sys` paths a line's code writes.
fn proc_literals(code: &str) -> usize {
    let c: Vec<char> = code.chars().collect();
    let mut n = 0;
    for i in 0..c.len() {
        for root in ["\"/proc", "\"/sys"] {
            if starts(&c, i, root) && matches!(c.get(i + root.len()), Some('/') | Some('"')) {
                n += 1;
            }
        }
    }
    n
}

/// The `thread::` path segments a line's flat code holds.
fn thread_paths(flat: &str) -> usize {
    let c: Vec<char> = flat.chars().collect();
    words(&c)
        .into_iter()
        .filter(|(at, w)| w == "thread" && starts(&c, at + w.len(), "::"))
        .count()
}

/// The types of `std::thread` a `use` may take: none of them spawns,
/// sleeps or parks, so a call through one is not a thread call the count
/// misses. `Builder` and `Scope` spawn, so they are not here.
const THREAD_TYPES: &[&str] = &[
    "self",
    "JoinHandle",
    "ScopedJoinHandle",
    "Thread",
    "ThreadId",
    "LocalKey",
    "AccessError",
    "Result",
];

/// The `use` statements of a file's flat code that the `thread::` count
/// cannot follow, by line, with what each does: one that takes a function
/// out of `std::thread` (`use std::thread::spawn`), a type that spawns
/// (`Builder`, `Scope`), a glob (`std::thread::*`), or a rename of the
/// module (`use std::thread as t`, `thread::{self as t}`), since a call
/// through any of those holds no `thread::` path. Test code aside.
fn thread_uses(lines: &[lex::Line]) -> Vec<(usize, String)> {
    let (s, line_of) = joined(lines, true);
    let mut out = Vec::new();
    for (at, w) in words(&s) {
        if w != "use" || lines[line_of[at]].test {
            continue;
        }
        let end = (at..s.len()).find(|&i| s[i] == ';').unwrap_or(s.len());
        // Whitespace kept only where it separates two words (`x as y`).
        let mut body = String::new();
        for w in s[at + 3..end].split(|c: &char| c.is_whitespace()) {
            let w: String = w.iter().collect();
            if w.is_empty() {
                continue;
            }
            let joins = |c: Option<char>| c.is_some_and(is_word);
            if joins(body.chars().last()) && joins(w.chars().next()) {
                body.push(' ');
            }
            body.push_str(&w);
        }
        let b: Vec<char> = body.chars().collect();
        for (k, w) in words(&b) {
            if w != "thread" {
                continue;
            }
            let rest: String = b[k + w.len()..].iter().collect();
            let why = if rest.starts_with(" as ") {
                Some("renames std::thread".to_string())
            } else if let Some(r) = rest.strip_prefix("::") {
                if r.starts_with('*') {
                    Some("globs std::thread".to_string())
                } else {
                    // The names the `use` takes out of the module, at the
                    // group's top level.
                    let names: Vec<String> = match r.strip_prefix('{') {
                        Some(g) => {
                            let mut depth = 0i32;
                            let mut cur = String::new();
                            let mut names = Vec::new();
                            for c in g.chars() {
                                match c {
                                    '{' => depth += 1,
                                    '}' if depth == 0 => break,
                                    '}' => depth -= 1,
                                    ',' if depth == 0 => {
                                        names.push(std::mem::take(&mut cur));
                                        continue;
                                    }
                                    _ => {}
                                }
                                cur.push(c);
                            }
                            names.push(cur);
                            names
                        }
                        None => vec![r.split([',', '}']).next().unwrap_or("").to_string()],
                    };
                    names.iter().find_map(|n| {
                        let n = n.trim();
                        let name = n.split(' ').next().unwrap_or("");
                        if name.is_empty() {
                            None
                        } else if !THREAD_TYPES.contains(&name) {
                            Some(if name.chars().next().is_some_and(|c| c.is_lowercase()) {
                                format!("takes a function out of std::thread ({name})")
                            } else {
                                format!("takes {name} out of std::thread")
                            })
                        } else if name == "self" && n.contains(" as ") {
                            Some("renames std::thread".to_string())
                        } else {
                            None
                        }
                    })
                }
            } else {
                None
            };
            if let Some(why) = why {
                out.push((line_of[at] + 1, why));
            }
        }
    }
    out
}

/// A foreign block, by line: every `extern` that is not `extern crate` or a
/// function of an ABI (`extern "C" fn`, a definition or a pointer's type).
/// What is left is a block (`extern "C" {`, `extern {`) or what a macro
/// makes one of (`extern $abi {`, a `ffi!(extern)` handing the keyword
/// over), so it is read by what may follow the keyword, never by what does.
fn foreign_blocks(lines: &[lex::Line]) -> Vec<usize> {
    let (s, line_of) = joined(lines, false);
    let mut out = Vec::new();
    for (at, w) in words(&s) {
        if w != "extern" {
            continue;
        }
        let mut k = skip_ws(&s, at + w.len());
        if s.get(k) == Some(&'"') {
            k += 1;
            while k < s.len() && s[k] != '"' {
                k += 1;
            }
            k = skip_ws(&s, k + 1);
        }
        let next = words(&s[k..(k + 16).min(s.len())])
            .into_iter()
            .next()
            .filter(|(o, _)| *o == 0)
            .map(|(_, n)| n);
        if !matches!(next.as_deref(), Some("fn") | Some("crate")) {
            out.push(line_of[at] + 1);
        }
    }
    out
}

/// Whether `name` (or `name = "value"`) holds in the door's build: the
/// library under the `checked` profile on Linux x86_64 with glibc, which is
/// what `scripts/door/run.sh` lints. `None` for a name the door's build does
/// not decide here (a feature, a `--cfg` of somebody's), which is read as
/// compiled out.
fn door_cfg(name: &str, value: Option<&str>) -> Option<bool> {
    match (name, value) {
        ("debug_assertions" | "unix", None) => Some(true),
        ("test" | "windows" | "doc" | "miri" | "supdb_emulated", None) => Some(false),
        (k, Some(v)) => {
            let have = match k {
                "target_os" => "linux",
                "target_family" => "unix",
                "target_arch" => "x86_64",
                "target_env" => "gnu",
                "target_pointer_width" => "64",
                "target_endian" => "little",
                "target_vendor" => "unknown",
                "panic" => "unwind",
                _ => return None,
            };
            Some(v == have)
        }
        _ => None,
    }
}

/// The top-level arguments of a predicate's list (`a, any(b, c)`).
fn cfg_args(p: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in p.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// A `cfg` predicate's value in the door's build, `None` where it is not
/// decided there.
fn cfg_eval(p: &str) -> Option<bool> {
    let p: String = p.chars().filter(|c| !c.is_whitespace()).collect();
    for op in ["all(", "any(", "not("] {
        if let Some(inner) = p.strip_prefix(op).and_then(|r| r.strip_suffix(')')) {
            let vals: Vec<Option<bool>> = cfg_args(inner).iter().map(|a| cfg_eval(a)).collect();
            return match op {
                "all(" if vals.contains(&Some(false)) => Some(false),
                "all(" if vals.iter().all(|v| *v == Some(true)) => Some(true),
                "any(" if vals.contains(&Some(true)) => Some(true),
                "any(" if vals.iter().all(|v| *v == Some(false)) => Some(false),
                "not(" if vals.len() == 1 => vals[0].map(|v| !v),
                _ => None,
            };
        }
    }
    match p.split_once('=') {
        Some((k, v)) => door_cfg(k, Some(v.trim_matches('"'))),
        None => door_cfg(&p, None),
    }
}

/// Every `cfg` predicate of a file's code, by the place of the attribute
/// or macro (its `#` or its name) and the place just past its closing
/// bracket: `#[cfg(P)]` and `#![cfg(P)]` with whether they are inner,
/// `cfg_attr(P, ..)`'s first argument, and `cfg!(P)`. Also, for a
/// `cfg_attr`, whether what it applies is a `cfg` itself.
struct CfgAt {
    at: usize,
    end: usize,
    /// The predicate as written, and with its literals blanked.
    pred: String,
    pred_flat: String,
    /// `Some(inner)` for a `cfg` attribute, `None` for `cfg_attr` and `cfg!`.
    attr: Option<bool>,
    applies_cfg: bool,
}

fn cfg_preds(s: &[char], code: &[char]) -> Vec<CfgAt> {
    let mut out = Vec::new();
    for (at, w) in words(s) {
        if w != "cfg" && w != "cfg_attr" {
            continue;
        }
        if at > 0 && s[at - 1] == ':' {
            continue;
        }
        let mut k = skip_ws(s, at + w.len());
        let bang = w == "cfg" && s.get(k) == Some(&'!');
        if bang {
            k = skip_ws(s, k + 1);
        }
        if s.get(k) != Some(&'(') {
            continue;
        }
        let close = bracket_close(s, k);
        let body: String = code[k + 1..close.min(s.len())].iter().collect();
        let body_flat: String = s[k + 1..close.min(s.len())].iter().collect();
        // An attribute's `#`, `#!`, `[` before the name.
        let mut b = back_ws(s, at);
        let in_attr = b > 0 && s[b - 1] == '[';
        let mut inner = false;
        let mut start = at;
        if in_attr {
            b = back_ws(s, b - 1);
            if b > 0 && s[b - 1] == '!' {
                inner = true;
                b = back_ws(s, b - 1);
            }
            if b > 0 && s[b - 1] == '#' {
                start = b - 1;
            }
        }
        // An attribute's end is past its `]`.
        let end = match skip_ws(s, close + 1) {
            j if in_attr && s.get(j) == Some(&']') => j + 1,
            _ => close + 1,
        };
        if w == "cfg_attr" {
            let args = cfg_args(&body);
            let flat_args = cfg_args(&body_flat);
            let rest = flat_args.get(1..).unwrap_or(&[]).join(",");
            let applies_cfg = words(&rest.chars().collect::<Vec<_>>())
                .iter()
                .any(|(_, x)| x == "cfg" || x == "cfg_attr");
            out.push(CfgAt {
                at: start,
                end,
                pred: args.first().cloned().unwrap_or_default(),
                pred_flat: flat_args.first().cloned().unwrap_or_default(),
                attr: None,
                applies_cfg,
            });
        } else {
            out.push(CfgAt {
                at: start,
                end,
                pred: body,
                pred_flat: body_flat,
                attr: if bang || !in_attr { None } else { Some(inner) },
                applies_cfg: false,
            });
        }
    }
    out
}

/// The end (exclusive) of what an outer attribute that ends at `from`
/// applies to: the item, statement, field, arm or expression after it,
/// past any further attributes. An inner attribute's (`inner`) is the
/// rest of its enclosing scope.
fn attr_target_end(s: &[char], from: usize, inner: bool) -> usize {
    let mut k = skip_ws(s, from);
    while !inner && s.get(k) == Some(&'#') {
        let j = skip_ws(s, k + 1);
        if s.get(j) != Some(&'[') {
            break;
        }
        k = skip_ws(s, bracket_close(s, j) + 1);
    }
    let item = words(&s[k..(k + 200).min(s.len())])
        .into_iter()
        .take_while(|(_, w)| {
            [
                "pub", "crate", "unsafe", "async", "const", "extern", "default",
            ]
            .contains(&w.as_str())
        })
        .count();
    let first = words(&s[k..(k + 200).min(s.len())])
        .into_iter()
        .nth(item)
        .map(|(_, w)| w)
        .unwrap_or_default();
    let is_item = [
        "fn",
        "struct",
        "enum",
        "union",
        "impl",
        "trait",
        "mod",
        "macro_rules",
    ]
    .contains(&first.as_str());
    let mut depth = 0i32;
    let mut i = k;
    while i < s.len() {
        match s[i] {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth < 0 {
                    return i;
                }
            }
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return i;
                }
                if depth == 0 && !inner {
                    if is_item {
                        return i + 1;
                    }
                    let n = skip_ws(s, i + 1);
                    let next: String = s[n..(n + 4).min(s.len())].iter().collect();
                    if next == "else" || s.get(n) == Some(&'.') || s.get(n) == Some(&'?') {
                        i = n;
                        continue;
                    }
                    return if s.get(n) == Some(&';') { n + 1 } else { i + 1 };
                }
            }
            ';' | ',' if depth == 0 && !inner => return i + 1,
            _ => {}
        }
        i += 1;
    }
    s.len()
}

/// The paths a call past the door is written with, from the door's config:
/// `fs::read`, `File::open`, `libc::remove` (a function's path by its last
/// two segments), `.exists(` (a filesystem question of `Path`'s, which is
/// asked as a method), and a barred type's name (`OpenOptions`, `Mmap`).
/// What the deleted text test of db.rs barred is a subset of it, which
/// `code_the_door_does_not_compile_is_held_by_its_text` holds.
fn door_texts(conf: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for p in door_table(conf, "disallowed-methods") {
        let segs: Vec<&str> = p.split("::").collect();
        let n = segs.len();
        out.insert(format!("{}::{}", segs[n - 2], segs[n - 1]));
        if segs[n - 2] == "Path" {
            out.insert(format!(".{}(", segs[n - 1]));
        }
    }
    for p in door_table(conf, "disallowed-types") {
        out.insert(p.rsplit("::").next().unwrap_or("").to_string());
    }
    out.into_iter().collect()
}

/// Where `text` stands in `s[from..to]` as a whole path (no word
/// character before a word start or after a word end).
fn text_hits(s: &[char], from: usize, to: usize, text: &str) -> Vec<usize> {
    let t: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = from;
    while i + t.len() <= to {
        if s[i..i + t.len()] == t[..]
            && (!is_word(t[0]) || boundary_before(s, i))
            && (!is_word(t[t.len() - 1]) || boundary_at(s, i + t.len()))
        {
            out.push(i);
        }
        i += 1;
    }
    out
}

/// The `use` declarations (and `extern crate`s) in `s[from..to]` that would
/// take a barred call out of reach of `door_texts`, which reads a path by
/// its last two segments: one that renames (`use std::fs as f`, then
/// `f::remove_file`), one that globs (`use std::fs::*`, then a bare
/// `remove_file`), and one that takes out a name some barred function has
/// as its last segment (`use std::fs::remove_file`). By place and what it
/// does.
fn use_hits(s: &[char], from: usize, to: usize, texts: &[String]) -> Vec<(usize, String)> {
    let fns: BTreeSet<&str> = texts
        .iter()
        .filter_map(|t| t.split_once("::").map(|(_, f)| f))
        .filter(|f| f.chars().next().is_some_and(|c| c.is_lowercase()))
        .collect();
    let mut out = Vec::new();
    for (at, w) in words(&s[from..to]) {
        let at = from + at;
        let is_use = w == "use"
            || (w == "extern"
                && words(&s[(at + 6).min(s.len())..(at + 20).min(s.len())])
                    .first()
                    .is_some_and(|(_, n)| n == "crate"));
        if !is_use || (at > 0 && s[at - 1] == ':') {
            continue;
        }
        let end = (at..to).find(|&i| s[i] == ';').unwrap_or(to);
        let body = &s[at + w.len()..end];
        let names = words(body);
        let why = if body.contains(&'*') {
            Some("globs a module".to_string())
        } else if names.iter().any(|(_, n)| n == "as") {
            Some("renames".to_string())
        } else {
            names
                .iter()
                .find(|(_, n)| fns.contains(n.as_str()))
                .map(|(_, n)| format!("takes out `{n}`"))
        };
        if let Some(why) = why {
            out.push((at, why));
        }
    }
    out
}

/// The door's config as the tree holds it.
fn door_conf() -> String {
    std::fs::read_to_string(root().join("scripts/door/clippy.toml"))
        .expect("scripts/door/clippy.toml reads")
}

/// What the door cannot compile, in one file: every `cfg` attribute whose
/// predicate does not hold in the door's build, by the scope it governs (as
/// an allow's is keyed) and the lines it governs, with each barred path written inside what it
/// governs, by line; and what fails outright -- a predicate naming
/// `clippy`, which the door's build alone sets, and a `cfg_attr` that
/// applies a `cfg`. Test code aside, and a door module's code (which may
/// call anything) aside but for the `clippy` rule.
/// The fourth part: the out-of-line modules (`mod name;`) such an
/// attribute governs, whose whole file the door's build does not compile.
type CompiledOut = (
    Vec<(String, usize, usize)>,
    Vec<(usize, String)>,
    Vec<String>,
    Vec<String>,
);

fn compiled_out(lines: &[lex::Line], door: bool, texts: &[String]) -> CompiledOut {
    let (code, line_of) = joined(lines, false);
    let (flat, _) = joined(lines, true);
    assert_eq!(
        code.len(),
        flat.len(),
        "the lexer's code and flat text line up"
    );
    let (mut regions_out, mut hits, mut errs, mut mods) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for c in cfg_preds(&flat, &code) {
        let ln = line_of[c.at];
        let names_clippy = words(&c.pred_flat.chars().collect::<Vec<_>>())
            .iter()
            .any(|(_, w)| w == "clippy");
        if names_clippy {
            errs.push(format!(
                "{}: a cfg predicate names `clippy`, which only the door's run sets: code it compiles out is code the door never sees",
                ln + 1
            ));
            continue;
        }
        if c.applies_cfg {
            errs.push(format!(
                "{}: a cfg_attr that applies a cfg: write the cfg itself, whose predicate the door reads",
                ln + 1
            ));
            continue;
        }
        let Some(inner) = c.attr else { continue };
        if door || lines[ln].test || cfg_eval(&c.pred) == Some(true) {
            continue;
        }
        // Test-only code (`cfg(all(test, ..))`) the lexer marks as test.
        let end = attr_target_end(&flat, c.end, inner);
        let scope = if inner {
            lines[ln].scope.clone()
        } else {
            let at = (c.end..flat.len())
                .find(|&i| flat[i] == '{' || flat[i] == ';')
                .unwrap_or(flat.len() - 1);
            lines[line_of[at]].scope.clone()
        };
        let scope = match scope.as_str() {
            "" | "<item>" => "<file>".to_string(),
            s => s.to_string(),
        };
        regions_out.push((scope, ln + 1, line_of[end.saturating_sub(1).max(c.end)] + 1));
        let body: String = flat[c.end..end].iter().collect();
        let ws: Vec<String> = body
            .split(|c: char| !is_word(c))
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        let vis = ws
            .iter()
            .take_while(|w| *w == "pub" || *w == "crate" || *w == "super" || *w == "self")
            .count();
        if ws.get(vis).map(String::as_str) == Some("mod")
            && ws.len() == vis + 2
            && body.trim_end().ends_with(';')
        {
            mods.push(ws[vis + 1].clone());
        }
        for t in texts {
            for i in text_hits(&flat, c.end, end, t) {
                if !lines[line_of[i]].test {
                    hits.push((line_of[i] + 1, t.clone()));
                }
            }
        }
        for (i, why) in use_hits(&flat, c.end, end, texts) {
            if !lines[line_of[i]].test {
                hits.push((line_of[i] + 1, format!("a use that {why}")));
            }
        }
    }
    hits.sort();
    hits.dedup();
    (regions_out, hits, errs, mods)
}

/// The file of module `name` declared out of line in `parent`.
fn child_file(src: &BTreeMap<String, String>, parent: &str, name: &str) -> Option<String> {
    let dir = match parent
        .strip_suffix("/mod.rs")
        .or_else(|| parent.strip_suffix("/lib.rs"))
    {
        Some(d) => d.to_string(),
        None => parent.trim_end_matches(".rs").to_string(),
    };
    [format!("{dir}/{name}.rs"), format!("{dir}/{name}/mod.rs")]
        .into_iter()
        .find(|f| src.contains_key(f))
}

/// The files the door's build compiles none of: each brought in by an
/// out-of-line module under a `cfg` that does not hold there, and every
/// module such a file declares.
fn files_compiled_out(src: &BTreeMap<String, String>, texts: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut todo: Vec<String> = Vec::new();
    for (f, text) in src.iter().filter(|(f, _)| f.ends_with(".rs")) {
        let (_, _, _, mods) = compiled_out(&regions(text), false, texts);
        todo.extend(mods.iter().filter_map(|m| child_file(src, f, m)));
    }
    while let Some(f) = todo.pop() {
        if out.insert(f.clone()) {
            let lines = regions(&src[&f]);
            let (s, line_of) = joined(&lines, true);
            for (at, w) in words(&s) {
                if w != "mod" || lines[line_of[at]].test {
                    continue;
                }
                let k = skip_ws(&s, at + 3);
                if let Some((0, n)) = words(&s[k..(k + 80).min(s.len())]).first().cloned() {
                    if s.get(skip_ws(&s, k + n.len())) == Some(&';') {
                        todo.extend(child_file(src, &f, &n));
                    }
                }
            }
        }
    }
    out
}

/// The out-of-line modules (`mod name;`) and `include!`s of a door
/// module's code: a file they bring in takes the module's allow with it and
/// is a door module the list does not name.
fn door_children(lines: &[lex::Line]) -> Vec<usize> {
    let (s, line_of) = joined(lines, true);
    let mut out = Vec::new();
    for (at, w) in words(&s) {
        if lines[line_of[at]].test || (at > 0 && s[at - 1] == ':') {
            continue;
        }
        let k = skip_ws(&s, at + w.len());
        let hit = match w.as_str() {
            "mod" => words(&s[k..(k + 80).min(s.len())])
                .first()
                .is_some_and(|(o, n)| *o == 0 && s.get(skip_ws(&s, k + n.len())) == Some(&';')),
            "include" => s.get(k) == Some(&'!'),
            _ => false,
        };
        if hit {
            out.push(line_of[at] + 1);
        }
    }
    out
}

/// What the door's source rules measure, by (kind, file, scope), with the
/// lines of each; and what fails outright.
type DoorMeasured = (BTreeMap<DoorKey, usize>, BTreeMap<DoorKey, Vec<usize>>);

fn door_measure(src: &BTreeMap<String, String>) -> Result<DoorMeasured, String> {
    let mut counts: BTreeMap<DoorKey, usize> = BTreeMap::new();
    let mut at: BTreeMap<DoorKey, Vec<usize>> = BTreeMap::new();
    let mut errs = Vec::new();
    let mut put = |k: DoorKey, n: usize, ln: usize| {
        *counts.entry(k.clone()).or_insert(0) += n;
        at.entry(k).or_default().push(ln);
    };
    let texts = door_texts(&door_conf());
    let whole_out = files_compiled_out(src, &texts);
    for (f, text) in src.iter().filter(|(f, _)| f.ends_with(".rs")) {
        let lines = regions(text);
        for (scope, ln) in door_allows(text) {
            put(("allow".into(), f.clone(), scope), 1, ln);
        }
        let door = DOOR_MODS.contains(&f.as_str());
        let (outs, mut hits, cerrs, _) = compiled_out(&lines, door, &texts);
        if whole_out.contains(f) && !door {
            let (s, line_of) = joined(&lines, true);
            for t in &texts {
                for i in text_hits(&s, 0, s.len(), t) {
                    if !lines[line_of[i]].test {
                        hits.push((line_of[i] + 1, t.clone()));
                    }
                }
            }
            for (i, why) in use_hits(&s, 0, s.len(), &texts) {
                if !lines[line_of[i]].test {
                    hits.push((line_of[i] + 1, format!("a use that {why}")));
                }
            }
        }
        for (scope, ln, last) in outs {
            if std::env::var_os("SUPDB_DOOR_PRINT").is_some() {
                println!("# cfgout {f}:{ln}-{last}");
            }
            put(("cfgout".into(), f.clone(), scope), 1, ln);
        }
        for (ln, t) in hits {
            errs.push(format!(
                "{f}:{ln}: `{t}` in code the door's build compiles out, which its lint never sees: go through the door"
            ));
        }
        for e in cerrs {
            errs.push(format!("{f}:{e}"));
        }
        if door {
            for ln in door_children(&lines) {
                errs.push(format!(
                    "{f}:{ln}: a module or an include brought into a door module's file takes its allow with it: a door module is one file of DOOR_MODS"
                ));
            }
        }
        for (i, l) in lines.iter().enumerate() {
            if l.test {
                continue;
            }
            let n = thread_paths(&l.flat);
            if n > 0 {
                put(("thread".into(), f.clone(), "*".into()), n, i + 1);
            }
            let n = proc_literals(&l.code);
            if n > 0 && !door {
                put(("literal".into(), f.clone(), l.scope.clone()), n, i + 1);
            }
            let c: Vec<char> = l.flat.chars().collect();
            if !door
                && words(&c)
                    .iter()
                    .any(|(at, w)| w == "concat" && c.get(skip_ws(&c, at + w.len())) == Some(&'!'))
            {
                errs.push(format!(
                    "{f}:{}: a string put together by concat! is a literal the /proc rule cannot read: write it whole",
                    i + 1
                ));
            }
        }
        for (ln, why) in thread_uses(&lines) {
            errs.push(format!(
                "{f}:{ln}: a use the thread count cannot follow, it {why}; call it as thread::name, which the count sees"
            ));
        }
        if f != "src/wasmapi.rs" {
            for ln in foreign_blocks(&lines) {
                errs.push(format!(
                    "{f}:{ln}: a foreign block outside src/wasmapi.rs declares what the door's lint names in libc"
                ));
            }
        }
        if f == "src/ordindex.rs" {
            let open: String = lines
                .iter()
                .filter(|l| !l.test && l.scope == "OrdIndex::open")
                .map(|l| l.code.as_str())
                .collect();
            if !open.contains("vault::map(") {
                errs.push("src/ordindex.rs: OrdIndex::open does not map through vault::map".into());
            }
        }
    }
    if errs.is_empty() {
        Ok((counts, at))
    } else {
        Err(errs.join("\n"))
    }
}

/// `tests/data/door.txt`: kind, file, scope, count, reason.
fn door_parse(text: &str) -> Result<BTreeMap<DoorKey, usize>, String> {
    let mut out = BTreeMap::new();
    for (i, l) in text.lines().enumerate() {
        if l.starts_with('#') || l.trim().is_empty() {
            continue;
        }
        let c: Vec<&str> = l.split('\t').collect();
        let bad = |why: &str| format!("tests/data/door.txt line {}: {why}: {l}", i + 1);
        if c.len() < 5 || c[4].trim().is_empty() {
            return Err(bad("five columns, the last a reason"));
        }
        if !DOOR_KINDS.contains(&c[0]) {
            return Err(bad("a kind that is not allow, literal, thread or cfgout"));
        }
        let n: usize = c[3]
            .parse()
            .map_err(|_| bad("a count that is not a number"))?;
        if n == 0 {
            return Err(bad("a count of zero"));
        }
        if out
            .insert((c[0].into(), c[1].into(), c[2].into()), n)
            .is_some()
        {
            return Err(bad("a key listed twice"));
        }
    }
    Ok(out)
}

/// The door's verdict over `src` against the listed table.
fn door_check(src: &BTreeMap<String, String>, listed: &str) -> Result<(), String> {
    let want = door_parse(listed)?;
    let (have, at) = door_measure(src)?;
    if std::env::var_os("SUPDB_DOOR_PRINT").is_some() {
        for ((kind, file, scope), n) in &have {
            println!("{kind}\t{file}\t{scope}\t{n}\t?");
        }
    }
    let keys: BTreeSet<&DoorKey> = have.keys().chain(want.keys()).collect();
    let mut errs = Vec::new();
    for k in keys {
        let (h, w) = (
            have.get(k).copied().unwrap_or(0),
            want.get(k).copied().unwrap_or(0),
        );
        if h == w {
            continue;
        }
        let (kind, file, scope) = k;
        let lines = at
            .get(k)
            .map(|v| {
                v.iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let lines = if lines.is_empty() {
            String::new()
        } else {
            format!(" (lines {lines})")
        };
        errs.push(if h > w {
            format!("{kind} {file} {scope}: {h} > {w} listed: the door grew{lines}")
        } else {
            format!("{kind} {file} {scope}: {h} < {w} listed: lower tests/data/door.txt{lines}")
        });
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn door_listed(root: &Path) -> String {
    std::fs::read_to_string(root.join("tests/data/door.txt")).expect("tests/data/door.txt reads")
}

#[test]
fn the_door_holds_what_clippy_cannot() {
    let root = root();
    if let Err(e) = door_check(&sources(&root), &door_listed(&root)) {
        panic!("the door, where clippy cannot hold it:\n{e}");
    }
}

/// The real tree with `line` put first in fn `name` of `file`, with the
/// listed table, for a red input.
fn door_edited(file: &str, name: &str, line: &str) -> (BTreeMap<String, String>, String) {
    (
        edited(file, |t| first_statement(t, name, line)),
        door_listed(&root()),
    )
}

#[test]
fn a_new_allow_of_the_door_fails_in_each_spelling() {
    let anchor = "drop_written";
    for attr in [
        "#[allow(clippy::disallowed_methods)]",
        "#[allow(clippy::disallowed_types)]",
        "#[expect(clippy::disallowed_methods)]",
        "#[allow(clippy::style)]",
        "#[allow(clippy::all)]",
        "#[allow(warnings)]",
        "#[allow(dead_code, clippy :: disallowed_methods)]",
        "#[cfg_attr(target_os = \"linux\", allow(clippy::disallowed_methods))]",
    ] {
        let (src, listed) = door_edited(
            "src/db/zfile.rs",
            anchor,
            &format!("{attr}\n    let _ = std::fs::read(\"/\");"),
        );
        let e = door_check(&src, &listed).expect_err(attr);
        assert!(
            e.contains("allow src/db/zfile.rs drop_written: 1 > 0 listed"),
            "{attr}: {e}"
        );
    }
    // An allow a macro writes from its input: the attribute names a
    // metavariable, which may expand to the door's lints.
    for mac in [
        "macro_rules! quiet { ($l:ident, $($i:item)*) => { $(#[allow(clippy::$l)] $i)* } }\nquiet!(disallowed_methods, fn door_probe() { let _ = std::fs::read(\"/\"); });\n",
        "macro_rules! quiet { ($l:path, $($i:item)*) => { $(#[allow($l)] $i)* } }\nquiet!(clippy::disallowed_methods, fn door_probe() { let _ = std::fs::read(\"/\"); });\n",
        // The whole attribute a metavariable, the allow handed over as tokens.
        "macro_rules! quiet { ($a:meta; $e:expr) => { #[$a] let _ = $e; } }\nfn door_probe() { quiet!(allow(clippy::disallowed_methods); std::fs::remove_file(\"/\")); }\n",
        // Each half alone: the attribute a metavariable, and the allow
        // handed to a macro written elsewhere.
        "macro_rules! quiet { ($a:meta) => { #[$a] fn door_probe() { let _ = std::fs::read(\"/\"); } } }\n",
        "fn door_probe() { quiet!(allow(clippy::disallowed_methods); std::fs::remove_file(\"/\")); }\n",
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{t}\n{mac}"));
        let e = door_check(&src, &door_listed(&root())).expect_err(mac);
        assert!(
            e.contains("allow src/db/zfile.rs ") && e.contains("the door grew"),
            "{mac}: {e}"
        );
    }
    // On an item, the allow is the item's.
    let src = edited("src/db/zfile.rs", |t| {
        t.replacen(
            "fn drop_written(",
            "#[allow(clippy::disallowed_methods)]\nfn drop_written(",
            1,
        )
    });
    let e = door_check(&src, &door_listed(&root())).expect_err("an item's allow");
    assert!(
        e.contains("allow src/db/zfile.rs drop_written: 1 > 0"),
        "{e}"
    );
    // A module's own, at the top of a file.
    let src = edited("src/db/zselect.rs", |t| {
        format!("#![allow(clippy::disallowed_methods)]\n{t}")
    });
    let e = door_check(&src, &door_listed(&root())).expect_err("a module's allow");
    assert!(e.contains("allow src/db/zselect.rs <file>: 1 > 0"), "{e}");
    // Not the door's lints, or not an allow, or a comment: nothing.
    for line in [
        "#[allow(clippy::needless_return)]\n    let _ = 0;",
        "#[deny(clippy::disallowed_methods)]\n    let _ = 0;",
        "// #[allow(clippy::disallowed_methods)]",
        "let _ = \"#[allow(clippy::disallowed_methods)]\";",
    ] {
        let (src, listed) = door_edited("src/db/zfile.rs", anchor, line);
        door_check(&src, &listed).unwrap_or_else(|e| panic!("{line}: {e}"));
    }
}

#[test]
fn an_allow_moved_or_taken_away_fails_until_the_list_follows() {
    let listed = door_listed(&root());
    // `drop_behind`'s allow moved onto `drop_written`.
    let src = edited("src/db/zfile.rs", |t| {
        let t = t.replacen(
            "fn drop_written(",
            "#[allow(clippy::disallowed_methods)]\nfn drop_written(",
            1,
        );
        let at = t.find("\nfn drop_behind(").expect("drop_behind");
        let allow = t[..at].rfind("#[allow(").expect("drop_behind's allow");
        let end = allow + t[allow..].find('\n').unwrap() + 1;
        format!("{}{}", &t[..allow], &t[end..])
    });
    let e = door_check(&src, &listed).expect_err("a move");
    assert!(e.contains("drop_written: 1 > 0"), "{e}");
    assert!(e.contains("drop_behind: 0 < 1 listed: lower"), "{e}");
}

#[test]
fn a_proc_path_outside_the_door_fails_and_a_tests_does_not() {
    let (src, listed) = door_edited(
        "src/db/zfile.rs",
        "drop_written",
        "let _ = \"/proc/self/status\";",
    );
    let e = door_check(&src, &listed).expect_err("a /proc literal");
    assert!(
        e.contains("literal src/db/zfile.rs drop_written: 1 > 0"),
        "{e}"
    );
    let (src, listed) = door_edited("src/db/zfile.rs", "drop_written", "let _ = \"/sys\";");
    door_check(&src, &listed).expect_err("a /sys literal");
    // A path put together from pieces the rule cannot read.
    let (src, listed) = door_edited(
        "src/db/zfile.rs",
        "drop_written",
        "let _ = concat!(\"/pro\", \"c/self/status\");",
    );
    let e = door_check(&src, &listed).expect_err("a concat! literal");
    assert!(e.contains("put together by concat!"), "{e}");
    // Test code reads what it likes; the door's modules are the door.
    let src = edited("src/db/zfile.rs", |t| {
        format!("{t}\n#[cfg(test)]\nmod door_probe {{\n    fn p() {{ let _ = \"/proc/self/stat\"; }}\n}}\n")
    });
    door_check(&src, &door_listed(&root())).expect("a test's /proc literal");
    let (src, listed) = door_edited("src/db/vault.rs", "sync_dir", "let _ = \"/proc/x\";");
    door_check(&src, &listed).expect("a /proc literal in the door");
    // Not a path's start.
    let (src, listed) = door_edited(
        "src/db/zfile.rs",
        "drop_written",
        "let _ = (\"/process\", \"/system\", \"x/proc/\");",
    );
    door_check(&src, &listed).expect("not /proc or /sys");
}

#[test]
fn a_thread_call_added_fails_and_a_use_of_a_thread_fn_fails() {
    let (src, listed) = door_edited(
        "src/db/zfile.rs",
        "drop_written",
        "std::thread::yield_now();",
    );
    let e = door_check(&src, &listed).expect_err("a thread:: path");
    assert!(e.contains("thread src/db/zfile.rs *:"), "{e}");
    assert!(e.contains("the door grew"), "{e}");
    for u in [
        "use std::thread::spawn;",
        "use std::thread::{park, JoinHandle};",
        "use std::{thread::sleep as nap};",
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{u}\n{t}"));
        let e = door_check(&src, &door_listed(&root())).expect_err(u);
        assert!(
            e.contains("takes a function out of std::thread"),
            "{u}: {e}"
        );
    }
    // A rename, a glob or a type that spawns: a call through any of them
    // holds no `thread::` path.
    for (u, why) in [
        ("use std::thread as t;", "renames std::thread"),
        ("use std::{io, thread as t};", "renames std::thread"),
        ("use std::thread::{self as t};", "renames std::thread"),
        ("use std::thread::*;", "globs std::thread"),
        (
            "use std::thread::Builder;",
            "takes Builder out of std::thread",
        ),
        (
            "use std::thread::{JoinHandle, Scope};",
            "takes Scope out of std::thread",
        ),
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{u}\n{t}"));
        let e = door_check(&src, &door_listed(&root())).expect_err(u);
        assert!(
            e.contains("the thread count cannot follow") && e.contains(why),
            "{u}: {e}"
        );
    }
    // A door module re-exporting the module under another name is a rename
    // too, though the door's own thread calls are not counted.
    let src = edited("src/db/pager.rs", |t| {
        format!("{t}\npub(crate) use std::thread as pth;\n")
    });
    let e = door_check(&src, &door_listed(&root())).expect_err("a re-export");
    assert!(
        e.contains("src/db/pager.rs:") && e.contains("renames std::thread"),
        "{e}"
    );
    for u in [
        "use std::thread;",
        "use std::thread::JoinHandle;",
        "use std::thread::{self, JoinHandle};",
        "use std::thread::{self, JoinHandle as Handle, ThreadId};",
        "use std::io::{self, Read};",
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{u}\n{t}"));
        let e = door_check(&src, &door_listed(&root()));
        assert!(
            !e.is_err_and(|e| e.contains("takes a function")),
            "{u} takes no function"
        );
    }
}

#[test]
fn a_foreign_block_outside_wasmapi_fails() {
    for b in [
        "extern \"C\" {\n    fn madvise(a: *mut u8, n: usize, adv: i32) -> i32;\n}\n",
        "extern {\n    fn open(p: *const u8, f: i32) -> i32;\n}\n",
        "unsafe extern \"C\" {\n    fn sync();\n}\n",
        // Written by a macro: the ABI a metavariable, or the keyword itself
        // handed over.
        "macro_rules! ffi { ($a:literal) => { extern $a { fn unlink(p: *const std::ffi::c_char) -> i32; } } }\nffi!(\"C\");\n",
        "macro_rules! ffi { ($k:tt) => { $k \"C\" { fn unlink(p: *const std::ffi::c_char) -> i32; } } }\nffi!(extern);\n",
    ] {
        let src = edited("src/db/zfile.rs", |t| format!("{t}\n{b}"));
        let e = door_check(&src, &door_listed(&root())).expect_err(b);
        assert!(e.contains("a foreign block outside src/wasmapi.rs"), "{e}");
    }
    // A function of the C ABI, or a pointer to one, declares nothing.
    let src = edited("src/db/zfile.rs", |t| {
        format!("{t}\npub extern \"C\" fn door_probe() {{}}\ntype DoorProbe = unsafe extern \"C\" fn() -> u8;\n")
    });
    door_check(&src, &door_listed(&root())).expect("no foreign block");
}

#[test]
fn the_ordered_index_opens_through_the_door() {
    let src = edited("src/ordindex.rs", |t| {
        t.replacen(
            "crate::db::vault::map(path)?",
            "crate::bytes::MmapBytes::open(path)?",
            1,
        )
    });
    let e = door_check(&src, &door_listed(&root())).expect_err("a map past the door");
    assert!(
        e.contains("OrdIndex::open does not map through vault::map"),
        "{e}"
    );
}

#[test]
fn the_door_table_is_read_as_it_says() {
    for (bad, why) in [
        ("allow\tsrc/x.rs\tf\t1\n", "five columns"),
        ("allow\tsrc/x.rs\tf\t1\t \n", "five columns"),
        ("deny\tsrc/x.rs\tf\t1\tr\n", "a kind"),
        ("allow\tsrc/x.rs\tf\tone\tr\n", "not a number"),
        ("allow\tsrc/x.rs\tf\t0\tr\n", "a count of zero"),
        (
            "allow\tsrc/x.rs\tf\t1\tr\nallow\tsrc/x.rs\tf\t2\tr\n",
            "listed twice",
        ),
    ] {
        let e = door_parse(bad).expect_err(bad);
        assert!(e.contains(why), "{bad}: {e}");
    }
    assert_eq!(door_parse("# a comment\n\n").unwrap().len(), 0);
}

/// The paths of one table of `scripts/door/clippy.toml` (`disallowed-methods`
/// or `disallowed-types`): every quoted string in it that is a path, which a
/// reason is not.
fn door_table(conf: &str, table: &str) -> Vec<String> {
    let head = format!("\n{table} = [");
    let Some(at) = conf.find(&head) else {
        return Vec::new();
    };
    let body = &conf[at + head.len()..];
    let body = &body[..body.find("\n]").unwrap_or(body.len())];
    let body: String = body
        .lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    body.split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| s.contains("::") && !s.contains(' '))
        .map(str::to_string)
        .collect()
}

/// What is wrong with the door's config: a table missing or empty, or an
/// entry whose last segment is not of its table's kind. An entry of the
/// wrong kind may resolve to something no call can name -- a tuple
/// struct's constructor, for a type written into the methods' table --
/// which clippy accepts without a word and bars nothing by.
fn door_conf_check(conf: &str) -> Result<(), String> {
    let mut errs = Vec::new();
    for (table, wants_fn) in [("disallowed-methods", true), ("disallowed-types", false)] {
        let paths = door_table(conf, table);
        if paths.is_empty() {
            errs.push(format!("{table}: no entries"));
        }
        for p in paths {
            let last = p.rsplit("::").next().unwrap_or("");
            let is_fn = last.chars().next().is_some_and(|c| c.is_lowercase());
            if is_fn != wants_fn {
                errs.push(format!(
                    "{table}: `{p}` is a {} path",
                    if is_fn { "function's" } else { "type's" }
                ));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

#[test]
fn each_table_of_the_doors_config_names_its_kind() {
    let conf = std::fs::read_to_string(root().join("scripts/door/clippy.toml"))
        .expect("scripts/door/clippy.toml reads");
    door_conf_check(&conf).unwrap_or_else(|e| panic!("scripts/door/clippy.toml:\n{e}"));
    assert!(door_table(&conf, "disallowed-methods").len() > 50);
    assert!(door_table(&conf, "disallowed-types").contains(&"std::fs::OpenOptions".to_string()));
    // A type moved into the methods' table, and a function into the types'.
    let moved = conf.replacen(
        "disallowed-methods = [\n",
        "disallowed-methods = [\n  \"std::fs::OpenOptions\",\n",
        1,
    );
    let e = door_conf_check(&moved).expect_err("a type among the methods");
    assert!(
        e.contains("disallowed-methods: `std::fs::OpenOptions` is a type's"),
        "{e}"
    );
    let moved = conf.replacen(
        "disallowed-types = [\n",
        "disallowed-types = [\n  { path = \"std::fs::read\", reason = \"a b\" },\n",
        1,
    );
    let e = door_conf_check(&moved).expect_err("a function among the types");
    assert!(
        e.contains("disallowed-types: `std::fs::read` is a function's"),
        "{e}"
    );
    let e = door_conf_check("").expect_err("no tables");
    assert!(e.contains("disallowed-methods: no entries"), "{e}");
}

/// The files of the resolved libc that declare its functions for the
/// door's builds, glibc on x86_64 and on aarch64 (the two Linux legs of CI),
/// relative to the crate's directory; and the directories whose every file
/// is one.
const LIBC_FILES: &[&str] = &[
    "src/unix/mod.rs",
    "src/unix/linux_like/mod.rs",
    "src/unix/linux_like/linux_l4re_shared.rs",
    "src/unix/linux_like/linux/mod.rs",
    "src/unix/linux_like/linux/arch/mod.rs",
    "src/unix/linux_like/linux/arch/generic/mod.rs",
    "src/unix/linux_like/linux/gnu/mod.rs",
    "src/unix/linux_like/linux/gnu/b64/mod.rs",
    "src/unix/linux_like/linux/gnu/b64/x86_64/mod.rs",
    "src/unix/linux_like/linux/gnu/b64/x86_64/not_x32.rs",
    "src/unix/linux_like/linux/gnu/b64/aarch64/mod.rs",
    "src/unix/linux_like/linux/gnu/b64/aarch64/lp64.rs",
];
const LIBC_DIRS: &[&str] = &["src/new/common", "src/new/glibc", "src/new/linux_uapi"];

/// The source of the libc `Cargo.lock` resolves, from cargo's registry.
fn libc_source() -> PathBuf {
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).expect("Cargo.lock reads");
    let at = lock
        .find("name = \"libc\"\nversion = \"")
        .expect("libc in Cargo.lock");
    let rest = &lock[at + "name = \"libc\"\nversion = \"".len()..];
    let version = &rest[..rest.find('"').expect("a version")];
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".cargo"));
    let reg = home.join("registry/src");
    let found = std::fs::read_dir(&reg)
        .unwrap_or_else(|e| {
            panic!(
                "{}: {e}: the libc source the door is held to",
                reg.display()
            )
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(format!("libc-{version}")))
        .find(|p| p.is_dir());
    found.unwrap_or_else(|| panic!("libc-{version} is not under {}", reg.display()))
}

/// Every function the files of `LIBC_FILES` and `LIBC_DIRS` under `dir`
/// declare with a string argument, a `*const c_char` or a `*mut c_char`:
/// the arguments a path is passed in, whatever they are called (`unlink`'s
/// is `c`). By name, with the file that declares it.
fn libc_string_calls(dir: &Path) -> BTreeMap<String, String> {
    let mut files: Vec<String> = LIBC_FILES.iter().map(|f| f.to_string()).collect();
    for d in LIBC_DIRS {
        let mut todo = vec![dir.join(d)];
        while let Some(p) = todo.pop() {
            for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                let q = e.path();
                if q.is_dir() {
                    todo.push(q);
                } else if q.extension().is_some_and(|x| x == "rs") {
                    files.push(q.strip_prefix(dir).unwrap().to_string_lossy().into_owned());
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for f in files {
        let text = std::fs::read_to_string(dir.join(&f))
            .unwrap_or_else(|e| panic!("libc's {f}: {e}: its layout moved, read LIBC_FILES again"));
        let text: String = text
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let s: Vec<char> = text.chars().collect();
        for (at, w) in words(&s) {
            if w != "fn" || !s[..at].ends_with(&['p', 'u', 'b', ' ']) {
                continue;
            }
            let Some((o, name)) = words(&s[at + 2..(at + 80).min(s.len())]).first().cloned() else {
                continue;
            };
            let open = skip_ws(&s, at + 2 + o + name.len());
            if s.get(open) != Some(&'(') {
                continue;
            }
            // The argument list, split at its top-level commas.
            let (mut depth, mut args, mut cur) = (0i32, Vec::new(), String::new());
            for k in open + 1..s.len() {
                let c = s[k];
                match c {
                    '(' | '<' => depth += 1,
                    ')' if depth == 0 => break,
                    ')' => depth -= 1,
                    '>' if s[k - 1] != '-' => depth -= 1,
                    ',' if depth == 0 => {
                        args.push(std::mem::take(&mut cur));
                        continue;
                    }
                    _ => {}
                }
                cur.push(c);
            }
            args.push(cur);
            let names_path = args.iter().any(|a| {
                let Some((_, ty)) = a.split_once(':') else {
                    return false;
                };
                let ty: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
                let ty = ty
                    .strip_prefix("*const")
                    .or_else(|| ty.strip_prefix("*mut"))
                    .unwrap_or("");
                ty.rsplit("::").next() == Some("c_char") && !ty.starts_with('*')
            });
            if names_path {
                out.entry(name).or_insert_with(|| f.clone());
            }
        }
    }
    out
}

/// `tests/data/door-libc.txt`: the libc functions with a string argument
/// that act on no file, each with its reason.
fn door_libc_notfile(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let (n, why) = l.split_once('\t').unwrap_or((l, ""));
            assert!(
                !why.trim().is_empty(),
                "tests/data/door-libc.txt: {l}: a reason"
            );
            (n.to_string(), why.to_string())
        })
        .collect()
}

/// What of `calls` the config does not bar and the list does not excuse,
/// and what of the list names no call or a call the config bars.
fn libc_unclassed(
    calls: &BTreeMap<String, String>,
    barred: &BTreeSet<String>,
    notfile: &BTreeMap<String, String>,
) -> Result<(), String> {
    let mut errs = Vec::new();
    for (n, f) in calls {
        if !barred.contains(n) && !notfile.contains_key(n) {
            errs.push(format!(
                "libc::{n} ({f}) takes a string, which may be a path: bar it in scripts/door/clippy.toml, or say in tests/data/door-libc.txt why it acts on no file"
            ));
        }
    }
    for n in notfile.keys() {
        if !calls.contains_key(n) {
            errs.push(format!(
                "tests/data/door-libc.txt: {n} is no libc call with a string"
            ));
        } else if barred.contains(n) {
            errs.push(format!("tests/data/door-libc.txt: {n} is barred as well"));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

#[test]
fn every_libc_call_that_takes_a_string_is_barred_or_said_to_touch_no_file() {
    let calls = libc_string_calls(&libc_source());
    for n in [
        "open",
        "unlink",
        "mkstemp",
        "execveat",
        "setmntent",
        "utime",
        "posix_spawn_file_actions_addopen",
        "strlen",
    ] {
        assert!(calls.contains_key(n), "the scan finds libc::{n}: {calls:?}");
    }
    assert!(calls.len() > 200, "{}", calls.len());
    let barred: BTreeSet<String> = door_table(&door_conf(), "disallowed-methods")
        .iter()
        .filter_map(|p| p.strip_prefix("libc::").map(str::to_string))
        .collect();
    let notfile = door_libc_notfile(
        &std::fs::read_to_string(root().join("tests/data/door-libc.txt"))
            .expect("tests/data/door-libc.txt reads"),
    );
    libc_unclassed(&calls, &barred, &notfile).unwrap_or_else(|e| panic!("{e}"));
    // An entry taken out of the config, a call the list names wrongly.
    let mut less = barred.clone();
    less.remove("utime");
    let e = libc_unclassed(&calls, &less, &notfile).expect_err("utime unbarred");
    assert!(
        e.contains("libc::utime (src/unix/mod.rs) takes a string"),
        "{e}"
    );
    let mut wrong = notfile.clone();
    wrong.insert("open".into(), "r".into());
    wrong.insert("no_such_fn".into(), "r".into());
    let e = libc_unclassed(&calls, &barred, &wrong).expect_err("a list of wrong names");
    assert!(
        e.contains("open is barred as well") && e.contains("no_such_fn is no libc call"),
        "{e}"
    );
}

#[test]
fn a_cfg_predicate_is_read_as_the_doors_build_reads_it() {
    for (p, want) in [
        ("debug_assertions", Some(true)),
        ("not(debug_assertions)", Some(false)),
        ("target_os = \"linux\"", Some(true)),
        ("not(target_os = \"linux\")", Some(false)),
        (
            "all(target_os = \"linux\", not(supdb_emulated))",
            Some(true),
        ),
        (
            "not(all(target_os = \"linux\", target_env = \"gnu\"))",
            Some(false),
        ),
        (
            "any(target_arch = \"x86_64\", target_arch = \"aarch64\")",
            Some(true),
        ),
        ("target_arch = \"aarch64\"", Some(false)),
        ("not(target_family = \"wasm\")", Some(true)),
        ("windows", Some(false)),
        ("feature = \"x\"", None),
        ("not(somebodys_cfg)", None),
        ("any(windows, somebodys_cfg)", None),
        ("all(windows, somebodys_cfg)", Some(false)),
        // Every name the door's build decides, each way.
        ("unix", Some(true)),
        ("test", Some(false)),
        ("doc", Some(false)),
        ("miri", Some(false)),
        ("supdb_emulated", Some(false)),
        ("target_family = \"unix\"", Some(true)),
        ("target_family = \"windows\"", Some(false)),
        ("target_env = \"gnu\"", Some(true)),
        ("target_env = \"musl\"", Some(false)),
        ("target_pointer_width = \"64\"", Some(true)),
        ("target_pointer_width = \"32\"", Some(false)),
        ("target_endian = \"little\"", Some(true)),
        ("target_endian = \"big\"", Some(false)),
        ("target_vendor = \"unknown\"", Some(true)),
        ("target_vendor = \"apple\"", Some(false)),
        ("panic = \"unwind\"", Some(true)),
        ("panic = \"abort\"", Some(false)),
    ] {
        assert_eq!(cfg_eval(p), want, "{p}");
    }
    // What an outer attribute governs ends with its item, statement, arm
    // or expression, an `else` and a method chain included.
    for (src, governed) in [
        ("fn a() {} fn b() {}", "fn a() {}"),
        (
            "let x = if c { 1 } else { 2 }; y();",
            "let x = if c { 1 } else { 2 };",
        ),
        ("{ a() } b();", "{ a() }"),
        ("A | B => f(x), _ => g(),", "A | B => f(x),"),
        ("use a::{b, c}; d();", "use a::{b, c};"),
        ("x.f() }", "x.f() "),
        ("#[inline] fn a() { b() } c", "#[inline] fn a() { b() }"),
        ("let y = S { a }.f()?; z", "let y = S { a }.f()?;"),
    ] {
        let s: Vec<char> = src.chars().collect();
        let end = attr_target_end(&s, 0, false);
        assert_eq!(s[..end].iter().collect::<String>(), governed, "{src}");
    }
    let s: Vec<char> = "fn a() {} fn b() {} } fn c() {}".chars().collect();
    assert_eq!(
        attr_target_end(&s, 0, true),
        20,
        "an inner attribute's scope"
    );
}

#[test]
fn code_the_door_does_not_compile_is_held_by_its_text() {
    // The deleted text test of db.rs barred these, and the texts the
    // config gives bar each.
    let texts = door_texts(&door_conf());
    for old in [
        "File::open",
        "File::create",
        "OpenOptions",
        "fs::rename",
        "fs::hard_link",
        "fs::remove_file",
        "fs::read_dir",
        "fs::metadata",
        "fs::read",
        "fs::write",
        "fs::copy",
        "MmapBytes::open",
        ".exists(",
        "libc::remove",
        "Command",
    ] {
        assert!(texts.iter().any(|t| t == old), "{old} among {texts:?}");
    }
    // And every entry of the config, a function by its last two segments,
    // a type by its last.
    let conf = door_conf();
    for p in door_table(&conf, "disallowed-methods") {
        let segs: Vec<&str> = p.rsplitn(3, "::").collect();
        let t = format!("{}::{}", segs[1], segs[0]);
        assert!(texts.contains(&t), "{p}: {t} among the texts");
    }
    for p in door_table(&conf, "disallowed-types") {
        let t = p.rsplit("::").next().unwrap().to_string();
        assert!(texts.contains(&t), "{p}: {t} among the texts");
    }
    assert!(
        files_compiled_out(&sources(&root()), &texts).contains("src/wasmapi.rs"),
        "the browser's ABI is a file the door's build does not compile"
    );
    // A barred path in code each kind of predicate compiles out.
    for line in [
        "#[cfg(not(debug_assertions))] let _ = std::path::Path::new(\"/x\").exists();",
        "#[cfg(not(target_os = \"linux\"))] let _ = std::fs::read(\"/x\");",
        "#[cfg(target_arch = \"aarch64\")] unsafe { libc::remove(c\"/x\".as_ptr()) };",
        "#[cfg(somebodys_cfg)] let _ = std::process::Command::new(\"rm\");",
        "#[cfg(windows)] let _ = if true { 1 } else { std::fs::write(\"/x\", b\"\").map(|_| 1).unwrap_or(0) };",
    ] {
        let (src, listed) = door_edited("src/db.rs", "seal", line);
        let e = door_check(&src, &listed).expect_err(line);
        assert!(
            e.contains("in code the door's build compiles out"),
            "{line}: {e}"
        );
    }
    // A use that renames, globs or takes a barred function out, in code
    // compiled out, where the texts would not see the call it leads to.
    for line in [
        "#[cfg(not(debug_assertions))] let _ = (1, { use std::fs as f; f::remove_file(\"/\") });",
        "#[cfg(not(target_os = \"linux\"))] { use std::fs::*; let _ = remove_file(\"/\"); }",
        "#[cfg(windows)] { use std::fs::{remove_file}; let _ = remove_file(\"/\"); }",
        "#[cfg(windows)] { extern crate libc as c; unsafe { c::unlink(std::ptr::null()) }; }",
    ] {
        let (src, listed) = door_edited("src/db.rs", "seal", line);
        let e = door_check(&src, &listed).expect_err(line);
        assert!(
            e.contains("`a use that") && e.contains("in code the door's build compiles out"),
            "{line}: {e}"
        );
    }
    // A use that does none of that: the call it leads to is written whole.
    let (src, listed) = door_edited(
        "src/db.rs",
        "seal",
        "#[cfg(windows)] { use std::path::Path; let _ = Path::new(\"/\"); }",
    );
    let e = door_check(&src, &listed).expect_err("a new region");
    assert!(!e.contains("a use that"), "{e}");
    // A function, a module whose `cfg` is inside it, and a file brought in
    // under one.
    for tail in [
        "#[cfg(not(target_os = \"linux\"))]\nfn door_probe() { let _ = std::fs::read(\"/x\"); }\n",
        "mod door_probe {\n    #![cfg(windows)]\n    pub fn f() { let _ = std::fs::read(\"/x\"); }\n}\n",
    ] {
        let src = edited("src/db.rs", |t| format!("{t}\n{tail}"));
        let e = door_check(&src, &door_listed(&root())).expect_err(tail);
        assert!(e.contains("src/db.rs:") && e.contains("compiles out"), "{tail}: {e}");
    }
    let mut src = edited("src/lib.rs", |t| {
        format!("{t}\n#[cfg(windows)]\nmod door_probe;\n")
    });
    src.insert(
        "src/door_probe.rs".into(),
        "pub fn f() {\n    let _ = std::fs::read(\"/x\");\n}\n".into(),
    );
    let e = door_check(&src, &door_listed(&root())).expect_err("a file compiled out");
    assert!(e.contains("src/door_probe.rs:2: `fs::read`"), "{e}");
    // Nothing barred: the region is counted, and a new one grows the door.
    let (src, listed) = door_edited("src/db.rs", "seal", "#[cfg(windows)] let _ = 1;");
    let e = door_check(&src, &listed).expect_err("a new region");
    assert!(e.contains("cfgout src/db.rs Db::seal: 1 > 0 listed"), "{e}");
    // Compiled in, or test code: nothing.
    for line in [
        "#[cfg(target_os = \"linux\")] let _ = 1;",
        "#[cfg(test)] let _ = std::fs::read(\"/x\");",
        "#[cfg(all(test, windows))] let _ = std::fs::read(\"/x\");",
    ] {
        let (src, listed) = door_edited("src/db.rs", "seal", line);
        door_check(&src, &listed).unwrap_or_else(|e| panic!("{line}: {e}"));
    }
}

#[test]
fn a_cfg_only_the_doors_run_sets_fails_and_so_does_a_cfg_applied_by_cfg_attr() {
    for line in [
        "#[cfg(not(clippy))] let _ = std::fs::remove_file(\"/x\");",
        "#[cfg(clippy)] let _ = 1;",
        "#[cfg(any(windows, not(clippy)))] let _ = 1;",
        "#[cfg_attr(not(clippy), allow(unused))] let _ = 1;",
        "if !cfg!(clippy) { let _ = 1; }",
    ] {
        let (src, listed) = door_edited("src/db.rs", "seal", line);
        let e = door_check(&src, &listed).expect_err(line);
        assert!(e.contains("names `clippy`"), "{line}: {e}");
    }
    // In a door module too: its compiled-out code is its own, but code
    // under `clippy` is not the door's lint's to see either.
    let (src, listed) = door_edited(
        "src/db/vault.rs",
        "sync_dir",
        "#[cfg(not(clippy))] let _ = 1;",
    );
    assert!(door_check(&src, &listed)
        .unwrap_err()
        .contains("names `clippy`"));
    let (src, listed) = door_edited(
        "src/db.rs",
        "seal",
        "#[cfg_attr(target_os = \"linux\", cfg(windows))] let _ = 1;",
    );
    let e = door_check(&src, &listed).expect_err("cfg_attr applying a cfg");
    assert!(e.contains("a cfg_attr that applies a cfg"), "{e}");
    // A lint name in a cfg_attr's attribute is not its predicate.
    let (src, listed) = door_edited(
        "src/db.rs",
        "seal",
        "#[cfg_attr(target_os = \"linux\", allow(clippy::needless_return))] let _ = 1;",
    );
    door_check(&src, &listed).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_door_module_brings_in_no_file_and_its_threads_are_counted() {
    for tail in [
        "mod door_helper;\n",
        "pub(crate) mod door_helper ;\n",
        "include!(\"door_helper.rs\");\n",
    ] {
        let src = edited("src/db/vault.rs", |t| format!("{t}\n{tail}"));
        let e = door_check(&src, &door_listed(&root())).expect_err(tail);
        assert!(
            e.contains("src/db/vault.rs:") && e.contains("takes its allow with it"),
            "{tail}: {e}"
        );
    }
    // A module written in the file is the file's.
    let src = edited("src/db/vault.rs", |t| {
        format!("{t}\nmod door_helper {{}}\n")
    });
    door_check(&src, &door_listed(&root())).unwrap_or_else(|e| panic!("{e}"));
    let (src, listed) = door_edited("src/crypt.rs", "parse", "std::thread::spawn(|| {});");
    let e = door_check(&src, &listed).expect_err("a thread in a door module");
    assert!(e.contains("thread src/crypt.rs *: 2 > 1 listed"), "{e}");
}

// ---------------------------------------------------------------------
// The entry points of the plain-cost job (scripts/cost.sh): every `pub fn`
// of the inherent impls of `Reader`, `Db` and `Blob`, in whichever file of
// src/ the impl stands (an `impl super::Reader` in a submodule adds a method
// as public as one in src/db.rs), every fn of an impl of a trait for one of
// them (a trait's methods carry no `pub` and are as public as the trait),
// and every macro invoked where an item of such an impl stands (it makes
// items the lexer cannot see), outside test code, is either mapped to the
// probes that count it or listed with the reason no probe does, in
// tests/data/cost-entries.txt. A new `pub fn` in neither fails, so a read
// path nobody counts cannot be added without a line that says so; a line
// whose function has gone fails too, so the list stays the tree's. Every
// pub fn of `Db` is listed, not only its reads: a `Db` reads through
// `Deref<Target = Reader>`, `impl Db` holds no read today, and a read added
// there has to be caught by a rule the lexer can apply. What it cannot see:
// a missing row inside a probe that is named (a scan at a limit nobody
// runs), and whether a probe named for a function reaches it.

/// The self types whose `pub fn`s are entry points, each the name of one
/// type of the crate (`Reader` and `Db` in src/db.rs, `Blob` in src/blob.rs),
/// so an impl is read as theirs by the last name of its path in any file.
const ENTRY_TYPES: &[&str] = &["Reader", "Db", "Blob"];

/// The type aliases of src/, alias -> the type it names at the end of its
/// chain (an entry type's alias is read as the entry type below): a
/// `type R = Reader;` anywhere in the crate makes `impl R` an impl of
/// `Reader`, so an impl is read by the type its last name resolves to, an
/// alias of an alias included.
fn entry_aliases(src: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for (file, text) in src {
        if !file.starts_with("src/") || !file.ends_with(".rs") {
            continue;
        }
        let (s, _) = joined(&lex::regions(text), true);
        for (at, w) in words(&s) {
            if w != "type" {
                continue;
            }
            let k = skip_ws(&s, at + 4);
            let Some(alias) = word_at(&s, k) else {
                continue;
            };
            let Some(eq) = (k..s.len()).find(|&e| matches!(s[e], '=' | ';' | '{')) else {
                continue;
            };
            if s[eq] != '=' {
                continue;
            }
            // The target's path, to its generic arguments or its end.
            let end = (eq..s.len())
                .find(|&e| matches!(s[e], ';' | '<'))
                .unwrap_or(s.len());
            if let Some((_, last)) = words(&s[eq + 1..end]).into_iter().last() {
                named.insert(alias, last);
            }
        }
    }
    let mut out = BTreeMap::new();
    for alias in named.keys() {
        let mut t = alias.clone();
        for _ in 0..8 {
            match named.get(&t) {
                Some(next) if next != &t => t = next.clone(),
                _ => break,
            }
        }
        out.insert(alias.clone(), t);
    }
    out
}

/// Every entry point, as `Type::name`, with the file it is in. An impl of a
/// trait for one of the impl's own generic parameters -- `impl<T:
/// Deref<Target = Reader>> ReadFast for T` -- is an impl for every type the
/// bound admits, the entry types among them, and its fns are named `*::`.
fn entry_points(src: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let aliases = entry_aliases(src);
    let mut out = BTreeMap::new();
    for (file, text) in src {
        if !file.starts_with("src/") || !file.ends_with(".rs") {
            continue;
        }
        let lines = lex::regions(text);
        let (s, line_of) = joined(&lines, true);
        for (at, name, generics) in impls(&s) {
            let Some(open) = (at..s.len()).find(|&k| s[k] == '{') else {
                continue;
            };
            // A trait's methods are as public as the trait, and carry no
            // `pub` of their own: in an impl of a trait for the type, every
            // fn is an entry point, named with the trait.
            let tr = impl_trait(&s, at, open);
            // Rust has no inherent impl for a type parameter, so a self
            // type that is one of the impl's parameters is a trait's.
            let name = if generics.contains(&name) {
                "*".to_string()
            } else {
                aliases.get(&name).cloned().unwrap_or(name)
            };
            if name != "*" && !ENTRY_TYPES.contains(&name.as_str()) {
                continue;
            }
            let close = close_of(&s, open);
            let mut depth = 0i32;
            let mut k = open + 1;
            while k < close {
                match s[k] {
                    '{' | '(' | '[' => depth += 1,
                    '}' | ')' | ']' => depth -= 1,
                    _ => {}
                }
                if depth == 0 && !lines[line_of[k]].test {
                    let w = word_at(&s, k);
                    let fname = match (&tr, w.as_deref()) {
                        (None, Some("pub")) => pub_fn_name(&s, k + 3),
                        (Some(_), Some("fn")) => word_at(&s, skip_ws(&s, k + 2)),
                        _ => None,
                    };
                    if let Some(f) = fname {
                        let at = tr.as_ref().map_or(String::new(), |t| format!("{t}::"));
                        out.insert(format!("{name}::{at}{f}"), file.clone());
                    }
                    // A macro invoked where an item stands makes items no
                    // lexer can see: it is an entry point of its own name,
                    // `Type::name!`, listed with what it makes.
                    if let Some(m) = w.as_deref() {
                        let after = skip_ws(&s, k + m.chars().count());
                        if s.get(after) == Some(&'!')
                            && matches!(s.get(skip_ws(&s, after + 1)), Some('(' | '{' | '['))
                        {
                            out.insert(format!("{name}::{m}!"), file.clone());
                        }
                    }
                }
                k += 1;
            }
        }
    }
    out
}

/// The trait of an impl whose header runs from `at` to its body at `open`
/// -- `impl<G> path::Trait<A> for Type` -- by its last name before `for`,
/// or None for an inherent impl.
fn impl_trait(s: &[char], at: usize, open: usize) -> Option<String> {
    let mut d = 0i32;
    let mut last = None;
    let mut k = at + 4;
    while k < open {
        match s[k] {
            '<' => d += 1,
            '>' if s[k - 1] != '-' => d -= 1,
            _ => {}
        }
        if let Some(w) = word_at(s, k) {
            let n = w.chars().count();
            if d == 0 {
                if w == "for" {
                    return last;
                }
                if w == "where" {
                    return None;
                }
                last = Some(w);
            }
            k += n;
            continue;
        }
        k += 1;
    }
    None
}

/// The name of the fn a bare `pub` (ending before `k`) declares: `pub fn`,
/// with `const`, `unsafe`, `async` or `extern "C"` between; never
/// `pub(crate)` and the like, which no caller outside the crate reaches.
fn pub_fn_name(s: &[char], k: usize) -> Option<String> {
    let mut k = skip_ws(s, k);
    loop {
        let w = word_at(s, k)?;
        k = skip_ws(s, k + w.chars().count());
        match w.as_str() {
            "const" | "unsafe" | "async" | "extern" => {
                // An ABI string is blanked in the flat text.
                continue;
            }
            "fn" => return word_at(s, k),
            _ => return None,
        }
    }
}

/// The probes scripts/cost.sh has: its `probe` table's names, and the
/// default list a run without names counts.
fn cost_probes(root: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>), String> {
    let text = std::fs::read_to_string(root.join("scripts/cost.sh"))
        .map_err(|e| format!("scripts/cost.sh: {e}"))?;
    let mut table = BTreeSet::new();
    let mut default = BTreeSet::new();
    let mut in_probe = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("probe() {") {
            in_probe = true;
            continue;
        }
        if in_probe {
            if t == "}" {
                in_probe = false;
            } else if let Some((name, rest)) = t.split_once(") echo \"") {
                if !name.is_empty() && !rest.is_empty() && !name.starts_with('*') {
                    table.insert(name.to_string());
                }
            }
        }
        if let Some(rest) = t.strip_prefix("PROBES=${*:-") {
            let list = rest
                .strip_suffix('}')
                .ok_or("PROBES' default does not close")?;
            default.extend(list.split_whitespace().map(String::from));
        }
    }
    if table.is_empty() || default.is_empty() {
        return Err("scripts/cost.sh: no probe table or no default list read".into());
    }
    Ok((table, default))
}

/// tests/data/cost-entries.txt, as `Type::name` -> what it says: the
/// probes, or none and a reason.
fn entries_parse(text: &str) -> Result<BTreeMap<String, (Vec<String>, String)>, String> {
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let ln = n + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        const SHAPE: &str = "not `Type::fn<TAB>probes` or `Type::fn<TAB>-<TAB>reason`";
        let (name, probes, reason) = match f.as_slice() {
            [name, probes] => (*name, *probes, ""),
            [name, "-", reason] => (*name, "-", *reason),
            _ => return Err(format!("line {ln}: {SHAPE}: {line}")),
        };
        let probes: Vec<String> = if probes == "-" {
            if reason.trim().is_empty() {
                return Err(format!(
                    "line {ln}: {name} is counted by no probe and gives no reason"
                ));
            }
            Vec::new()
        } else {
            let p: Vec<String> = probes.split(',').map(|p| p.trim().to_string()).collect();
            if p.iter().any(|p| p.is_empty()) {
                return Err(format!("line {ln}: {name}: an empty probe name"));
            }
            p
        };
        if out
            .insert(name.to_string(), (probes, reason.to_string()))
            .is_some()
        {
            return Err(format!("line {ln}: {name} is listed twice"));
        }
    }
    Ok(out)
}

fn entries_check(
    src: &BTreeMap<String, String>,
    listed: &str,
    probes: &(BTreeSet<String>, BTreeSet<String>),
) -> Result<(), String> {
    let found = entry_points(src);
    let listed = entries_parse(listed)?;
    let mut errs = Vec::new();
    for (f, file) in &found {
        if !listed.contains_key(f) {
            errs.push(format!(
                "{file}: pub fn {f} is on no line of tests/data/cost-entries.txt: name the probes of scripts/cost.sh that count it, or `-` and the reason none does"
            ));
        }
    }
    for (f, (ps, _)) in &listed {
        if !found.contains_key(f) {
            errs.push(format!(
                "tests/data/cost-entries.txt: {f} is no pub fn of the tree"
            ));
        }
        for p in ps {
            if !probes.0.contains(p) {
                errs.push(format!(
                    "tests/data/cost-entries.txt: {f} names `{p}`, no probe of scripts/cost.sh"
                ));
            } else if !probes.1.contains(p) {
                errs.push(format!("tests/data/cost-entries.txt: {f} names `{p}`, which scripts/cost.sh does not run by default"));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn cost_entries(root: &Path) -> String {
    std::fs::read_to_string(root.join("tests/data/cost-entries.txt"))
        .expect("tests/data/cost-entries.txt reads")
}

#[test]
fn every_entry_point_is_counted_by_a_probe_or_says_why_not() {
    let root = root();
    let probes = cost_probes(&root).unwrap_or_else(|e| panic!("{e}"));
    if let Err(e) = entries_check(&sources(&root), &cost_entries(&root), &probes) {
        panic!("the plain-cost job's entry points:\n{e}");
    }
}

#[test]
fn a_new_entry_point_on_no_line_fails() {
    let root = root();
    let probes = cost_probes(&root).unwrap();
    let listed = cost_entries(&root);
    for (file, header, name) in [
        ("src/db.rs", "impl Reader {\n", "Reader::cost_red_entry"),
        ("src/db.rs", "impl Db {\n", "Db::cost_red_entry"),
        (
            "src/blob.rs",
            "impl<B: Bytes> Blob<B> {\n",
            "Blob::cost_red_entry",
        ),
    ] {
        for decl in [
            "pub fn cost_red_entry(&self) {}",
            "pub const fn cost_red_entry(&self) {}",
            "pub unsafe fn cost_red_entry(&self) {}",
            "#[inline]\n    pub fn cost_red_entry(&self) {}",
        ] {
            let src = edited(file, |t| {
                assert!(t.contains(header), "{file} has `{header}`");
                t.replacen(header, &format!("{header}    {decl}\n"), 1)
            });
            let e = entries_check(&src, &listed, &probes).expect_err(decl);
            assert!(
                e.contains(&format!("pub fn {name} is on no line")),
                "{decl}: {e}"
            );
        }
    }
    // An impl in another module of the crate, by any path to the type.
    for (file, imp, name) in [
        (
            "src/db/zfile.rs",
            "impl super::Reader",
            "Reader::cost_red_entry",
        ),
        (
            "src/db/vault.rs",
            "impl crate::db::Db",
            "Db::cost_red_entry",
        ),
        (
            "src/db/zpages.rs",
            "impl<B: crate::bytes::Bytes> crate::blob::Blob<B>",
            "Blob::cost_red_entry",
        ),
    ] {
        let src = edited(file, |t| {
            format!("{t}\n{imp} {{\n    pub fn cost_red_entry(&self) {{}}\n}}\n")
        });
        let e = entries_check(&src, &listed, &probes).expect_err(file);
        assert!(
            e.contains(&format!("{file}: pub fn {name} is on no line")),
            "{file}: {e}"
        );
    }
    // A method of a trait implemented for the type, which carries no `pub`
    // and is as public as the trait; and a macro invoked where an item
    // stands, whose items no lexer sees.
    for (file, add, name) in [
        (
            "src/db.rs",
            "pub trait ReadFast {\n    fn get_fast(&self, k: &[u8]) -> bool;\n}\nimpl ReadFast for Reader {\n    fn get_fast(&self, k: &[u8]) -> bool {\n        self.read_all(k, |_| ()).is_ok()\n    }\n}\n",
            "Reader::ReadFast::get_fast",
        ),
        (
            "src/blob.rs",
            "impl<B: Bytes> crate::db::ReadFast for Blob<B> {\n    fn get_fast(&self, _: &[u8]) -> bool {\n        true\n    }\n}\n",
            "Blob::ReadFast::get_fast",
        ),
    ] {
        let src = edited(file, |t| format!("{t}\n{add}"));
        let e = entries_check(&src, &listed, &probes).expect_err(name);
        assert!(e.contains(&format!("pub fn {name} is on no line")), "{name}: {e}");
    }
    // An impl that names the type by no name of its own: a trait implemented
    // for every type a bound admits, `Reader` among them, and an inherent
    // impl through an alias of the type, in the file or another, an alias of
    // an alias included.
    for (file, add, name) in [
        (
            "src/db.rs",
            "pub trait ReadFast {\n    fn get_fast(&self, k: &[u8]) -> bool;\n}\nimpl<T: std::ops::Deref<Target = Reader>> ReadFast for T {\n    fn get_fast(&self, k: &[u8]) -> bool {\n        self.read_all(k, |_| ()).is_ok()\n    }\n}\n",
            "*::ReadFast::get_fast",
        ),
        (
            "src/db.rs",
            "pub type R = Reader;\nimpl R {\n    pub fn cost_red_entry(&self) {}\n}\n",
            "Reader::cost_red_entry",
        ),
        (
            "src/db/vault.rs",
            "pub type R2 = crate::db::Db;\npub(crate) type R3 = R2;\nimpl R3 {\n    pub fn cost_red_entry(&self) {}\n}\n",
            "Db::cost_red_entry",
        ),
        (
            "src/blob.rs",
            "type Bl<B> = Blob<B>;\nimpl<B: Bytes> Bl<B> {\n    pub fn cost_red_entry(&self) {}\n}\n",
            "Blob::cost_red_entry",
        ),
    ] {
        let src = edited(file, |t| format!("{t}\n{add}"));
        let e = entries_check(&src, &listed, &probes).expect_err(name);
        assert!(e.contains(&format!("pub fn {name} is on no line")), "{name}: {e}");
    }
    // An alias of another type is not an entry type, and an impl of a trait
    // for a generic parameter of the type's own is the type's.
    for add in [
        "pub type R = Vec<Reader>;\nimpl Foo for R {\n    fn f(&self) {}\n}\n",
        "impl<B: Bytes> Wrapper<B> {\n    pub fn cost_red_entry(&self) {}\n}\n",
    ] {
        let src = edited("src/blob.rs", |t| format!("{t}\n{add}"));
        entries_check(&src, &listed, &probes).unwrap_or_else(|e| panic!("{add}: {e}"));
    }
    for invoke in [
        "gen!(cost_red_entry);",
        "gen! {cost_red_entry}",
        "gen ! [cost_red_entry];",
    ] {
        let src = edited("src/db.rs", |t| {
            let t =
                format!("macro_rules! gen {{ ($n:ident) => {{ pub fn $n(&self) {{}} }} }}\n{t}");
            t.replacen(
                "impl Reader {\n",
                &format!("impl Reader {{\n    {invoke}\n"),
                1,
            )
        });
        let e = entries_check(&src, &listed, &probes).expect_err(invoke);
        assert!(
            e.contains("pub fn Reader::gen! is on no line"),
            "{invoke}: {e}"
        );
    }
    // Not entry points: a fn of the crate's own, a fn in test code, a fn
    // nested in a method, a macro inside a method, and a pub fn of another
    // type.
    for decl in [
        "pub(crate) fn cost_red_entry(&self) {}",
        "pub(super) fn cost_red_entry(&self) {}",
        "fn cost_red_entry(&self) {}",
        "#[cfg(test)]\n    pub fn cost_red_entry(&self) {}",
        "fn outer(&self) { pub fn cost_red_entry() {} }",
        "fn outer(&self) { let _ = format!(\"{}\", 1); }",
    ] {
        let src = edited("src/db.rs", |t| {
            t.replacen(
                "impl Reader {\n",
                &format!("impl Reader {{\n    {decl}\n"),
                1,
            )
        });
        entries_check(&src, &listed, &probes).unwrap_or_else(|e| panic!("{decl}: {e}"));
    }
    let src = edited("src/db.rs", |t| {
        format!("{t}\nimpl Options {{\n    pub fn cost_red_entry(&self) {{}}\n}}\n")
    });
    entries_check(&src, &listed, &probes).unwrap_or_else(|e| panic!("another type: {e}"));
}

#[test]
fn a_line_the_tree_or_the_job_does_not_have_fails() {
    let root = root();
    let probes = cost_probes(&root).unwrap();
    let src = sources(&root);
    let listed = cost_entries(&root);
    let e = entries_check(
        &src,
        &format!("{listed}Reader::cost_red_gone\tread\n"),
        &probes,
    )
    .expect_err("a line for no fn");
    assert!(e.contains("Reader::cost_red_gone is no pub fn"), "{e}");
    let line = "Reader::read_all\t";
    let at = listed.find(line).expect("read_all is listed");
    let edit = |with: &str| {
        let end = at + listed[at..].find('\n').unwrap();
        format!("{}{line}{with}{}", &listed[..at], &listed[end..])
    };
    let e =
        entries_check(&src, &edit("read,no_such_probe"), &probes).expect_err("an unknown probe");
    assert!(e.contains("names `no_such_probe`, no probe"), "{e}");
    let e = entries_check(&src, &edit("-\t  "), &probes).expect_err("no reason");
    assert!(e.contains("gives no reason"), "{e}");
    let e = entries_check(&src, &format!("{listed}Reader::read_all\tread\n"), &probes)
        .expect_err("a second line");
    assert!(e.contains("listed twice"), "{e}");
    let e = entries_check(&src, &format!("{listed}Reader::read_all read\n"), &probes)
        .expect_err("a line without its tab");
    assert!(e.contains("not `Type::fn<TAB>probes`"), "{e}");
    // A probe in the table that a run without names skips.
    let narrowed = (
        probes.0.clone(),
        probes.1.iter().filter(|p| *p != "read").cloned().collect(),
    );
    let e = entries_check(&src, &listed, &narrowed).expect_err("a probe not run by default");
    assert!(
        e.contains("names `read`, which scripts/cost.sh does not run by default"),
        "{e}"
    );
}

#[test]
fn the_cost_job_names_its_probes_as_the_test_reads_them() {
    let (table, default) = cost_probes(&root()).unwrap();
    for p in [
        "read",
        "read_z",
        "scan",
        "scan_long",
        "scan_z",
        "seal",
        "scan_at",
        "merge",
    ] {
        assert!(
            table.contains(p) && default.contains(p),
            "{p}: {table:?} {default:?}"
        );
    }
    assert!(!table.contains("*"), "the fallthrough is no probe");
}

// CI checks through `sh scripts/check.sh` and nothing else: every check this
// project makes is one command, run the same locally and in CI, so a run
// step of a workflow that checks a change (CI_CHECKS) is a call of
// scripts/check.sh's groups or set-up on the short lists below, every group
// the script knows is run by some job or says here why not, every other
// file under .github/workflows is listed as checking no change (CI_OTHER)
// and runs on no change's event, and set-up writes to the job's environment
// only what it lists. A cargo, a python or a script of scripts/ called from
// the YAML, or shell there that decides a base or a verdict, is a second
// definition of the checks.

/// Run steps that set up and check nothing, matched whole.
const CI_SETUP: &[(&str, &str)] = &[
    (
        "sh bench/scripts/libclang.sh",
        "installs libclang for bindgen and names it to the build",
    ),
    (
        "sudo sh scripts/fscrypt.sh up /mnt/supdb-fscrypt \"$(id -u):$(id -g)\" >> \"$GITHUB_ENV\"",
        "makes the filesystem the fscrypt group runs on, and names it in the job's environment",
    ),
    (
        "./bench/target/release/bench machine | tee -a \"$GITHUB_STEP_SUMMARY\"",
        "prints the runner as a row records it, after the checks: it decides nothing",
    ),
];

/// A package install: this prefix, then package names alone.
const CI_APT: &str = "sudo apt-get update -qq && sudo apt-get install -y -qq ";

/// Actions a step may use, matched whole: each fetches, installs or
/// publishes, and none checks.
const CI_ACTIONS: &[(&str, &str)] = &[
    ("actions/checkout@v4", "the tree"),
    ("dtolnay/rust-toolchain@stable", "the toolchain"),
    ("Swatinem/rust-cache@v2", "the build cache"),
    ("actions/upload-artifact@v4", "publishes the quick row"),
];

/// Inputs an action may be given, matched by action and name: none
/// chooses what is checked.
const CI_WITH: &[(&str, &str, &str)] = &[
    (
        "actions/checkout@v4",
        "fetch-depth",
        "the whole history, which the base needs",
    ),
    (
        "dtolnay/rust-toolchain@stable",
        "components",
        "rustfmt and clippy, which the lint group runs",
    ),
    (
        "dtolnay/rust-toolchain@stable",
        "targets",
        "the targets the wasm and cross-arm groups build for",
    ),
    (
        "Swatinem/rust-cache@v2",
        "workspaces",
        "which target directories to cache",
    ),
    ("actions/upload-artifact@v4", "name", "the artifact's name"),
    (
        "actions/upload-artifact@v4",
        "if-no-files-found",
        "a run that wrote no row publishes nothing",
    ),
    ("actions/upload-artifact@v4", "path", "what is published"),
];

/// The environment a workflow, a job or a step may set, matched whole,
/// name and value: the event's values and a person's input, and nothing
/// that picks a base, a step or a verdict, which are scripts/check.sh's
/// and scripts/base.sh's to decide.
const CI_ENV: &[(&str, &str, &str)] = &[
    (
        "CARGO_TERM_COLOR",
        "always",
        "colours cargo's output in the log",
    ),
    (
        "ARMS",
        "${{ inputs.arms }}",
        "the quick group's arms, as the person who dispatched the run named them",
    ),
    (
        "KEYS",
        "${{ inputs.keys }}",
        "the quick group's keys, numbered or ids, as the person who dispatched the run named them",
    ),
    (
        "SUPDB_PR_BASE",
        "${{ github.event.pull_request.base.sha }}",
        "the event's value scripts/base.sh decides the base from",
    ),
    (
        "SUPDB_PUSH_BEFORE",
        "${{ github.event.before }}",
        "the event's value scripts/base.sh decides the base from",
    ),
];

/// The conditions a step may carry, matched whole with the action it is
/// on: a step that publishes may run whether the checks before it passed.
/// A condition on a check, or on a job, decides in the YAML whether a
/// check runs.
const CI_IF: &[(&str, &str, &str)] = &[(
    "always()",
    "actions/upload-artifact@v4",
    "the quick row is published whether or not the run passed: it decides nothing",
)];

/// The workflow's triggers, by path under `on:` and value, matched whole
/// both ways (a person's inputs to `workflow_dispatch` are read by the
/// keys above): a trigger dropped or narrowed is a change on which no job
/// runs, and so a group nobody runs there.
const CI_ON: &[(&str, &str, &str)] = &[
    ("push", "", "a push runs every job"),
    ("push.branches", "[main]", "the main line, every push to it"),
    ("push.tags", "['*']", "a tag, every one"),
    (
        "pull_request",
        "",
        "every pull request, whatever its branch",
    ),
    (
        "workflow_dispatch",
        "",
        "a run by hand, which the quick job's arms are for",
    ),
];

/// Groups of scripts/check.sh no CI job runs, each with why.
const CI_NOT_RUN: &[(&str, &str)] = &[];

/// The groups scripts/check.sh's loop knows, by its case's labels (`a)` or
/// `a | b)`, at the first label's indent, a case inside a group read past
/// to its own `esac`), or why the test cannot read one: a line that looks
/// like a label at another indent, or a `case` or `esac` in another shape
/// than a line that begins `case ... in` or one whose last word is `esac`.
fn check_groups(check_sh: &str) -> Result<BTreeSet<String>, String> {
    let lines: Vec<&str> = check_sh.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == "case \"$g\" in")
        .ok_or("scripts/check.sh has no `case \"$g\" in`")?;
    // The depth of cases at each line after the start, ending where the
    // groups' own case is closed.
    let mut depth = 1;
    let mut end = None;
    let mut at_depth = Vec::new();
    for (i, l) in lines.iter().enumerate().skip(start + 1) {
        let words: Vec<&str> = l
            .split_whitespace()
            .take_while(|w| !w.starts_with('#'))
            .collect();
        let opens = words.first() == Some(&"case");
        let closes = words.last() == Some(&"esac");
        if words.iter().any(|w| *w == "case" || *w == "esac")
            && !(opens && (closes || words.last() == Some(&"in")))
            && !(closes
                && words
                    .iter()
                    .filter(|w| **w == "case" || **w == "esac")
                    .count()
                    == 1)
        {
            return Err(format!(
                "scripts/check.sh line {}: a `case` or `esac` the test cannot read: {l}",
                i + 1
            ));
        }
        at_depth.push(depth);
        match (opens, closes) {
            (true, false) => depth += 1,
            (false, true) => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            end = Some(i);
            break;
        }
    }
    let end = end.ok_or("scripts/check.sh's case has no `esac`")?;
    let label = |t: &str| -> Option<Vec<String>> {
        let g = t.strip_suffix(')')?;
        let names: Vec<String> = g.split('|').map(|n| n.trim().to_string()).collect();
        names
            .iter()
            .all(|n| {
                n == "*"
                    || (!n.is_empty()
                        && n.chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
            })
            .then_some(names)
    };
    let mut at = None;
    let mut out = BTreeSet::new();
    for (i, l) in lines[start + 1..end].iter().enumerate() {
        if at_depth[i] != 1 {
            continue;
        }
        let t = l.trim_start();
        let Some(names) = label(t.trim_end()) else {
            continue;
        };
        let ind = l.len() - t.len();
        if *at.get_or_insert(ind) != ind {
            return Err(format!(
                "scripts/check.sh line {}: a label of scripts/check.sh the test cannot read, at another indent: {l}",
                start + 2 + i
            ));
        }
        out.extend(names.into_iter().filter(|n| n != "*"));
    }
    Ok(out)
}

/// A step of a workflow: its job, where it starts, its run commands, its
/// action, its inputs and its condition, each with its line.
#[derive(Default)]
struct CiStep {
    job: String,
    line: usize,
    runs: Vec<(usize, String)>,
    uses: Option<(usize, String)>,
    with: Vec<(usize, String)>,
    cond: Option<(usize, String)>,
}

/// A workflow read: its steps, the jobs that carry a condition, and every
/// key the test does not let stand.
type CiRead = (Vec<CiStep>, BTreeSet<String>, Vec<String>);

/// Every key of a workflow, by the whole path to it (a job named `inputs`
/// is a job). A key the test does not know, an anchor, an alias, a flow
/// mapping, a quoted run, a `shell:` or a `continue-on-error:` is an error;
/// so is an `env:` entry not on CI_ENV, and a trigger other than `on`'s
/// (CI_ON for ci.yml) or one of `on`'s missing.
/// A run's block is one command a line, a line ending in `\` going on in
/// the next.
fn ci_steps(yaml: &str, on: &[(&str, &str, &str)]) -> CiRead {
    const TOP: &[&str] = &["name", "on", "permissions", "env", "jobs"];
    const ON: &[&str] = &["push", "pull_request", "workflow_dispatch"];
    const JOB: &[&str] = &["strategy", "runs-on", "steps", "timeout-minutes", "env"];
    const STEP: &[&str] = &["name", "uses", "run", "with", "env"];
    const FLOW: &[&str] = &["branches", "tags", "runner"];
    let lines: Vec<&str> = yaml.lines().collect();
    let (mut steps, mut cond_jobs, mut errs) = (Vec::<CiStep>::new(), BTreeSet::new(), Vec::new());
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut triggers: Vec<(usize, String, String)> = Vec::new();
    let mut job = String::new();
    // Whether the last step pushed is one of the steps block being read.
    let mut open = false;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let ln = i + 1;
        i += 1;
        let t = l.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let cannot = |why: &str| format!("line {ln}: a step the test cannot read ({why}): {l}");
        let (item, rest) = match t.strip_prefix("- ") {
            Some(r) => (true, r.trim_start()),
            None => (false, t),
        };
        let kind = l.len() - rest.len();
        while stack.last().is_some_and(|(n, _)| *n >= kind) {
            stack.pop();
        }
        // The paths a pull request's trigger is narrowed to: an item each,
        // held to `on` as a trigger's value.
        if item
            && stack
                .iter()
                .map(|(_, k)| k.as_str())
                .eq(["on", "pull_request", "paths"])
        {
            triggers.push((ln, "pull_request.paths.-".to_string(), rest.to_string()));
            continue;
        }
        let Some((key, v)) = rest.split_once(':').filter(|(k, v)| {
            !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
                && (v.is_empty() || v.starts_with(' '))
        }) else {
            errs.push(cannot("no key the test reads"));
            continue;
        };
        let v = v.trim();
        let path: Vec<&str> = stack.iter().map(|(_, k)| k.as_str()).collect();
        // A key is read by the whole path to it, never by its parent's
        // name alone: a job may be named `inputs`, `with` or `env`.
        let in_steps = matches!(path.as_slice(), ["jobs", _, "steps"]);
        if item {
            if !in_steps {
                errs.push(cannot("a list outside a job's steps"));
                continue;
            }
            steps.push(CiStep {
                job: job.clone(),
                line: ln,
                ..CiStep::default()
            });
            open = true;
        }
        if key == "steps" || path == ["jobs"] {
            open = false;
        }
        if v.starts_with(['&', '*']) {
            errs.push(cannot("an anchor or an alias"));
        }
        if v.starts_with(['[', '{']) && !FLOW.contains(&key) {
            errs.push(cannot("a flow value"));
        }
        let block = !v.is_empty()
            && v.starts_with(['|', '>'])
            && v[1..].chars().all(|c| "+-0123456789".contains(c));
        let mut body = Vec::new();
        if block {
            while i < lines.len() {
                let b = lines[i];
                let bi = b.len() - b.trim_start().len();
                if !b.trim().is_empty() && bi <= kind {
                    break;
                }
                if !b.trim().is_empty() {
                    body.push((i + 1, b.trim().to_string()));
                }
                i += 1;
            }
        }
        if path.first() == Some(&"on") && path.get(1) != Some(&"workflow_dispatch") {
            let at: Vec<&str> = path[1..].iter().copied().chain([key]).collect();
            triggers.push((ln, at.join("."), v.to_string()));
        }
        let step = steps.last_mut().filter(|_| open && in_steps);
        let known = match path.as_slice() {
            [] => TOP.contains(&key),
            ["on"] => ON.contains(&key),
            ["on", "push"] => ["branches", "tags"].contains(&key),
            ["on", "pull_request"] => key == "paths",
            // A token that reads the tree and writes nothing.
            ["permissions"] => key == "contents" && v == "read",
            ["on", "workflow_dispatch"] => key == "inputs",
            ["on", "workflow_dispatch", "inputs"] => true,
            ["on", "workflow_dispatch", "inputs", _] => {
                ["description", "required", "default"].contains(&key)
            }
            ["jobs"] => {
                job = key.to_string();
                true
            }
            ["jobs", _] => {
                if key == "if" {
                    cond_jobs.insert(job.clone());
                    errs.push(format!(
                        "line {ln}: a job's `if:` decides in the YAML whether its checks run: {l}"
                    ));
                    true
                } else {
                    JOB.contains(&key)
                }
            }
            ["jobs", _, "strategy"] => ["fail-fast", "matrix"].contains(&key),
            ["jobs", _, "strategy", "matrix"] => key == "runner",
            ["env"] | ["jobs", _, "env"] | ["jobs", _, "steps", "env"] => {
                if !CI_ENV.iter().any(|(n, x, _)| *n == key && *x == v) {
                    errs.push(format!(
                        "line {ln}: the environment `{key}: {v}` is on no list of the test: CI passes the event's values, and the checks decide the rest"
                    ));
                }
                true
            }
            ["jobs", _, "steps", "with"] => true,
            ["jobs", _, "steps"] => {
                let Some(st) = step else {
                    errs.push(cannot("a key before any step"));
                    continue;
                };
                match key {
                    "if" => {
                        st.cond = Some((ln, v.to_string()));
                        true
                    }
                    "uses" => {
                        st.uses = Some((ln, v.to_string()));
                        true
                    }
                    "run" => {
                        match v {
                            "" => errs.push(format!("line {ln}: a run with no command: {l}")),
                            _ if block && v.starts_with('>') => {
                                let all: Vec<String> =
                                    body.iter().map(|(_, c)| c.clone()).collect();
                                st.runs.push((ln, all.join(" ")));
                            }
                            _ if block => {
                                let mut cur: Option<(usize, String)> = None;
                                for (bl, c) in body.drain(..) {
                                    let (at, mut s) = cur.take().unwrap_or((bl, String::new()));
                                    if !s.is_empty() {
                                        s.push(' ');
                                    }
                                    match c.strip_suffix('\\') {
                                        Some(head) => {
                                            s.push_str(head.trim_end());
                                            cur = Some((at, s));
                                        }
                                        None => {
                                            s.push_str(&c);
                                            st.runs.push((at, s));
                                        }
                                    }
                                }
                                if let Some(c) = cur {
                                    st.runs.push(c);
                                }
                            }
                            _ if v.starts_with(['"', '\'']) => errs.push(format!(
                                "line {ln}: a quoted run the test does not read: {l}"
                            )),
                            _ => st.runs.push((ln, v.to_string())),
                        }
                        true
                    }
                    _ => STEP.contains(&key),
                }
            }
            _ => false,
        };
        if matches!(path.as_slice(), ["jobs", _, "steps", "with"]) {
            if let Some(st) = steps.last_mut() {
                st.with.push((ln, key.to_string()));
            }
        }
        if key == "continue-on-error" {
            errs.push(format!(
                "line {ln}: continue-on-error decides in the YAML that a red check reads green: {l}"
            ));
        } else if key == "shell" {
            errs.push(format!("line {ln}: a shell of the step's own: {l}"));
        } else if !known {
            errs.push(format!(
                "line {ln}: the key `{key}` is on no list of the test: {l}"
            ));
        }
        stack.push((kind, key.to_string()));
    }
    let got: BTreeSet<(String, String)> = triggers
        .iter()
        .map(|(_, k, v)| (k.clone(), v.clone()))
        .collect();
    for (ln, k, v) in &triggers {
        if !on.iter().any(|(x, y, _)| x == k && y == v) {
            errs.push(format!(
                "line {ln}: the trigger `{k}: {v}` is on no list of the test"
            ));
        }
    }
    for (k, v, why) in on {
        if !got.contains(&(k.to_string(), v.to_string())) {
            errs.push(format!(
                "the trigger `{k}` the test lists ({why}) is not in the workflow as `{k}: {v}`"
            ));
        }
    }
    (steps, cond_jobs, errs)
}

/// The verdict over ci.yml's text and check.sh's, the other workflows as
/// they stand.
fn ci_check(yaml: &str, check_sh: &str) -> Result<(), String> {
    ci_check_with(yaml, check_sh, CI_NOT_RUN)
}

/// The same, with the groups no job runs given.
fn ci_check_with(yaml: &str, check_sh: &str, not_run: &[(&str, &str)]) -> Result<(), String> {
    let mut files = ci_workflows();
    for (n, t) in files.iter_mut() {
        if n == CI_YML {
            *t = yaml.to_string();
        }
    }
    ci_check_files(&files, check_sh, not_run)
}

/// The workflow that checks every change.
const CI_YML: &str = ".github/workflows/ci.yml";

/// The triggers of apple-silicon.yml, as CI_ON is ci.yml's: its group reads
/// the machine capture, so it runs where the capture's code or the
/// workflow changes, and by hand.
const APPLE_ON: &[(&str, &str, &str)] = &[
    (
        "pull_request",
        "",
        "a pull request that changes what it reads",
    ),
    ("pull_request.paths", "", "the files the capture is"),
    (
        "pull_request.paths.-",
        "'bench/src/machine.rs'",
        "the capture",
    ),
    (
        "pull_request.paths.-",
        "'bench/src/env.rs'",
        "what the capture reads the host through",
    ),
    (
        "pull_request.paths.-",
        "'.github/workflows/apple-silicon.yml'",
        "the workflow itself",
    ),
    (
        "pull_request.paths.-",
        "'scripts/check.sh'",
        "the group the workflow runs",
    ),
    ("workflow_dispatch", "", "a run by hand"),
];

/// A workflow's triggers: the path under `on:`, the value, and why.
type CiOn = &'static [(&'static str, &'static str, &'static str)];

/// The workflows that check a change, each held as ci.yml is, with its
/// triggers: every run step a group's call or listed set-up, and the
/// groups any of them runs counted together.
const CI_CHECKS: &[(&str, CiOn)] = &[
    (CI_YML, CI_ON),
    (".github/workflows/apple-silicon.yml", APPLE_ON),
];

/// The workflows that check no change, each with why: a measurement or a
/// proposal, run by hand, on a schedule or after a merge to main. Their
/// triggers are read and held to that (workflow_dispatch, schedule, and a
/// push to main alone); one that runs on a change's event is a check, and
/// belongs on CI_CHECKS.
const CI_OTHER: &[(&str, &str)] = &[
    (
        ".github/workflows/bank-row.yml",
        "after a merge to main, proposes the hosted runners' quick rows as a pull request, which ci.yml then checks",
    ),
    (
        ".github/workflows/comparators.yml",
        "a measurement of the comparators against their own tools, by hand and weekly: a finding about an arm, not about a change",
    ),
    (
        ".github/workflows/ooc-hosted.yml",
        "a bounded measurement, by hand: its row is an artifact for a person to bank",
    ),
    (
        ".github/workflows/quiet-bench.yml",
        "a full-scale measurement on a self-hosted Mac, by hand",
    ),
    (
        ".github/workflows/rocksdb-alloc.yml",
        "a table of RocksDB's two allocators for a person to read, by hand and weekly",
    ),
    (
        ".github/workflows/runner-smoke.yml",
        "prints which runner claimed a job, by hand",
    ),
];

/// The triggers a workflow on CI_OTHER may have: none is a change's event.
fn other_triggers(name: &str, yaml: &str) -> Vec<String> {
    let mut errs = Vec::new();
    let lines: Vec<&str> = yaml.lines().collect();
    let tops: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            t.len() == l.len() && !t.is_empty() && !t.starts_with('#')
        })
        .map(|(i, _)| i)
        .collect();
    let ons: Vec<usize> = tops
        .iter()
        .copied()
        .filter(|&i| {
            let k = lines[i].split(':').next().unwrap_or("").trim();
            k == "on"
        })
        .collect();
    let [at] = ons.as_slice() else {
        errs.push(format!("{name}: no one `on:` the test can read"));
        return errs;
    };
    let end = tops
        .iter()
        .copied()
        .find(|&i| i > *at)
        .unwrap_or(lines.len());
    // Each trigger by its indent under `on:`, and push's branches.
    let mut ind = None;
    let mut trig = String::new();
    for (i, l) in lines.iter().enumerate().take(end).skip(at + 1) {
        let t = l.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let d = l.len() - t.len();
        let first = *ind.get_or_insert(d);
        let key = t.split(':').next().unwrap_or("").trim();
        if d == first {
            trig = key.to_string();
            if !["workflow_dispatch", "schedule", "push"].contains(&key) || t.starts_with('-') {
                errs.push(format!(
                    "{name} line {}: the trigger `{t}` is a change's event, or one the test does not know: a workflow that checks a change is on CI_CHECKS",
                    i + 1
                ));
            } else if t != format!("{key}:") {
                errs.push(format!(
                    "{name} line {}: a trigger the test does not read as a block: {t}",
                    i + 1
                ));
            }
        } else if d < first {
            errs.push(format!(
                "{name} line {}: a line the test cannot read: {l}",
                i + 1
            ));
        } else if trig == "push" && t != "branches: [main]" {
            errs.push(format!(
                "{name} line {}: a push to anything but main is a change's event: {t}",
                i + 1
            ));
        }
    }
    if ind.is_none() {
        errs.push(format!("{name}: an `on:` with no trigger"));
    }
    if lines[*at + 1..end]
        .iter()
        .any(|l| l.trim_start().starts_with("push:"))
        && !lines[*at + 1..end]
            .iter()
            .any(|l| l.trim() == "branches: [main]")
    {
        errs.push(format!(
            "{name}: a push to every branch is a change's event"
        ));
    }
    errs
}

/// The verdict over every workflow file (name, text) and check.sh's text.
fn ci_check_files(
    files: &[(String, String)],
    check_sh: &str,
    not_run: &[(&str, &str)],
) -> Result<(), String> {
    let known = check_groups(check_sh)?;
    let mut ran = BTreeSet::new();
    let mut errs = Vec::new();
    for (name, yaml) in files {
        if let Some((_, on)) = CI_CHECKS.iter().find(|(n, _)| n == name) {
            for e in ci_file(yaml, on, &known, &mut ran) {
                errs.push(format!("{name}: {e}"));
            }
        } else if CI_OTHER.iter().any(|(n, _)| n == name) {
            errs.extend(other_triggers(name, yaml));
        } else {
            errs.push(format!(
                "{name}: a workflow the test does not list: one that checks a change is held as ci.yml is (CI_CHECKS), one that does not says why (CI_OTHER)"
            ));
        }
    }
    for n in CI_CHECKS
        .iter()
        .map(|(n, _)| *n)
        .chain(CI_OTHER.iter().map(|(n, _)| *n))
    {
        if !files.iter().any(|(f, _)| f == n) {
            errs.push(format!("{n}: listed by the test and not in the tree"));
        }
    }
    for g in &known {
        let said = not_run.iter().any(|(n, _)| n == g);
        if !ran.contains(g) && !said {
            errs.push(format!(
                "the group `{g}` of scripts/check.sh runs in no CI job, and CI_NOT_RUN says not why"
            ));
        }
        if ran.contains(g) && said {
            errs.push(format!(
                "the group `{g}` runs in CI and CI_NOT_RUN says it does not"
            ));
        }
    }
    for (n, why) in not_run {
        if why.trim().is_empty() {
            errs.push(format!("CI_NOT_RUN gives `{n}` no reason"));
        }
        if !known.contains(*n) {
            errs.push(format!(
                "CI_NOT_RUN names `{n}`, no group of scripts/check.sh"
            ));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

fn ci_texts() -> (String, String) {
    let r = root();
    (
        std::fs::read_to_string(r.join(".github/workflows/ci.yml")).expect("ci.yml reads"),
        std::fs::read_to_string(r.join("scripts/check.sh")).expect("check.sh reads"),
    )
}

/// Every file under .github/workflows, by its path from the root.
fn ci_workflows() -> Vec<(String, String)> {
    ci_workflows_in(&root())
}

/// Every file under `r`'s .github/workflows, whatever its name, in every
/// directory under it.
fn ci_workflows_in(r: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut dirs = vec![r.join(".github/workflows")];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).expect("the workflows read") {
            let p = e.expect("an entry").path();
            if p.is_dir() {
                dirs.push(p);
                continue;
            }
            let n = p
                .strip_prefix(r)
                .expect("under the root")
                .to_string_lossy()
                .into_owned();
            let t = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{n}: {e}"));
            out.push((n, t));
        }
    }
    out.sort();
    out
}

/// One checking workflow's steps held, the groups its unconditioned runs
/// call added to `ran`.
fn ci_file(
    yaml: &str,
    on: &[(&str, &str, &str)],
    known: &BTreeSet<String>,
    ran: &mut BTreeSet<String>,
) -> Vec<String> {
    let (steps, cond_jobs, mut errs) = ci_steps(yaml, on);
    for st in &steps {
        let action = st.uses.as_ref().map(|(_, a)| a.as_str()).unwrap_or("");
        if st.uses.is_some() && !st.runs.is_empty() {
            errs.push(format!(
                "line {}: a step that is both a run and an action",
                st.line
            ));
        }
        if let Some((ln, c)) = &st.cond {
            if !CI_IF.iter().any(|(x, a, _)| x == c && *a == action) {
                errs.push(format!(
                    "line {ln}: an `if:` the test does not list (`{c}` on `{}`): a condition in the YAML decides whether a check runs",
                    if action.is_empty() { "a run" } else { action }
                ));
            }
        }
        for (ln, k) in &st.with {
            if !CI_WITH.iter().any(|(a, x, _)| *a == action && x == k) {
                errs.push(format!(
                    "line {ln}: the input `{k}` of `{action}` is on no list of the test"
                ));
            }
        }
        // A run under a condition is a run the YAML may skip, so it runs
        // no group.
        let counts = st.cond.is_none() && !cond_jobs.contains(&st.job);
        for (ln, cmd) in &st.runs {
            if let Some(rest) = cmd.strip_prefix("sh scripts/check.sh ") {
                for g in rest.split_whitespace() {
                    if known.contains(g) {
                        if counts {
                            ran.insert(g.to_string());
                        }
                    } else {
                        errs.push(format!(
                            "line {ln}: `{g}` in `{cmd}` is no group of scripts/check.sh"
                        ));
                    }
                }
                continue;
            }
            if CI_SETUP.iter().any(|(s, _)| s == cmd) {
                continue;
            }
            if let Some(pkgs) = cmd.strip_prefix(CI_APT) {
                let name = |w: &str| {
                    w.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                        && w.chars().all(|c| {
                            c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c)
                        })
                };
                if pkgs.split_whitespace().count() > 0 && pkgs.split_whitespace().all(name) {
                    continue;
                }
            }
            errs.push(format!(
                "line {ln}: `{cmd}` is no call of scripts/check.sh and no set-up the test lists: a check CI makes is a group of scripts/check.sh"
            ));
        }
        if let Some((ln, a)) = &st.uses {
            if !CI_ACTIONS.iter().any(|(x, _)| x == a) {
                errs.push(format!(
                    "line {ln}: the action `{a}` is on no list of the test"
                ));
            }
        }
    }
    errs
}

#[test]
fn ci_checks_only_through_check_sh_and_runs_every_group() {
    let (yaml, sh) = ci_texts();
    if let Err(e) = ci_check(&yaml, &sh) {
        panic!(".github/workflows:\n{e}");
    }
    let files = ci_workflows();
    assert!(files.iter().any(|(n, _)| n == CI_YML), "{files:?}");
    let r = root();
    for (n, _) in CI_SETUP {
        if let Some(script) = ci_setup_script(n) {
            let text =
                std::fs::read_to_string(r.join(script)).unwrap_or_else(|e| panic!("{script}: {e}"));
            let errs = ci_setup_writes(script, &text, n.contains(">> \"$GITHUB_ENV\""));
            assert!(errs.is_empty(), "{}", errs.join("\n"));
        }
    }
}

/// What a set-up step may put in the job's environment, by script and
/// line, matched whole: a line that names one of the runner's files
/// (CI_RUNNER_FILES), and in a script whose output the step sends to
/// $GITHUB_ENV, a line that prints to it. A check that set-up could narrow
/// through the environment (`RATCHET_RED_STEPS`, `COST_PROBES`, a base) is a
/// check decided outside scripts/check.sh.
const CI_SETUP_WRITES: &[(&str, &str, &str)] = &[
    (
        "bench/scripts/libclang.sh",
        "if [ -n \"${GITHUB_ENV:-}\" ]; then",
        "asks whether it runs in a workflow",
    ),
    (
        "bench/scripts/libclang.sh",
        "echo \"$1=$2\" >> \"$GITHUB_ENV\"",
        "`emit`'s write, of the names CI_SETUP_VARS lists",
    ),
    (
        "scripts/fscrypt.sh",
        "echo \"SUPDB_FSCRYPT_DIR=$dir\"",
        "the filesystem the fscrypt group runs on",
    ),
    (
        "scripts/fscrypt.sh",
        "echo \"SUPDB_FSCRYPT_IMAGE=$img\"",
        "the file that filesystem is in",
    ),
];

/// The names libclang.sh's `emit` may write, by script and name.
const CI_SETUP_VARS: &[(&str, &str, &str)] = &[
    (
        "bench/scripts/libclang.sh",
        "LIBCLANG_PATH",
        "where bindgen finds libclang",
    ),
    (
        "bench/scripts/libclang.sh",
        "RUSTFLAGS",
        "macOS's rpath to libclang for the build scripts, appended",
    ),
];

/// The runner's files a step writes the job's state through.
const CI_RUNNER_FILES: &[&str] = &[
    "GITHUB_ENV",
    "GITHUB_PATH",
    "GITHUB_OUTPUT",
    "GITHUB_STATE",
    "BASH_ENV",
];

/// The script a set-up line runs, where it is one (`sh X ...`, `sudo sh X
/// ...`).
fn ci_setup_script(cmd: &str) -> Option<&str> {
    let w: Vec<&str> = cmd.split_whitespace().collect();
    let w = if w.first() == Some(&"sudo") {
        &w[1..]
    } else {
        &w[..]
    };
    (w.first() == Some(&"sh"))
        .then(|| w.get(1).copied())
        .flatten()
}

/// The lines of a set-up script that write the job's environment and are
/// on no list, each an error.
fn ci_setup_writes(script: &str, text: &str, to_env: bool) -> Vec<String> {
    let mut errs = Vec::new();
    for (i, l) in text.lines().enumerate() {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let listed = CI_SETUP_WRITES
            .iter()
            .any(|(s, x, _)| *s == script && *x == t);
        let words: Vec<&str> = t.split_whitespace().collect();
        if words.first() == Some(&"emit") {
            let name = words.get(1).copied().unwrap_or("");
            if !CI_SETUP_VARS
                .iter()
                .any(|(s, n, _)| *s == script && *n == name)
            {
                errs.push(format!(
                    "{script} line {}: `emit` of a name CI_SETUP_VARS does not list: {t}",
                    i + 1
                ));
            }
            continue;
        }
        let names_file = CI_RUNNER_FILES.iter().any(|f| t.contains(f));
        let prints = to_env
            && words
                .iter()
                .any(|w| ["echo", "printf"].contains(&w.trim_start_matches(['{', '(', ';'])))
            && !t.contains(">&2");
        if (names_file || prints) && !listed {
            errs.push(format!(
                "{script} line {}: a write to the job's environment CI_SETUP_WRITES does not list: {t}",
                i + 1
            ));
        }
    }
    errs
}

#[test]
fn a_set_up_step_that_writes_the_environment_unlisted_fails() {
    let r = root();
    let lib = std::fs::read_to_string(r.join("bench/scripts/libclang.sh")).expect("reads");
    let fs = std::fs::read_to_string(r.join("scripts/fscrypt.sh")).expect("reads");
    assert!(ci_setup_writes("bench/scripts/libclang.sh", &lib, false).is_empty());
    assert!(ci_setup_writes("scripts/fscrypt.sh", &fs, true).is_empty());
    let at = "    echo \"$1=$2\" >> \"$GITHUB_ENV\"\n";
    for to in [
        "    echo RATCHET_RED_STEPS=G >> \"$GITHUB_ENV\"\n",
        "    echo \"$PWD/fake\" >> \"$GITHUB_PATH\"\n",
        "    printf 'COST_PROBES=seal\\n' >>\"${GITHUB_ENV}\"\n",
    ] {
        let e = ci_setup_writes(
            "bench/scripts/libclang.sh",
            &ci_edit(&lib, at, &format!("{at}{to}")),
            false,
        );
        assert!(
            e.len() == 1 && e[0].contains("CI_SETUP_WRITES does not list"),
            "{to}: {e:?}"
        );
    }
    let e = ci_setup_writes(
        "bench/scripts/libclang.sh",
        &ci_edit(&lib, at, &format!("{at}emit SUPDB_BASE HEAD\n")),
        false,
    );
    assert!(e.len() == 1 && e[0].contains("CI_SETUP_VARS"), "{e:?}");
    // A script whose output goes to $GITHUB_ENV: a line printed to it is
    // a write; one to stderr is not. The same line is no write where the
    // step keeps the output.
    let at = "    echo \"SUPDB_FSCRYPT_IMAGE=$img\"\n";
    let more = ci_edit(&fs, at, &format!("{at}    echo \"RATCHET_RED_STEPS=G\"\n"));
    let e = ci_setup_writes("scripts/fscrypt.sh", &more, true);
    assert!(e.len() == 1, "{e:?}");
    assert!(ci_setup_writes("scripts/fscrypt.sh", &more, false).is_empty());
    let more = ci_edit(&fs, at, &format!("{at}    echo \"a note\" >&2\n"));
    assert!(ci_setup_writes("scripts/fscrypt.sh", &more, true).is_empty());
    assert_eq!(
        ci_setup_script("sudo sh scripts/fscrypt.sh up x"),
        Some("scripts/fscrypt.sh")
    );
    assert_eq!(
        ci_setup_script("./bench/target/release/bench machine"),
        None
    );
}

#[test]
fn every_file_under_the_workflows_is_read() {
    let r = PathBuf::from(std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into()))
        .join(format!("supdb-next-workflows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&r);
    let w = r.join(".github/workflows");
    std::fs::create_dir_all(w.join("nested/deeper")).expect("dirs");
    for (f, t) in [
        ("a.yml", "a"),
        ("b.yaml", "b"),
        ("notes", "c"),
        ("nested/deeper/d.yml", "d"),
    ] {
        std::fs::write(w.join(f), t).expect("write");
    }
    let got = ci_workflows_in(&r);
    let _ = std::fs::remove_dir_all(&r);
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            ".github/workflows/a.yml",
            ".github/workflows/b.yaml",
            ".github/workflows/nested/deeper/d.yml",
            ".github/workflows/notes",
        ],
        "{got:?}"
    );
    assert_eq!(got[2].1, "d");
}

#[test]
fn a_workflow_the_test_does_not_hold_fails() {
    let (yaml, sh) = ci_texts();
    let files = ci_workflows();
    ci_check_files(&files, &sh, CI_NOT_RUN).expect("the workflows as they stand are green");
    let with = |name: &str, text: &str| -> Vec<(String, String)> {
        let mut f: Vec<(String, String)> =
            files.iter().filter(|(n, _)| n != name).cloned().collect();
        f.push((name.to_string(), text.to_string()));
        f
    };
    let text = |name: &str| -> String {
        files
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{name}"))
            .1
            .clone()
    };
    // A new workflow, a check of every pull request outside check.sh, and
    // one that checks nothing: both unlisted.
    let extra = "name: extra\non:\n  pull_request:\njobs:\n  t:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@v4\n      - run: cargo test\n";
    for (name, t) in [
        (".github/workflows/extra.yml", extra),
        (".github/workflows/nested/extra.yml", extra),
        (".github/workflows/notes.txt", "a note\n"),
    ] {
        let e = ci_check_files(&with(name, t), &sh, CI_NOT_RUN).expect_err(name);
        assert!(
            e.contains(&format!("{name}: a workflow the test does not list")),
            "{name}: {e}"
        );
    }
    // apple-silicon.yml is held as ci.yml is: a check of its own in place
    // of the group's call, a working directory, a trigger narrowed or
    // widened.
    let apple = ".github/workflows/apple-silicon.yml";
    let at = text(apple);
    for (from, to, want) in [
        (
            "run: sh scripts/check.sh machine",
            "run: cargo test --profile checked",
            "is no call of scripts/check.sh",
        ),
        (
            "run: sh scripts/check.sh machine",
            "run: sh scripts/check.sh machine\n        working-directory: bench",
            "the key `working-directory`",
        ),
        (
            "      - 'bench/src/env.rs'\n",
            "",
            "the trigger `pull_request.paths.-` the test lists",
        ),
        (
            "      - 'bench/src/env.rs'\n",
            "      - 'bench/src/env.rs'\n      - 'src/**'\n",
            "the trigger `pull_request.paths.-: 'src/**'`",
        ),
        (
            "  workflow_dispatch:\n",
            "  workflow_dispatch:\n  push:\n",
            "the trigger `push: ` is on no list",
        ),
        ("contents: read", "contents: write", "the key `contents`"),
    ] {
        let e =
            ci_check_files(&with(apple, &ci_edit(&at, from, to)), &sh, CI_NOT_RUN).expect_err(to);
        assert!(
            e.contains(&format!("{apple}: ")) && e.contains(want),
            "{to}: {e}"
        );
    }
    let e = ci_check_files(
        &with(
            apple,
            &ci_edit(
                &at,
                "run: sh scripts/check.sh machine",
                "run: sh bench/scripts/libclang.sh",
            ),
        ),
        &sh,
        CI_NOT_RUN,
    )
    .expect_err("machine run by nobody");
    assert!(
        e.contains("the group `machine` of scripts/check.sh runs in no CI job"),
        "{e}"
    );
    // A workflow that checks nothing, given a change's event.
    let other = ".github/workflows/comparators.yml";
    let ot = text(other);
    for (from, to) in [
        (
            "  workflow_dispatch:\n",
            "  workflow_dispatch:\n  pull_request:\n",
        ),
        ("  workflow_dispatch:\n", "  pull_request_target:\n"),
        ("  workflow_dispatch:\n", "  workflow_dispatch:\n  push:\n"),
        (
            "  workflow_dispatch:\n",
            "  workflow_dispatch:\n  push:\n    branches: ['**']\n",
        ),
        (
            "  workflow_dispatch:\n",
            "  workflow_dispatch:\n  push:\n    branches: [main]\n    tags: ['*']\n",
        ),
        ("  workflow_dispatch:\n", "  workflow_run:\n"),
        ("  workflow_dispatch:\n", "  merge_group:\n"),
        ("  workflow_dispatch:\n", "  - pull_request\n"),
        ("\non:\n", "\non: [pull_request]\nx:\n"),
        ("\non:\n", "\n'on':\n"),
        ("\non:\n", "\ntrue:\n"),
    ] {
        let e =
            ci_check_files(&with(other, &ci_edit(&ot, from, to)), &sh, CI_NOT_RUN).expect_err(to);
        assert!(e.contains(other), "{to}: {e}");
    }
    // A push to main is not a change's event, and bank-row's is that.
    let ok = ci_edit(
        &ot,
        "  workflow_dispatch:\n",
        "  workflow_dispatch:\n  push:\n    branches: [main]\n",
    );
    ci_check_files(&with(other, &ok), &sh, CI_NOT_RUN).expect("a push to main");
    // A listed workflow gone.
    let gone: Vec<(String, String)> = files.iter().filter(|(n, _)| n != other).cloned().collect();
    let e = ci_check_files(&gone, &sh, CI_NOT_RUN).expect_err("gone");
    assert!(
        e.contains(&format!("{other}: listed by the test and not in the tree")),
        "{e}"
    );
    let gone: Vec<(String, String)> = files.iter().filter(|(n, _)| n != apple).cloned().collect();
    let e = ci_check_files(&gone, &sh, CI_NOT_RUN).expect_err("apple gone");
    assert!(
        e.contains(&format!("{apple}: listed by the test and not in the tree")),
        "{e}"
    );
    let _ = yaml;
}

/// `yaml` with the first occurrence of `from` replaced by `to`.
fn ci_edit(yaml: &str, from: &str, to: &str) -> String {
    assert!(yaml.contains(from), "the workflow has `{from}`");
    yaml.replacen(from, to, 1)
}

#[test]
fn a_ci_step_that_checks_outside_check_sh_fails() {
    let (yaml, sh) = ci_texts();
    ci_check(&yaml, &sh).expect("the workflow as it stands is green");
    let call = "run: sh scripts/check.sh wasm";
    // Each edit adds a step after the wasm group's call, so the verdict is
    // the step's alone and every group still runs.
    let red = |to: &str, want: &str| {
        let e =
            ci_check(&ci_edit(&yaml, call, &format!("{call}\n      - {to}")), &sh).expect_err(to);
        assert!(
            e.contains(want) && !e.contains("runs in no CI job"),
            "{to}: {e}"
        );
    };
    let outside = "is no call of scripts/check.sh";
    // A cargo, a python and a script of scripts/ called to check.
    red("run: cargo test --profile checked --lib", outside);
    red("run: python3 scripts/ratchet.py HEAD~1", outside);
    red("run: sh scripts/cost.sh HEAD~1", outside);
    // Shell that decides a base, in a block.
    red(
        "run: |\n          base=\"$PR_BASE\"\n          [ -n \"$base\" ] || base=$(git rev-parse HEAD~1)\n          COST_BASE=\"$base\" sh scripts/check.sh cost",
        outside,
    );
    // An environment set on the call, and a call with more after it.
    red("run: COST_BASE=HEAD~1 sh scripts/check.sh cost", outside);
    red(
        "run: sh scripts/check.sh wasm && cargo test",
        "no group of scripts/check.sh",
    );
    red(
        "run: sh scripts/check.sh wasm-not",
        "no group of scripts/check.sh",
    );
    // Set-up that hides a check, and a package list that runs something.
    red("run: sh bench/scripts/libclang.sh && cargo test", outside);
    red(
        "run: sudo apt-get update -qq && sudo apt-get install -y -qq valgrind && make",
        outside,
    );
    // A package name is a name: a `;` or a `&` inside one word runs what
    // follows it.
    red(
        "run: sudo apt-get update -qq && sudo apt-get install -y -qq valgrind;cargo test",
        outside,
    );
    red(
        "run: sudo apt-get update -qq && sudo apt-get install -y -qq valgrind&&cargo test",
        outside,
    );
    red(
        "run: sudo apt-get update -qq && sudo apt-get install -y -qq valgrind|sh",
        outside,
    );
    red(
        "run: sh scripts/check.sh wasm\n        shell: python3 {0}",
        "a shell of the step's own",
    );
    red("run: \"sh scripts/check.sh wasm\"", "a quoted run");
    red("run:", "a run with no command");
    // A run in flow style, and an action that checks.
    let e = ci_check(
        &ci_edit(
            &yaml,
            "  wasm:\n",
            "  wasm:\n    steps: [{run: cargo test}]\n",
        ),
        &sh,
    )
    .expect_err("a flow-style run");
    assert!(e.contains("a step the test cannot read"), "{e}");
    let e = ci_check(
        &ci_edit(
            &yaml,
            "uses: Swatinem/rust-cache@v2",
            "uses: actions-rs/clippy-check@v1",
        ),
        &sh,
    )
    .expect_err("an unknown action");
    assert!(e.contains("the action `actions-rs/clippy-check@v1`"), "{e}");
    // A continued line is one command.
    ci_check(
        &ci_edit(
            &yaml,
            call,
            "run: |\n          sh scripts/check.sh \\\n            wasm",
        ),
        &sh,
    )
    .unwrap_or_else(|e| panic!("a continued call: {e}"));
    // The keys around a run: a condition, a verdict ignored, an
    // environment the test does not list, a key it does not know, an
    // anchor, and a step that is two things.
    let lists = "        run: sh scripts/check.sh lists\n";
    let cost = "          SUPDB_PR_BASE: ${{ github.event.pull_request.base.sha }}\n          SUPDB_PUSH_BEFORE: ${{ github.event.before }}\n        run: sh scripts/check.sh cost\n";
    let cross = "      - name: Build and test under emulation\n";
    let edits: &[(&str, &str, &str)] = &[
        (lists, "        if: false\n        run: sh scripts/check.sh lists\n", "an `if:` the test does not list"),
        ("  lists:\n", "  lists:\n    if: github.event_name == 'workflow_dispatch'\n", "a job's `if:`"),
        ("  wasm:\n", "  wasm:\n    if: false\n", "a job's `if:`"),
        (lists, "        continue-on-error: true\n        run: sh scripts/check.sh lists\n", "continue-on-error"),
        ("  cost:\n", "  cost:\n    continue-on-error: true\n", "continue-on-error"),
        (cross, "      - name: Build and test under emulation\n        env:\n          CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS: --cfg supdb_emulated\n", "the environment `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"),
        (cost, "          SUPDB_BASE: ${{ github.event.pull_request.base.sha || github.event.before }}\n        run: sh scripts/check.sh cost\n", "the environment `SUPDB_BASE"),
        (lists, "        env:\n          SUPDB_BASE: HEAD\n        run: sh scripts/check.sh lists\n", "the environment `SUPDB_BASE: HEAD`"),
        (lists, "        env:\n          RATCHET_RED_STEPS: B\n        run: sh scripts/check.sh lists\n", "the environment `RATCHET_RED_STEPS: B`"),
        (cost, "          SUPDB_PR_BASE: ${{ github.event.pull_request.base.sha }}\n          SUPDB_PUSH_BEFORE: ${{ github.event.before }}\n          COST_PROBES: read\n        run: sh scripts/check.sh cost\n", "the environment `COST_PROBES: read`"),
        ("env:\n  CARGO_TERM_COLOR: always\n", "env:\n  CARGO_TERM_COLOR: always\n  SUPDB_BASE: HEAD\n", "the environment `SUPDB_BASE: HEAD`"),
        ("  wasm:\n", "  wasm:\n    env:\n      COST_BASE: HEAD\n", "the environment `COST_BASE: HEAD`"),
        ("  wasm:\n", "  wasm:\n    defaults:\n      run:\n        working-directory: bench\n", "the key `defaults`"),
        (lists, "        working-directory: bench\n        run: sh scripts/check.sh lists\n", "the key `working-directory`"),
        ("        with:\n          fetch-depth: 0\n", "        with:\n          fetch-depth: 0\n          ref: HEAD~1\n", "the input `ref` of `actions/checkout@v4`"),
        (lists, "        uses: actions/checkout@v4\n        run: sh scripts/check.sh lists\n", "both a run and an action"),
        (cross, "      - <<: *anchor\n      - name: Build and test under emulation\n", "a step the test cannot read"),
        (cross, "      - name: x\n        <<: *anchor\n        run: cargo test\n      - name: Build and test under emulation\n", "a step the test cannot read"),
        (cross, "      - name: &a x\n      - name: Build and test under emulation\n", "a step the test cannot read"),
        ("  wasm:\n", "  wasm:\n    env: {COST_BASE: HEAD}\n", "a step the test cannot read"),
        ("        if: always()\n", "        if: failure()\n", "an `if:` the test does not list"),
        // A name the test lists, with a value it does not.
        ("          SUPDB_PR_BASE: ${{ github.event.pull_request.base.sha }}\n", "          SUPDB_PR_BASE: HEAD~1\n", "the environment `SUPDB_PR_BASE: HEAD~1`"),
        // An input the test lists for another action.
        ("        with:\n          fetch-depth: 0\n", "        with:\n          fetch-depth: 0\n          path: elsewhere\n", "the input `path` of `actions/checkout@v4`"),
        // A list where the workflow has none, and a step's keys with no step.
        ("env:\n  CARGO_TERM_COLOR: always\n", "env:\n  - CARGO_TERM_COLOR: always\n", "a list outside a job's steps"),
        ("  wasm:\n", "  wasm:\n    steps:\n      name: x\n", "a key before any step"),
    ];
    for (from, to, want) in edits {
        let e = ci_check(&ci_edit(&yaml, from, to), &sh).expect_err(to);
        assert!(e.contains(want), "{to}: {e}");
    }
    // A condition on a check, or on its job, is a group nobody may run.
    for (from, to) in [
        (
            lists,
            "        if: false\n        run: sh scripts/check.sh lists\n",
        ),
        (
            "  lists:\n",
            "  lists:\n    if: github.event_name == 'workflow_dispatch'\n",
        ),
    ] {
        let e = ci_check(&ci_edit(&yaml, from, to), &sh).expect_err(to);
        assert!(
            e.contains("the group `lists` of scripts/check.sh runs in no CI job"),
            "{to}: {e}"
        );
    }
    // The one condition the test lists is on the step that publishes.
    let moved = ci_edit(&yaml, "        if: always()\n", "");
    let moved = ci_edit(
        &moved,
        lists,
        "        if: always()\n        run: sh scripts/check.sh lists\n",
    );
    let e = ci_check(&moved, &sh).expect_err("always() on a check");
    assert!(e.contains("an `if:` the test does not list"), "{e}");
    // A job named as a key of another place is read as a job: its
    // condition, its other keys, and its groups under the condition.
    let renamed = ci_edit(&yaml, "  lists:\n", "  inputs:\n    if: false\n");
    let e = ci_check(&renamed, &sh).expect_err("a job named `inputs` with a condition");
    assert!(
        e.contains("a job's `if:`")
            && e.contains("the group `lists` of scripts/check.sh runs in no CI job"),
        "{e}"
    );
    for name in [
        "inputs",
        "push",
        "workflow_dispatch",
        "matrix",
        "with",
        "on",
    ] {
        let e = ci_check(
            &ci_edit(
                &yaml,
                "  lists:\n",
                &format!("  {name}:\n    needs: test\n"),
            ),
            &sh,
        )
        .expect_err(name);
        assert!(e.contains("the key `needs`"), "{name}: {e}");
    }
    // An environment is read where an environment stands, and nowhere
    // else: under a strategy it is a key the test does not know.
    let e = ci_check(
        &ci_edit(
            &yaml,
            "      fail-fast: false\n",
            "      fail-fast: false\n      env:\n        CARGO_TERM_COLOR: always\n",
        ),
        &sh,
    )
    .expect_err("env under a strategy");
    assert!(e.contains("the key `env`"), "{e}");
    // The triggers: every one the test lists, with its value, and no other.
    for (from, to, want) in [
        ("  pull_request:\n", "", "the trigger `pull_request`"),
        (
            "  push:\n    branches: [main]\n",
            "  push:\n    branches: [never]\n",
            "the trigger `push.branches: [never]`",
        ),
        ("    tags: ['*']\n", "", "the trigger `push.tags`"),
        (
            "  workflow_dispatch:\n",
            "  workflow_dispatch:\n    branches: [main]\n",
            "the key `branches`",
        ),
    ] {
        let e = ci_check(&ci_edit(&yaml, from, to), &sh).expect_err(to);
        assert!(e.contains(want), "{to}: {e}");
    }
}

#[test]
fn a_group_no_ci_job_runs_fails() {
    let (yaml, sh) = ci_texts();
    ci_check(&yaml, &sh).expect("the workflow as it stands is green");
    let e = ci_check(
        &ci_edit(
            &yaml,
            "run: sh scripts/check.sh wasm",
            "run: sh bench/scripts/libclang.sh",
        ),
        &sh,
    )
    .expect_err("wasm run by nobody");
    assert!(
        e.contains("the group `wasm` of scripts/check.sh runs in no CI job"),
        "{e}"
    );
    let more = ci_edit(&sh, "    wasm)\n", "    newgroup)\n      ;;\n    wasm)\n");
    let e = ci_check(&yaml, &more).expect_err("a new group");
    assert!(e.contains("the group `newgroup`"), "{e}");
    let groups = check_groups(&sh).expect("check.sh's labels read");
    assert!(groups.contains("wasm") && !groups.contains("*"));
    // Two groups in one label, and a label the test cannot read.
    let two = ci_edit(&sh, "    wasm)\n", "    newa|newb)\n      ;;\n    wasm)\n");
    let e = ci_check(&yaml, &two).expect_err("two new groups");
    assert!(
        e.contains("the group `newa`") && e.contains("the group `newb`"),
        "{e}"
    );
    // A case inside a group is not the end of the groups' case.
    for nested in [
        "    build)\n      case \"$x\" in\n        a) ;;\n      esac\n",
        "    build)\n      case \"$x\" in\n        a)\n          ;;\n      esac\n",
        "    build)\n      case \"$x\" in a) ;; esac\n",
    ] {
        let more = ci_edit(&sh, "    build)\n", nested);
        let more = ci_edit(&more, "    wasm)\n", "    newgroup)\n      ;;\n    wasm)\n");
        let e = ci_check(&yaml, &more).expect_err(nested);
        assert!(e.contains("the group `newgroup`"), "{nested}: {e}");
    }
    let odd = ci_edit(
        &sh,
        "    build)\n",
        "    build)\n      true && case \"$x\" in\n",
    );
    let e = ci_check(&yaml, &odd).expect_err("a case the test cannot read");
    assert!(e.contains("a `case` or `esac` the test cannot read"), "{e}");
    let deep = ci_edit(&sh, "    wasm)\n", "      newc)\n      ;;\n    wasm)\n");
    let e = ci_check(&yaml, &deep).expect_err("a label at another indent");
    assert!(
        e.contains("a label of scripts/check.sh the test cannot read"),
        "{e}"
    );
    // A group no job runs, said so with a reason, passes; said so without
    // one, or said of a group a job runs or of no group, fails.
    let none = ci_edit(
        &yaml,
        "run: sh scripts/check.sh wasm",
        "run: sh bench/scripts/libclang.sh",
    );
    ci_check_with(&none, &sh, &[("wasm", "a reason")])
        .unwrap_or_else(|e| panic!("wasm said not run: {e}"));
    let e = ci_check_with(&none, &sh, &[("wasm", " ")]).expect_err("no reason");
    assert!(e.contains("CI_NOT_RUN gives `wasm` no reason"), "{e}");
    let e = ci_check_with(&yaml, &sh, &[("wasm", "a reason")]).expect_err("run and said not");
    assert!(
        e.contains("the group `wasm` runs in CI and CI_NOT_RUN says it does not"),
        "{e}"
    );
    let e = ci_check_with(&yaml, &sh, &[("nogroup", "a reason")]).expect_err("no such group");
    assert!(e.contains("CI_NOT_RUN names `nogroup`, no group"), "{e}");
}

// ---------------------------------------------------------------------------
// The seal's tokens (forms.md R2, plan G1-7). The compiler refuses a whole
// borrow outside its reader because only that reader can make the token the
// borrow takes; what the compiler cannot see is the token's own module
// growing a second way to make one (a derive, an impl, a function that
// hands one out), a token made by unsafe code, or a new caller inside the
// reader's own module. This holds each of those by the source, reading
// names as identifiers and not text, so a qualified path, a renamed import,
// a call by type or through a function value is read as the plain spelling
// is (`a_seals_token_spelled_another_way_fails`); inside a macro's tokens,
// where a name handed in as an identifier is put where the macro chooses
// (`mk!(lend)` making `$m::Token::new()`, `call!(m, lent, t)` making
// `$b.$m($t)`), `lent` and `lend` are refused in every position; and
// outside the crate, where only unsafe code can make a token, `lend_at` is
// not named at all. What it cannot read is a name a macro builds from
// pieces (`concat_idents!` is unstable, `paste` is no dependency), and code
// from outside a listed file is the wall's to refuse.

/// The token modules: the file, the module path the constructor is
/// confined to.
const SEAL_TOKENS: &[(&str, &str)] = &[
    ("src/blob.rs", "crate::blob"),
    ("src/ordindex.rs", "crate::ordindex"),
    ("src/db/zfile.rs", "crate::db::zfile"),
];

/// The calls a token is made for, by the name the call is made by: the
/// files and scopes that may make it, and the token as it is written at the
/// call. A name is read as an identifier, whatever path or receiver it is
/// reached by -- `m.lent(..)`, `MmapBytes::lent(m, ..)`, `<B as
/// Bytes>::lend_at` as a value -- so the spelling of the call does not
/// matter; `lent` and `base` are common names and are read only where they
/// are a method's (after `.` with a call, after `::`), `fn` ahead of a name
/// being its definition -- outside a macro: inside one `lent` and `lend`
/// are refused wherever they stand (`seal_in_macro`, `SEAL_MACRO_WORDS`).
type SealHomes = &'static [(&'static str, &'static [&'static str])];
const SEAL_CALLS: &[(&str, SealHomes, &str)] = &[
    (
        "lend_at",
        &[("src/blob.rs", &["Sec::lend", "Sec::get"])],
        "lend::Token::new())",
    ),
    (
        "lent",
        &[
            (
                "src/ordindex.rs",
                &["OrdIndex::over", "OrdIndex::mapped_heads"],
            ),
            (
                "src/db/zfile.rs",
                &[
                    "ZFile::attach_stride",
                    "ZFile::stride_bytes",
                    "check_stride",
                    "file_facts",
                ],
            ),
        ],
        "lend::Token::new())",
    ),
    (
        "base",
        &[("src/bytes.rs", &["MmapBytes::paged"])],
        "Base(()))",
    ),
];

/// The lines that name a seal's token and make no call, by file and text:
/// the token modules, the imports and re-exports of the tokens, the
/// signatures that take one, and the seam's impls of the readers' trait.
/// A token's names are the module `lend` as a path (`lend::`, `mod lend`,
/// any `use` of it), the three names the tokens are imported and
/// re-exported under, and `Base` (in the seam, and as `bytes::Base`
/// elsewhere). Every other line that names one is a call's, made at the
/// call, or red: a token made apart from its call, by any path, under a
/// renamed import or the re-export's name, cannot be spent.
const SEAL_DECLS: &[(&str, &str)] = &[
    ("src/blob.rs", "pub(crate) mod lend {"),
    ("src/ordindex.rs", "pub(crate) mod lend {"),
    ("src/db/zfile.rs", "pub(crate) mod lend {"),
    ("src/bytes.rs", "use crate::blob::lend::Token as LendToken;"),
    ("src/bytes.rs", "use crate::db::StrideLendToken;"),
    ("src/bytes.rs", "use crate::ordindex::OrdLendToken;"),
    ("src/bytes.rs", "impl whole::Reader for OrdLendToken {}"),
    ("src/bytes.rs", "impl whole::Reader for StrideLendToken {}"),
    (
        "src/bytes.rs",
        "fn lend_at(&self, off: u64, len: usize, _: LendToken) -> Option<&[u8]> {",
    ),
    ("src/bytes.rs", "fn base(&self, _: Base) -> *const u8;"),
    ("src/bytes.rs", "pub(crate) struct Base(());"),
    (
        "src/ordindex.rs",
        "pub(crate) use lend::Token as OrdLendToken;",
    ),
    (
        "src/ordindex.rs",
        "fn base(&self, _: crate::bytes::Base) -> *const u8 {",
    ),
    (
        "src/db/pager.rs",
        "fn base(&self, _: crate::bytes::Base) -> *const u8 {",
    ),
    (
        "src/db.rs",
        "pub(crate) use zfile::lend::Token as StrideLendToken;",
    ),
];

/// The names a line calls a seal's call by, and the names of a seal's
/// token it holds, read off the line with its literals blanked. `in_use`:
/// the line is inside a `use` item.
fn seal_names(file: &str, flat: &str, in_use: bool) -> (Vec<&'static str>, usize) {
    let c: Vec<char> = flat.chars().collect();
    let before = |i: usize| -> String {
        let mut j = i;
        while j > 0 && c[j - 1].is_whitespace() {
            j -= 1;
        }
        let st = j.saturating_sub(2);
        c[st..j].iter().collect()
    };
    let after = |i: usize| -> String {
        let mut j = i;
        while j < c.len() && c[j].is_whitespace() {
            j += 1;
        }
        let mut k = j;
        let mut out = String::new();
        while k < c.len() && out.len() < 2 {
            if !c[k].is_whitespace() {
                out.push(c[k]);
            }
            k += 1;
        }
        out
    };
    let ws = words(&c);
    let mut calls = Vec::new();
    let mut toks = 0;
    for (n, (i, w)) in ws.iter().enumerate() {
        let end = i + w.chars().count();
        let (pre, post) = (before(*i), after(end));
        let def = n > 0 && ws[n - 1].1 == "fn";
        let dot = pre.ends_with('.') && !pre.ends_with("..");
        let path = pre == "::";
        match w.as_str() {
            "lend_at" if !def => calls.push("lend_at"),
            "lent" if !def && (path || (dot && (post.starts_with('(') || post == "::"))) => {
                calls.push("lent")
            }
            "base"
                if !def
                    && ((dot && post.starts_with('(') && post != "()")
                        || (path && post != "()")) =>
            {
                calls.push("base")
            }
            "lend" if post == "::" || in_use || (n > 0 && ws[n - 1].1 == "mod") => toks += 1,
            "LendToken" | "OrdLendToken" | "StrideLendToken" => toks += 1,
            "Base" if file == "src/bytes.rs" || (path && n > 0 && ws[n - 1].1 == "bytes") => {
                toks += 1
            }
            _ => {}
        }
    }
    (calls, toks)
}

/// The lines that name `lent` or `lend` inside a macro's tokens and are not
/// the seal's, by file, scope and text: locals of those names in the
/// assertions of tests. Keyed by the scope as well, so the same text in a
/// macro of another function -- `lent,` as a macro's argument -- is red.
const SEAL_MACRO_WORDS: &[(&str, &str, &str)] = &[
    (
        "src/crypt.rs",
        "every_range_reads_as_the_plaintext_does",
        "Some(lent) => assert_eq!(lent, &p[a..b], \"lent {a}..{b}, {what}\"),",
    ),
    (
        "src/db.rs",
        "a_writer_that_has_lent_its_upkeep_leaves_the_sweep_to_the_thread_that_has_it",
        "lent,",
    ),
    (
        "src/db/pager.rs",
        "nothing_is_given_back_under_a_hold",
        "assert_eq!(lent, &bytes[..], \"the bytes lent are still the bytes\");",
    ),
    (
        "src/flatindex.rs",
        "rank_order_matches_key_order",
        "assert_eq!(lent.as_deref(), Some(e.as_slice()));",
    ),
];

/// Which characters of a flat line are inside a macro's tokens -- an
/// invocation's delimiters and what they hold, or a `macro_rules!`
/// definition's -- carrying the bracket depth and the open macros' depths
/// across lines. Inside a macro a name's position is the macro's to choose
/// (`$b.$m($t)`, `$m::Token`), so no reading by position holds it there.
fn seal_in_macro(c: &[char], depth: &mut i64, open: &mut Vec<i64>) -> Vec<bool> {
    const KEYWORDS: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "yield",
    ];
    let skip = |mut j: usize| {
        while j < c.len() && c[j].is_whitespace() {
            j += 1;
        }
        j
    };
    // The delimiters that open a macro: after `name!`, or `macro_rules!
    // name`, with any whitespace between.
    let mut opener = vec![false; c.len()];
    for (i, &ch) in c.iter().enumerate() {
        if ch != '!' {
            continue;
        }
        let mut b = i;
        while b > 0 && c[b - 1].is_whitespace() {
            b -= 1;
        }
        let mut a = b;
        while a > 0 && lex::is_word(c[a - 1]) {
            a -= 1;
        }
        let name: String = c[a..b].iter().collect();
        if name.is_empty() || KEYWORDS.contains(&name.as_str()) {
            continue;
        }
        let mut j = skip(i + 1);
        if name == "macro_rules" {
            while j < c.len() && lex::is_word(c[j]) {
                j += 1;
            }
            j = skip(j);
        }
        if j < c.len() && matches!(c[j], '(' | '[' | '{') {
            opener[j] = true;
        }
    }
    let mut inside = vec![false; c.len()];
    for (i, &ch) in c.iter().enumerate() {
        match ch {
            '(' | '[' | '{' => {
                if opener[i] {
                    open.push(*depth);
                }
                *depth += 1;
                inside[i] = !open.is_empty();
            }
            ')' | ']' | '}' => {
                inside[i] = !open.is_empty();
                *depth -= 1;
                if open.last() == Some(depth) {
                    open.pop();
                }
            }
            _ => inside[i] = !open.is_empty(),
        }
    }
    inside
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn seal_check(src: &BTreeMap<String, String>) -> Result<(), String> {
    let mut errs = Vec::new();
    let code_of = |f: &str| -> Vec<(String, String, bool)> {
        regions(&src[f])
            .into_iter()
            .map(|l| (l.code, l.scope, l.test))
            .collect()
    };
    // 1. Each token module is a struct with a private field and one
    // constructor confined to its module: no derive, no impl, no field.
    for (f, path) in SEAL_TOKENS {
        let lines = code_of(f);
        let text: String = lines.iter().map(|(c, _, _)| format!("{c}\n")).collect();
        let starts: Vec<usize> = text.match_indices("mod lend").map(|(i, _)| i).collect();
        if starts.len() != 1 {
            errs.push(format!(
                "{f}: {} `mod lend` where one is the seal's",
                starts.len()
            ));
            continue;
        }
        let at = text[..starts[0]].rfind('\n').map_or(0, |i| i + 1);
        let Some(open) = text[at..].find('{').map(|i| at + i) else {
            errs.push(format!("{f}: `mod lend` has no body"));
            continue;
        };
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            errs.push(format!("{f}: `mod lend` does not close"));
            continue;
        };
        let want = format!(
            "pub(crate) mod lend {{ pub struct Token(()); impl Token {{ #[inline(always)] \
             pub(in {path}) fn new() -> Token {{ Token(()) }} }} }}"
        );
        let got = squash(&text[at..end]);
        if got != want {
            errs.push(format!(
                "{f}: the token's module is `{got}`, where the seal's is `{want}`"
            ));
        }
    }
    // 2. The address token: a unit struct with a private field, no derive,
    // made at the one call.
    let bytes = code_of("src/bytes.rs");
    let defs: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, (c, _, _))| c.contains("struct Base"))
        .map(|(i, _)| i)
        .collect();
    match defs.as_slice() {
        [i] => {
            if squash(&bytes[*i].0) != "pub(crate) struct Base(());" {
                errs.push(format!(
                    "src/bytes.rs: `Base` is `{}`",
                    squash(&bytes[*i].0)
                ));
            }
            let mut j = *i;
            while j > 0 {
                j -= 1;
                let c = bytes[j].0.trim();
                if c.is_empty() {
                    continue;
                }
                if !c.starts_with("#[") {
                    break;
                }
                if c != "#[cfg(not(target_family = \"wasm\"))]" {
                    errs.push(format!("src/bytes.rs: `Base` carries `{c}`"));
                }
            }
        }
        _ => errs.push(format!(
            "src/bytes.rs: {} definitions of `Base`",
            defs.len()
        )),
    }
    // 3. No impl gives a token anything but the seal's own (the
    // constructor in its module, the readers' trait in the seam).
    let allowed_impls = [
        "impl Token {",
        "impl whole::Reader for OrdLendToken {}",
        "impl whole::Reader for StrideLendToken {}",
    ];
    for f in src.keys() {
        if !f.ends_with(".rs") {
            continue;
        }
        let home = SEAL_TOKENS.iter().any(|(h, _)| h == f);
        for (c, scope, _) in code_of(f) {
            let t = squash(&c);
            if !t.contains("impl") {
                continue;
            }
            let names_token = t.contains("lend::Token")
                || t.contains("LendToken")
                || t.contains("whole::Reader")
                || (home
                    && words(&t.chars().collect::<Vec<_>>())
                        .iter()
                        .any(|(_, w)| w == "Token"))
                || (f == "src/bytes.rs"
                    && words(&t.chars().collect::<Vec<_>>())
                        .iter()
                        .any(|(_, w)| w == "Base"));
            if names_token && !allowed_impls.contains(&t.as_str()) {
                errs.push(format!("{f} ({scope}): `{t}` gives a seal's token an impl"));
            }
        }
    }
    // 4. A seal's call is made only by its readers, in the scopes listed,
    // with its token made as its last argument; a line that names a token
    // and makes no call is one of the listed lines. A new reader of a
    // mapping as it lies is a line of SEAL_CALLS, read in review.
    for f in src.keys() {
        if !f.ends_with(".rs") {
            continue;
        }
        let mut in_use = false;
        let (mut depth, mut open) = (0i64, Vec::new());
        for l in regions(&src[f]) {
            let t = squash(&l.code);
            let ft = squash(&l.flat);
            let fc: Vec<char> = l.flat.chars().collect();
            let inside = seal_in_macro(&fc, &mut depth, &mut open);
            let in_macro = words(&fc)
                .iter()
                .any(|(i, w)| (w == "lent" || w == "lend") && inside[*i]);
            if in_macro && !SEAL_MACRO_WORDS.contains(&(f.as_str(), l.scope.as_str(), t.as_str())) {
                errs.push(format!(
                    "{f} ({}): `{t}` names `lent` or `lend` inside a macro, where the macro \
                     chooses its place (SEAL_MACRO_WORDS)",
                    l.scope
                ));
            }
            let opens_use = ft.starts_with("use ")
                || ft.starts_with("pub use ")
                || (ft.starts_with("pub(") && ft.contains(") use "));
            in_use |= opens_use;
            let (calls, toks) = seal_names(f, &l.flat, in_use);
            if in_use && ft.contains(';') {
                in_use = false;
            }
            let scope = &l.scope;
            if SEAL_DECLS.contains(&(f.as_str(), t.as_str())) && calls.is_empty() {
                continue;
            }
            if calls.is_empty() {
                if toks > 0 {
                    errs.push(format!(
                        "{f} ({scope}): `{t}` names a seal's token outside the call it is made for"
                    ));
                }
                continue;
            }
            let mut made = 0;
            for (call, homes, token) in SEAL_CALLS {
                if !calls.contains(call) {
                    continue;
                }
                let named: Vec<&str> = homes.iter().map(|(h, _)| *h).collect();
                match homes.iter().find(|(h, _)| h == f) {
                    None => errs.push(format!(
                        "{f} ({scope}): `{t}` calls `{call}` outside {}",
                        named.join(", ")
                    )),
                    Some((_, scopes)) if !scopes.contains(&scope.as_str()) => errs.push(format!(
                        "{f} ({scope}): `{t}`: a caller of `{call}` that is not listed (SEAL_CALLS)"
                    )),
                    Some(_) => made += t.matches(token).count(),
                }
            }
            if made != calls.len() || toks != calls.len() {
                errs.push(format!(
                    "{f} ({scope}): `{t}`: `{}` with a token not made at the call",
                    calls.join("`, `")
                ));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

/// The Rust files of the tests, the examples and the suite: code outside
/// the crate, which reaches `Bytes::lend_at` by its name alone (`lent` and
/// `Paging` are the crate's), with a token made by unsafe code
/// (`std::mem::zeroed()`) since none can be made safely there.
fn outside_sources(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = rd.map(|e| e.expect("an entry").path()).collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                if name != "target" && !name.starts_with('.') {
                    walk(&p, root, out);
                }
            } else if name.ends_with(".rs") {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(&p).expect("a file reads");
                out.insert(rel, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    let mut out = BTreeMap::new();
    for d in ["tests", "examples", "bench"] {
        walk(&root.join(d), root, &mut out);
    }
    out
}

/// No code outside the crate names `lend_at`: it cannot override it (the
/// token's module is private) and cannot make its token safely, so a call
/// there is one whose token was made by unsafe code.
fn seal_outside_check(out: &BTreeMap<String, String>) -> Result<(), String> {
    let mut errs = Vec::new();
    for (f, text) in out {
        for l in regions(text) {
            let (calls, _) = seal_names(f, &l.flat, false);
            if calls.contains(&"lend_at") {
                errs.push(format!(
                    "{f} ({}): `{}` calls `lend_at` outside the crate",
                    l.scope,
                    squash(&l.code)
                ));
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

#[test]
fn the_seals_tokens_are_made_only_where_they_are_spent() {
    if let Err(e) = seal_check(&sources(&root())) {
        panic!("the seal's tokens:\n{e}");
    }
    let out = outside_sources(&root());
    assert!(
        out.contains_key("tests/blob.rs")
            && out.contains_key("examples/crypt.rs")
            && out.contains_key("bench/src/engines.rs"),
        "the tests, the examples and the suite are read: {:?}",
        out.keys().take(5).collect::<Vec<_>>()
    );
    if let Err(e) = seal_outside_check(&out) {
        panic!("the seal's token outside the crate:\n{e}");
    }
}

/// The check's red inputs: the tree with one change each, every one a way
/// round the seal that compiles.
#[test]
fn a_seals_token_made_another_way_fails() {
    let src = sources(&root());
    let edit = |f: &str, old: &str, new: &str| {
        let mut s = src.clone();
        let t = s.get_mut(f).expect("the file");
        assert_eq!(t.matches(old).count(), 1, "{f}: {old}");
        *t = t.replacen(old, new, 1);
        s
    };
    let red = |s: &BTreeMap<String, String>, want: &str| {
        let e = seal_check(s).expect_err(want);
        assert!(e.contains(want), "wanted `{want}` in:\n{e}");
    };
    seal_check(&src).expect("the tree");
    // A derive on the token, which an inferred `Default::default()` takes.
    red(
        &edit(
            "src/blob.rs",
            "pub(crate) mod lend {\n    pub struct Token(());",
            "pub(crate) mod lend {\n    #[derive(Default)]\n    pub struct Token(());",
        ),
        "src/blob.rs: the token's module is",
    );
    // A constructor widened.
    red(
        &edit(
            "src/ordindex.rs",
            "pub(in crate::ordindex) fn new()",
            "pub(crate) fn new()",
        ),
        "src/ordindex.rs: the token's module is",
    );
    // An impl outside the module.
    red(
        &edit(
            "src/blob.rs",
            "/// A section of the file: lent by the source",
            "impl Default for lend::Token {\n    fn default() -> Self {\n        lend::Token::new()\n    }\n}\n\n/// A section of the file: lent by the source",
        ),
        "gives a seal's token an impl",
    );
    // A function of blob that hands a token out.
    red(
        &edit(
            "src/blob.rs",
            "/// A section of the file: lent by the source",
            "pub(crate) fn token() -> lend::Token {\n    lend::Token::new()\n}\n\n/// A section of the file: lent by the source",
        ),
        "names a seal's token outside the call it is made for",
    );
    // A call abroad, whatever its token (one made by unsafe code here).
    red(
        &edit(
            "src/db/zfile.rs",
            "use std::sync::Arc;\n",
            "use std::sync::Arc;\n#[allow(dead_code)]\nfn conjured(m: &MmapBytes) -> Option<&[u8]> {\n    m.lend_at(0, 1, unsafe { std::mem::zeroed() })\n}\n",
        ),
        "calls `lend_at` outside src/blob.rs",
    );
    // A token made apart from its call, and spent through a binding.
    red(
        &edit(
            "src/blob.rs",
            "Sec::Lent { off, len } => src.lend_at(*off, *len, lend::Token::new()),",
            "Sec::Lent { off, len } => {\n                let t = lend::Token::new();\n                src.lend_at(*off, *len, t)\n            }",
        ),
        "with a token not made at the call",
    );
    // A new reader of a mapping as it lies, at home.
    red(
        &edit(
            "src/db/zfile.rs",
            "use std::sync::Arc;\n",
            "use std::sync::Arc;\n#[allow(dead_code)]\nfn another(m: &MmapBytes) -> Option<usize> {\n    let all = m.lent(lend::Token::new());\n    if all.is_empty() {\n        return None;\n    }\n    Some(all.len())\n}\n",
        ),
        "a caller of `lent` that is not listed",
    );
    // The readers' trait given to every type, which takes any value.
    red(
        &edit(
            "src/bytes.rs",
            "impl whole::Reader for StrideLendToken {}",
            "impl whole::Reader for StrideLendToken {}\n#[cfg(not(target_family = \"wasm\"))]\nimpl<T> whole::Reader for (T,) {}",
        ),
        "gives a seal's token an impl",
    );
    // The address token given a derive, or a constructor.
    red(
        &edit(
            "src/bytes.rs",
            "pub(crate) struct Base(());",
            "#[derive(Default)]\npub(crate) struct Base(());",
        ),
        "src/bytes.rs: `Base` carries `#[derive(Default)]`",
    );
    red(
        &edit(
            "src/bytes.rs",
            "pub(crate) struct Base(());",
            "pub(crate) struct Base(());\nimpl Base {\n    pub(crate) fn new() -> Base {\n        Base(())\n    }\n}",
        ),
        "gives a seal's token an impl",
    );
    // The address asked for outside the seam.
    red(
        &edit(
            "src/db/pager.rs",
            "use crate::bytes::Paging;\n",
            "use crate::bytes::Paging;\n#[allow(dead_code)]\nfn addr(p: &dyn crate::bytes::Paging, b: crate::bytes::Base) -> *const u8 {\n    p.base(b)\n}\n",
        ),
        "calls `base` outside src/bytes.rs",
    );
}

/// The same routes in the other spellings Rust gives them: a token made by
/// a qualified path, a renamed import or the re-export's name, and a call
/// made by type or through a function value. Each compiles with no unsafe.
#[test]
fn a_seals_token_spelled_another_way_fails() {
    let src = sources(&root());
    let edit = |s: &BTreeMap<String, String>, f: &str, old: &str, new: &str| {
        let mut s = s.clone();
        let t = s.get_mut(f).expect("the file");
        assert_eq!(t.matches(old).count(), 1, "{f}: {old}");
        *t = t.replacen(old, new, 1);
        s
    };
    // Every case is run and every miss said, so a red run names each route
    // the check let through.
    let misses = std::cell::RefCell::new(Vec::new());
    let red = |s: &BTreeMap<String, String>, want: &[&str]| match seal_check(s) {
        Ok(()) => misses
            .borrow_mut()
            .push(format!("passed the check: {}", want[0])),
        Err(e) => {
            for w in want {
                if !e.contains(w) {
                    misses.borrow_mut().push(format!("wanted `{w}` in:\n{e}"));
                }
            }
        }
    };
    let ord = "use std::path::Path;\n";
    let zf = "use std::sync::Arc;\n";
    let bl = "/// A section of the file: lent by the source";
    let db = "use std::cell::UnsafeCell;\n";
    // A function of the ordered index that hands a token out by a qualified
    // path, spent by type abroad: a whole borrow outside the readers.
    let hand = edit(
        &src,
        "src/ordindex.rs",
        ord,
        &format!(
            "{ord}#[allow(dead_code)]\npub(crate) fn hand() -> lend::Token {{\n    <lend::Token>::new()\n}}\n"
        ),
    );
    red(
        &hand,
        &[
            "src/ordindex.rs (hand)",
            "names a seal's token outside the call it is made for",
        ],
    );
    let both = edit(
        &hand,
        "src/db.rs",
        db,
        &format!(
            "{db}#[allow(dead_code)]\nfn sneak(m: &crate::bytes::MmapBytes) -> usize {{\n    crate::bytes::MmapBytes::lent(m, crate::ordindex::hand()).len()\n}}\n"
        ),
    );
    red(&both, &["src/db.rs (sneak)", "calls `lent` outside"]);
    // A new reader at home, by type and a qualified token.
    red(
        &edit(
            &src,
            "src/db/zfile.rs",
            zf,
            &format!(
                "{zf}#[allow(dead_code)]\nfn another(m: &MmapBytes) -> usize {{\n    MmapBytes::lent(m, <lend::Token>::new()).len()\n}}\n"
            ),
        ),
        &["src/db/zfile.rs (another)", "a caller of `lent` that is not listed"],
    );
    // A renamed import and a call through a function value, in blob.
    red(
        &edit(
            &src,
            "src/blob.rs",
            bl,
            &format!(
                "use lend::Token as Tk;\n#[allow(dead_code)]\nfn lend_any<B: Bytes>(src: &B) -> Option<&[u8]> {{\n    let f = <B as Bytes>::lend_at;\n    f(src, 0, 1, Tk::new())\n}}\n\n{bl}"
            ),
        ),
        &[
            "src/blob.rs (<item>): `use lend::Token as Tk;`",
            "src/blob.rs (lend_any)",
            "a caller of `lend_at` that is not listed",
        ],
    );
    // The re-export's name at home, a glob of the token's module, and the
    // address asked for by a function value.
    red(
        &edit(
            &src,
            "src/ordindex.rs",
            ord,
            &format!(
                "{ord}#[allow(dead_code)]\nfn by_name(m: &MmapBytes) -> usize {{\n    m.lent(OrdLendToken::new()).len()\n}}\n"
            ),
        ),
        &["src/ordindex.rs (by_name)", "a caller of `lent` that is not listed"],
    );
    red(
        &edit(
            &src,
            "src/db/zfile.rs",
            zf,
            &format!("{zf}#[allow(unused_imports)]\nuse lend::*;\n"),
        ),
        &["src/db/zfile.rs (<item>): `use lend::*;`"],
    );
    red(
        &edit(
            &src,
            "src/db/pager.rs",
            "use crate::bytes::Paging;\n",
            "use crate::bytes::Paging;\n#[allow(dead_code)]\nfn addr(p: &dyn crate::bytes::Paging) -> fn(&dyn crate::bytes::Paging, crate::bytes::Base) -> *const u8 {\n    let _ = p;\n    <dyn crate::bytes::Paging>::base\n}\n",
        ),
        &["src/db/pager.rs (addr)", "calls `base` outside src/bytes.rs"],
    );
    // The token's module imported under another name, by a path that does
    // not go on past it.
    red(
        &edit(
            &src,
            "src/blob.rs",
            bl,
            &format!("use self::lend as mk;\n\n{bl}"),
        ),
        &["src/blob.rs (<item>): `use self::lend as mk;`"],
    );
    // A listed call that makes a second token beside its own.
    red(
        &edit(
            &src,
            "src/blob.rs",
            "Sec::Lent { off, len } => src.lend_at(*off, *len, lend::Token::new()),",
            "Sec::Lent { off, len } => src.lend_at(*off, *len, lend::Token::new()).filter(|_| keep(<lend::Token>::new())),",
        ),
        &["`lend_at` with a token not made at the call"],
    );
    // A token handed out under a re-export's name, at home.
    red(
        &edit(
            &src,
            "src/ordindex.rs",
            ord,
            &format!("{ord}#[allow(dead_code)]\npub(crate) fn hand() -> OrdLendToken {{\n    OrdLendToken::new()\n}}\n"),
        ),
        &["src/ordindex.rs (hand): `OrdLendToken::new()` names a seal's token outside the call"],
    );
    red(
        &edit(
            &src,
            "src/db/zfile.rs",
            zf,
            &format!("{zf}#[allow(dead_code)]\npub(crate) fn hand() -> crate::db::StrideLendToken {{\n    crate::db::StrideLendToken::new()\n}}\n"),
        ),
        &["src/db/zfile.rs (hand): `crate::db::StrideLendToken::new()` names a seal's token outside the call"],
    );
    // A macro of the reader's module that makes the call where it is
    // invoked: its text is at home, in a scope SEAL_CALLS does not list.
    red(
        &edit(
            &src,
            "src/ordindex.rs",
            ord,
            &format!(
                "{ord}macro_rules! whole_of {{\n    ($m:expr) => {{\n        $m.lent(lend::Token::new())\n    }};\n}}\n"
            ),
        ),
        &["a caller of `lent` that is not listed"],
    );
    // The names handed to macros as identifiers, in positions the reading
    // by position does not count: the token's module as a macro's argument
    // in ordindex, made into a path inside the macro, and `lent` as a
    // method name a macro of db.rs calls. Each compiles with no unsafe.
    let mac_hand = edit(
        &src,
        "src/ordindex.rs",
        ord,
        &format!(
            "{ord}macro_rules! ty {{\n    ($m:ident) => {{\n        $m::Token\n    }};\n}}\nmacro_rules! mk {{\n    ($m:ident) => {{\n        $m::Token::new()\n    }};\n}}\n#[allow(dead_code)]\npub(crate) fn hand() -> ty!(lend) {{\n    mk!(lend)\n}}\n"
        ),
    );
    red(
        &mac_hand,
        &["src/ordindex.rs (hand): `mk!(lend)` names `lent` or `lend` inside a macro"],
    );
    red(
        &edit(
            &mac_hand,
            "src/db.rs",
            db,
            &format!(
                "{db}#[allow(dead_code)]\nfn sneak(m: &crate::bytes::MmapBytes) -> usize {{\n    macro_rules! call {{\n        ($b:expr, $m:ident, $t:expr) => {{\n            $b.$m($t)\n        }};\n    }}\n    call!(m, lent, crate::ordindex::hand()).len()\n}}\n"
            ),
        ),
        &["src/db.rs (sneak): `call!(m, lent, crate::ordindex::hand()).len()` names `lent` or `lend` inside a macro"],
    );
    // An unlisted whole reader in a token home, by the same forwarding.
    red(
        &edit(
            &src,
            "src/db/zfile.rs",
            zf,
            &format!(
                "{zf}impl ZFile {{\n    #[allow(dead_code)]\n    fn sneak(&self) -> &[u8] {{\n        macro_rules! call {{\n            ($b:expr, $m:ident, $t:expr) => {{\n                $b.$m($t)\n            }};\n        }}\n        macro_rules! mk {{\n            ($m:ident) => {{\n                $m::Token::new()\n            }};\n        }}\n        call!(self.map, lent, mk!(lend))\n    }}\n}}\n"
            ),
        ),
        &["src/db/zfile.rs (ZFile::sneak): `call!(self.map, lent, mk!(lend))` names `lent` or `lend` inside a macro"],
    );
    // The same call with `lent` on a line of its own, the text of a line
    // SEAL_MACRO_WORDS lets through in another function.
    red(
        &edit(
            &mac_hand,
            "src/db.rs",
            db,
            &format!(
                "{db}#[allow(dead_code)]\nfn sneak(m: &crate::bytes::MmapBytes) -> usize {{\n    macro_rules! call {{\n        ($b:expr, $m:ident, $t:expr) => {{\n            $b.$m($t)\n        }};\n    }}\n    call!(\n        m,\n        lent,\n        crate::ordindex::hand()\n    )\n    .len()\n}}\n"
            ),
        ),
        &["src/db.rs (sneak): `lent,` names `lent` or `lend` inside a macro"],
    );
    // A definition whose body imports the token's module under another
    // name, which the macro's invocation does not name at all.
    red(
        &edit(
            &src,
            "src/ordindex.rs",
            ord,
            &format!(
                "{ord}macro_rules! u {{\n    () => {{ use self::lend as mk; }};\n}}\nu!();\n#[allow(dead_code)]\npub(crate) fn hand() -> mk::Token {{\n    mk::Token::new()\n}}\n"
            ),
        ),
        &["`() => { use self::lend as mk; };` names `lent` or `lend` inside a macro"],
    );
    // The control: `lend_at` forwarded the same way is read as a call.
    red(
        &edit(
            &edit(
                &src,
                "src/blob.rs",
                bl,
                &format!(
                    "macro_rules! ty {{\n    ($m:ident) => {{\n        $m::Token\n    }};\n}}\nmacro_rules! mk {{\n    ($m:ident) => {{\n        $m::Token::new()\n    }};\n}}\n#[allow(dead_code)]\npub(crate) fn hand() -> ty!(lend) {{\n    mk!(lend)\n}}\n\n{bl}"
                ),
            ),
            "src/db/zfile.rs",
            zf,
            &format!(
                "{zf}#[allow(dead_code)]\nfn sneak(src: &MmapBytes) -> Option<&[u8]> {{\n    macro_rules! call {{\n        ($b:expr, $m:ident, $o:expr, $l:expr, $t:expr) => {{\n            $b.$m($o, $l, $t)\n        }};\n    }}\n    call!(src, lend_at, 0, 1, crate::blob::hand())\n}}\n"
            ),
        ),
        &[
            "src/db/zfile.rs (sneak): `call!(src, lend_at, 0, 1, crate::blob::hand())` calls `lend_at` outside src/blob.rs",
            "src/blob.rs (hand): `mk!(lend)` names `lent` or `lend` inside a macro",
        ],
    );
    // The token's module re-exported crate-wide under another name.
    red(
        &edit(
            &src,
            "src/blob.rs",
            bl,
            &format!("pub(crate) use self::lend as mk;\n\n{bl}"),
        ),
        &["src/blob.rs (<item>): `pub(crate) use self::lend as mk;`"],
    );
    // What the check leaves alone, and must: a `base()` of
    // no argument by type (another thing's: the address's takes a token),
    // a range whose end is a local closure named `lent`, and that closure
    // negated in parentheses, which is no macro's.
    let green = edit(
        &src,
        "src/db/pager.rs",
        "use crate::bytes::Paging;\n",
        "use crate::bytes::Paging;\n#[allow(dead_code)]\nfn others(n: usize) -> usize {\n    let lent = |k: usize| k + 1;\n    if !(lent(n) > 1) {\n        return 0;\n    }\n    let b = Others::base();\n    (0..lent(n)).count() + b\n}\n",
    );
    if let Err(e) = seal_check(&green) {
        misses
            .borrow_mut()
            .push(format!("refused what is not the seal's:\n{e}"));
    }
    // A call from outside the crate, its token made by unsafe code.
    let mut out = outside_sources(&root());
    seal_outside_check(&out).expect("the tests, the examples and the suite");
    let t = out.get_mut("tests/blob.rs").expect("tests/blob.rs");
    t.push_str(
        "\n#[allow(dead_code)]\nfn conjured(b: &MmapBytes) -> Option<&[u8]> {\n    b.lend_at(0, 1, unsafe { std::mem::zeroed() })\n}\n",
    );
    match seal_outside_check(&out) {
        Ok(()) => misses
            .borrow_mut()
            .push("passed the check: tests/blob.rs (conjured)".into()),
        Err(e) if !e.contains("tests/blob.rs (conjured)") => misses
            .borrow_mut()
            .push(format!("wanted `tests/blob.rs (conjured)` in:\n{e}")),
        Err(_) => {}
    }
    let misses = misses.into_inner();
    assert!(misses.is_empty(), "{}", misses.join("\n"));
}
