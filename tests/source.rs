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
//! lowered with it, so the file only goes down. A coupling moved from one
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
    let t = c("PagerGrace m.paged_resident(0) MmapBytes::paged(x) Walked::Held PartWalk::Walked Walked(3)");
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
    assert!(
        e.contains("src/bytes.rs path out of the encryption modules code: 1 > 0"),
        "{e}"
    );
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
