//! A small lexer of Rust source, for the tests that read `src/` as text.
//!
//! It answers, for each line of a file: its code (comments removed, string
//! and char literals kept as written), its comment text, whether the line
//! lies in test code (an item under a `cfg` that implies `test`, or a
//! `#[test]` fn), and the scope it lies in -- `Type::fn`, `fn`, `outer>inner`
//! for a fn nested in a fn, `Type::<item>` for a type's body or an impl's
//! header, `const NAME` and the like for a top-level item. It does not parse
//! Rust: it tracks the depth of braces, parentheses and brackets outside
//! literals, comments and attributes, and opens a scope at the first brace
//! after a keyword that names one. That is enough for this crate's source,
//! and `every_file_closes_at_depth_zero` holds it there.
//!
//! The counting rules every source test shares are this module's: a token
//! is counted in code, never in a comment; a string's contents are code,
//! since a name in a string couples as surely as one in a path. Each trap
//! met while writing it is a unit test below.

/// One line of a file, as the lexer reads it.
#[derive(Clone, Debug)]
pub struct Line {
    /// The line's code: comments removed, literals as written.
    pub code: String,
    /// The line's code with every literal's characters blanked to spaces,
    /// attributes kept: where a path or an item is read.
    pub flat: String,
    /// The line's comment text, doc comments included.
    pub comment: String,
    /// Inside test code.
    pub test: bool,
    /// The enclosing scope: the one open at the line's end, or at its
    /// start where its end is at the top level, so a fn written on one
    /// line is keyed by the scope around it.
    pub scope: String,
}

/// A character that `\w` matches.
pub fn is_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Per line: the code, the comment, and the code with every literal's
/// characters blanked to spaces (for depth and keyword matching).
fn split(src: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let s: Vec<char> = src.chars().collect();
    let n = s.len();
    let mut code = vec![String::new()];
    let mut comm = vec![String::new()];
    let mut flat = vec![String::new()];
    // Where each character goes: code (with `lit` saying it is a literal's
    // and so blanked in the flat text) or comment.
    let mut put = |ch: char, to_code: bool, lit: bool| {
        if ch == '\n' {
            code.push(String::new());
            comm.push(String::new());
            flat.push(String::new());
        } else if to_code {
            code.last_mut().unwrap().push(ch);
            flat.last_mut().unwrap().push(if lit { ' ' } else { ch });
        } else {
            comm.last_mut().unwrap().push(ch);
        }
    };
    let after_word = |i: usize| i > 0 && is_word(s[i - 1]);
    let mut i = 0;
    while i < n {
        let c = s[i];
        let next = if i + 1 < n { s[i + 1] } else { '\0' };
        if c == '/' && next == '/' {
            while i < n && s[i] != '\n' {
                put(s[i], false, false);
                i += 1;
            }
            continue;
        }
        if c == '/' && next == '*' {
            let mut depth = 0;
            while i < n {
                if s[i] == '/' && i + 1 < n && s[i + 1] == '*' {
                    depth += 1;
                    put('/', false, false);
                    put('*', false, false);
                    i += 2;
                    continue;
                }
                if s[i] == '*' && i + 1 < n && s[i + 1] == '/' {
                    depth -= 1;
                    put('*', false, false);
                    put('/', false, false);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                put(s[i], false, false);
                i += 1;
            }
            continue;
        }
        // Raw strings: r"..", r#".."#, br#".."#.
        if (c == 'r' || (c == 'b' && next == 'r')) && !after_word(i) {
            let mut j = i + if c == 'b' { 2 } else { 1 };
            let mut hashes = 0;
            while j < n && s[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < n && s[j] == '"' {
                // The end is a quote followed by as many hashes.
                let mut k = j + 1;
                let end = loop {
                    if k >= n {
                        break n;
                    }
                    if s[k] == '"' && (1..=hashes).all(|h| k + h < n && s[k + h] == '#') {
                        break k + 1 + hashes;
                    }
                    k += 1;
                };
                for &ch in &s[i..end] {
                    put(ch, true, true);
                }
                i = end;
                continue;
            }
        }
        if c == '"' || (c == 'b' && next == '"' && !after_word(i)) {
            if c == 'b' {
                put('b', true, true);
                i += 1;
            }
            put('"', true, true);
            i += 1;
            while i < n && s[i] != '"' {
                if s[i] == '\\' && i + 1 < n {
                    put(s[i], true, true);
                    put(s[i + 1], true, true);
                    i += 2;
                    continue;
                }
                put(s[i], true, true);
                i += 1;
            }
            if i < n {
                put('"', true, true);
                i += 1;
            }
            continue;
        }
        if c == '\'' {
            // A char literal, or a lifetime (left as code).
            let end = if next == '\\' {
                let from = if i + 2 < n && s[i + 2] == '\'' {
                    i + 3
                } else {
                    i + 2
                };
                (from..n).find(|&k| s[k] == '\'')
            } else if i + 2 < n && s[i + 2] == '\'' {
                Some(i + 2)
            } else {
                None
            };
            if let Some(e) = end {
                for &ch in &s[i..=e] {
                    put(ch, true, true);
                }
                i = e + 1;
                continue;
            }
        }
        put(c, true, false);
        i += 1;
    }
    (code, comm, flat)
}

/// The attributes on a flat line, as (start, end, text between the
/// brackets). An attribute opens at `#[` or `#![` and closes at its
/// matching `]`.
fn attributes(s: &[char]) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] == '#' {
            let open = if s.get(i + 1) == Some(&'[') {
                Some(i + 1)
            } else if s.get(i + 1) == Some(&'!') && s.get(i + 2) == Some(&'[') {
                Some(i + 2)
            } else {
                None
            };
            if let Some(o) = open {
                let mut d = 0;
                let close = (o..s.len()).find(|&k| {
                    match s[k] {
                        '[' => d += 1,
                        ']' => d -= 1,
                        _ => {}
                    }
                    d == 0
                });
                if let Some(c) = close {
                    out.push((i, c + 1, s[o + 1..c].iter().collect()));
                    i = c + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Whether a `cfg` predicate is true only in a test build: `test` itself,
/// `all(..)` with an argument that implies it, `any(..)` whose every
/// argument implies it. `not(test)` and `any(test, x)` do not: the item is
/// compiled where they hold and the test flag is off.
fn implies_test(p: &str) -> bool {
    let p = p.trim();
    if p == "test" {
        return true;
    }
    let args = |name: &str| -> Option<Vec<String>> {
        let rest = p.strip_prefix(name)?.trim_start();
        let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
        let mut parts = Vec::new();
        let mut d = 0;
        let mut cur = String::new();
        let mut in_str = false;
        for ch in inner.chars() {
            match ch {
                '"' => in_str = !in_str,
                '(' if !in_str => d += 1,
                ')' if !in_str => d -= 1,
                ',' if !in_str && d == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(ch);
        }
        if !cur.trim().is_empty() {
            parts.push(cur);
        }
        Some(parts)
    };
    if let Some(a) = args("all") {
        return a.iter().any(|x| implies_test(x));
    }
    if let Some(a) = args("any") {
        return !a.is_empty() && a.iter().all(|x| implies_test(x));
    }
    false
}

/// Whether an attribute's text (between its brackets) marks a test item.
fn is_test_attr(text: &str) -> bool {
    let t = text.trim();
    if t == "test" {
        return true;
    }
    match t.strip_prefix("cfg").map(str::trim_start) {
        Some(rest) if rest.starts_with('(') && rest.ends_with(')') => {
            implies_test(&rest[1..rest.len() - 1])
        }
        _ => false,
    }
}

/// The words of a line: maximal runs of word characters, with the char
/// index each starts at.
pub fn words(s: &[char]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if is_word(s[i]) {
            let st = i;
            while i < s.len() && is_word(s[i]) {
                i += 1;
            }
            out.push((st, s[st..i].iter().collect()));
        } else {
            i += 1;
        }
    }
    out
}

/// The word that follows index `i` past any whitespace, with where it ends.
fn word_after(s: &[char], mut i: usize, need_space: bool) -> Option<(String, usize)> {
    let st = i;
    while i < s.len() && s[i].is_whitespace() {
        i += 1;
    }
    if need_space && i == st {
        return None;
    }
    let w = i;
    while i < s.len() && is_word(s[i]) {
        i += 1;
    }
    (i > w).then(|| (s[w..i].iter().collect(), i))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Fn,
    Impl,
    Mod,
    Macro,
    Trait,
    Type,
    Block,
}

/// The type an impl header names: the one after `for`, or the one impl'd,
/// with its generics and path cut off.
fn impl_name(head: &str) -> String {
    let head = head.trim();
    let ty = match head.find(" for ") {
        Some(k) => &head[k + 5..],
        None => head,
    };
    let ty = ty.split('<').next().unwrap();
    let ty = match ty.find(" where") {
        Some(k) => &ty[..k],
        None => ty,
    };
    let ty = ty.trim().rsplit("::").next().unwrap().trim();
    if ty.is_empty() {
        "?".into()
    } else {
        ty.into()
    }
}

/// The scope-opening keywords of a flat line, in order: (index, kind, name).
fn openers(s: &[char]) -> Vec<(usize, Kind, String)> {
    let mut out = Vec::new();
    let ws = words(s);
    let lead = s.iter().take_while(|c| c.is_whitespace()).count();
    for (k, (at, w)) in ws.iter().enumerate() {
        let end = at + w.chars().count();
        match w.as_str() {
            "fn" | "mod" | "trait" => {
                if let Some((name, _)) = word_after(s, end, true) {
                    let kind = match w.as_str() {
                        "fn" => Kind::Fn,
                        "mod" => Kind::Mod,
                        _ => Kind::Trait,
                    };
                    out.push((*at, kind, name));
                }
            }
            "macro_rules" if s.get(end) == Some(&'!') => {
                if let Some((name, _)) = word_after(s, end + 1, false) {
                    out.push((*at, Kind::Macro, name));
                }
            }
            "impl" => {
                // Only an impl that begins the line (after `unsafe`) opens a
                // block: `-> impl Trait` in a signature does not.
                let first = ws.first().map(|x| x.0) == Some(lead);
                let at_start = (k == 0 && *at == lead) || (k == 1 && first && ws[0].1 == "unsafe");
                if !at_start {
                    continue;
                }
                let mut i = end;
                while i < s.len() && s[i].is_whitespace() {
                    i += 1;
                }
                if s.get(i) == Some(&'<') {
                    let mut d = 0;
                    while i < s.len() {
                        match s[i] {
                            '<' => d += 1,
                            '>' => d -= 1,
                            _ => {}
                        }
                        i += 1;
                        if d == 0 {
                            break;
                        }
                    }
                }
                let head: String = s[i..]
                    .iter()
                    .take_while(|&&c| c != '{' && c != ';')
                    .collect();
                out.push((0, Kind::Impl, impl_name(&head)));
            }
            "struct" | "enum" | "union" => {
                // At the line's start, after a visibility.
                let before: String = s[lead..*at].iter().collect();
                let before = before.trim();
                let vis = before.is_empty()
                    || before == "pub"
                    || (before.starts_with("pub") && before[3..].trim_start().starts_with('('));
                if vis {
                    if let Some((name, _)) = word_after(s, end, true) {
                        out.push((0, Kind::Type, name));
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

struct Open {
    kind: Kind,
    name: String,
    depth: i32,
    test: bool,
}

fn enclosing(stack: &[Open]) -> String {
    let mut f: Option<String> = None;
    let mut imp: Option<String> = None;
    for o in stack {
        match o.kind {
            Kind::Fn => {
                f = Some(match f {
                    None => o.name.clone(),
                    Some(x) => format!("{x}>{}", o.name),
                })
            }
            Kind::Impl | Kind::Trait | Kind::Type if f.is_none() => imp = Some(o.name.clone()),
            Kind::Macro if f.is_none() => f = Some(format!("{}!", o.name)),
            _ => {}
        }
    }
    match (f, imp) {
        (None, None) => {
            let mods: Vec<&str> = stack
                .iter()
                .filter(|o| o.kind == Kind::Mod)
                .map(|o| o.name.as_str())
                .collect();
            if mods.is_empty() {
                "<item>".into()
            } else {
                format!("<item:{}>", mods.join("::"))
            }
        }
        (None, Some(i)) => format!("{i}::<item>"),
        (Some(f), Some(i)) => format!("{i}::{f}"),
        (Some(f), None) => f,
    }
}

/// A top-level line's scope: the item it declares, as `const NAME`.
fn item_of(code: &str, fallback: &str) -> String {
    let c: Vec<char> = code.chars().collect();
    for (at, w) in words(&c) {
        if matches!(
            w.as_str(),
            "const" | "static" | "struct" | "enum" | "mod" | "trait" | "type"
        ) {
            if let Some((name, _)) = word_after(&c, at + w.len(), true) {
                return format!("{w} {name}");
            }
        }
    }
    fallback.into()
}

/// Every line of a file, read.
pub fn regions(src: &str) -> Vec<Line> {
    let (code, comm, flat) = split(src);
    let mut stack: Vec<Open> = Vec::new();
    let mut depth: i32 = 0;
    let mut pb: i32 = 0; // parentheses and brackets
    let mut pending: Option<(Kind, String)> = None;
    // A test attribute seen whose item has not opened, with the depths it
    // was seen at.
    let mut pending_test = false;
    let (mut pt_depth, mut pt_pb) = (0, 0);
    let mut out = Vec::with_capacity(flat.len());
    for (ln, line) in flat.iter().enumerate() {
        let mut s: Vec<char> = line.chars().collect();
        for (a, b, text) in attributes(&s) {
            if is_test_attr(&text) {
                pending_test = true;
                pt_depth = depth;
                pt_pb = pb;
            }
            for c in &mut s[a..b] {
                *c = ' ';
            }
        }
        let test_at_start = pending_test || stack.iter().any(|o| o.test);
        let ent_at_start = enclosing(&stack);
        let ops = openers(&s);
        let mut k = 0;
        for (pos, &ch) in s.iter().enumerate() {
            while k < ops.len() && ops[k].0 <= pos {
                pending = Some((ops[k].1, ops[k].2.clone()));
                k += 1;
            }
            match ch {
                '(' | '[' => pb += 1,
                ')' | ']' => pb = (pb - 1).max(0),
                '{' => {
                    depth += 1;
                    if let Some((kind, name)) = pending.take() {
                        stack.push(Open {
                            kind,
                            name,
                            depth,
                            test: pending_test,
                        });
                        pending_test = false;
                    } else if pending_test && depth == pt_depth + 1 && pb == pt_pb {
                        // `#[cfg(test)] if let .. {` and the like: a test block.
                        stack.push(Open {
                            kind: Kind::Block,
                            name: String::new(),
                            depth,
                            test: true,
                        });
                        pending_test = false;
                    } else {
                        stack.push(Open {
                            kind: Kind::Block,
                            name: String::new(),
                            depth,
                            test: false,
                        });
                    }
                }
                '}' => {
                    if stack.last().map(|o| o.depth) == Some(depth) {
                        stack.pop();
                    }
                    depth -= 1;
                    if pending_test && depth < pt_depth {
                        pending_test = false;
                    }
                }
                // A test field (of a struct or a struct literal) ends at
                // its comma.
                ',' if pending_test && pb == pt_pb && pending.is_none() && depth == pt_depth => {
                    pending_test = false;
                }
                ';' if pb == if pending_test { pt_pb } else { 0 } => {
                    if matches!(pending, Some((Kind::Fn | Kind::Mod | Kind::Type, _))) {
                        // A fn without a body, `mod x;`, `struct X;`.
                        pending = None;
                        if depth == 0
                            || matches!(
                                stack.last().map(|o| o.kind),
                                Some(Kind::Mod | Kind::Impl | Kind::Trait)
                            )
                        {
                            pending_test = false;
                        }
                    } else if pending_test && pending.is_none() && depth == pt_depth {
                        // A test `use`, `const` or statement ends at its `;`.
                        pending_test = false;
                    }
                }
                _ => {}
            }
        }
        let test = test_at_start || stack.iter().any(|o| o.test);
        let now = enclosing(&stack);
        let mut ent = if now.starts_with("<item") {
            ent_at_start
        } else {
            now.clone()
        };
        if let Some((Kind::Fn, name)) = &pending {
            // A signature still open names its fn.
            ent = if let Some(k) = now.find("::<item>") {
                format!("{}::{name}", &now[..k])
            } else if now.starts_with("<item") {
                name.clone()
            } else {
                format!("{now}>{name}")
            };
        }
        if ent.starts_with("<item") {
            ent = item_of(&code[ln], &ent);
        }
        out.push(Line {
            code: code[ln].clone(),
            flat: line.clone(),
            comment: comm[ln].clone(),
            test,
            scope: ent,
        });
    }
    out
}

/// Whether the braces, parentheses and brackets of a file's code close at
/// depth zero and never go below it, outside literals, comments and
/// attributes: the lexer's self-check, since a literal it misread leaves a
/// bracket open.
pub fn closes(src: &str) -> Result<(), String> {
    let (_, _, flat) = split(src);
    let (mut b, mut p, mut q) = (0i64, 0i64, 0i64);
    for (ln, line) in flat.iter().enumerate() {
        let mut s: Vec<char> = line.chars().collect();
        for (a, e, _) in attributes(&s) {
            for c in &mut s[a..e] {
                *c = ' ';
            }
        }
        for ch in s {
            match ch {
                '{' => b += 1,
                '}' => b -= 1,
                '(' => p += 1,
                ')' => p -= 1,
                '[' => q += 1,
                ']' => q -= 1,
                _ => {}
            }
            if b < 0 || p < 0 || q < 0 {
                return Err(format!("line {}: below depth zero", ln + 1));
            }
        }
    }
    if (b, p, q) == (0, 0, 0) {
        Ok(())
    } else {
        Err(format!(
            "ends at depth braces {b}, parentheses {p}, brackets {q}"
        ))
    }
}

/// Non-overlapping occurrences of `needle` in `hay`, ignoring ASCII case.
pub fn count_ci(hay: &str, needle: &str) -> usize {
    let h = hay.to_ascii_lowercase();
    h.matches(&needle.to_ascii_lowercase()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(src: &str) -> Vec<(String, bool, String)> {
        regions(src)
            .into_iter()
            .map(|l| (l.code.trim().to_string(), l.test, l.scope))
            .collect()
    }

    fn at<'a>(r: &'a [(String, bool, String)], needle: &str) -> &'a (String, bool, String) {
        r.iter()
            .find(|l| l.0.contains(needle))
            .unwrap_or_else(|| panic!("no line holds {needle}: {r:?}"))
    }

    #[test]
    fn comments_go_apart_and_strings_stay_code() {
        let r =
            regions("fn f() {\n    let a = \"vault // x\"; // vault\n    /* a\n b */ g();\n}\n");
        assert_eq!(r[1].code.trim(), "let a = \"vault // x\";");
        assert_eq!(r[1].comment.trim(), "// vault");
        assert_eq!(r[2].code.trim(), "");
        assert_eq!(r[3].code.trim(), "g();");
        assert!(r[3].comment.contains("b */"));
        assert_eq!(r[1].scope, "f");
    }

    #[test]
    fn the_flat_text_blanks_literals_and_keeps_attributes() {
        let r = regions("#[path = \"a.rs\"]\nfn f() { let c = '{'; g(\"super::x\"); } // y\n");
        assert_eq!(r[0].flat, "#[path =       ]");
        assert_eq!(r[1].flat, "fn f() { let c =    ; g(          ); } ");
        assert_eq!(r[1].code, "fn f() { let c = '{'; g(\"super::x\"); } ");
    }

    #[test]
    fn a_block_comment_nests() {
        let r = regions("fn f() {\n    /* a /* b */ cipher */ c();\n}\n");
        assert_eq!(r[1].code.trim(), "c();");
        assert!(r[1].comment.contains("cipher"));
    }

    #[test]
    fn a_test_field_ends_at_its_comma() {
        let r = read(
            "struct S {\n    #[cfg(test)]\n    a: u8,\n    b: u8,\n}\nimpl S {\n    fn attach(&self) {\n        x();\n    }\n}\n",
        );
        assert!(at(&r, "a: u8").1);
        assert!(!at(&r, "b: u8").1);
        assert!(!at(&r, "x();").1);
        assert_eq!(at(&r, "x();").2, "S::attach");
    }

    #[test]
    fn an_attribute_with_commas_is_one_attribute() {
        let r = read(
            "#[cfg(all(test, target_os = \"linux\"))]\nmod tests {\n    fn t() {\n        y();\n    }\n}\nfn z() {\n    w();\n}\n",
        );
        assert!(at(&r, "y();").1);
        assert_eq!(at(&r, "y();").2, "t");
        assert!(!at(&r, "w();").1);
    }

    #[test]
    fn an_array_type_in_a_signature_is_no_end_of_item() {
        let r = read(
            "fn wrap(k: [u8; 32]) -> [u8; 40] {\n    body();\n}\nfn next() {\n    other();\n}\n",
        );
        assert_eq!(at(&r, "body();").2, "wrap");
        assert_eq!(at(&r, "other();").2, "next");
    }

    #[test]
    fn a_string_across_lines_is_blanked_whole() {
        let src = "fn f() {\n    let s = \"a {\n b ( fn g() {\n\";\n    h();\n}\nfn k() {}\n";
        let r = read(src);
        assert_eq!(at(&r, "h();").2, "f");
        assert!(
            at(&r, "b ( fn g").0.contains("fn g"),
            "the string's text is code"
        );
        assert_eq!(at(&r, "b ( fn g").2, "f");
        assert!(closes(src).is_ok());
        let raw = "fn f() { let s = r#\"{ \" }\"#; }\n";
        assert!(closes(raw).is_ok());
    }

    #[test]
    fn a_char_literal_holding_a_bracket_is_no_bracket() {
        let src = "fn f<'a>(x: &'a str) -> bool {\n    let o = '{';\n    let c = '(';\n    let e = '\\'';\n    x == \"}\"\n}\nfn g() {\n    y();\n}\n";
        assert!(closes(src).is_ok());
        assert_eq!(at(&read(src), "y();").2, "g");
    }

    #[test]
    fn an_impl_in_a_return_type_opens_no_impl() {
        let r = read("impl Wal {\n    fn it(&self) -> impl Iterator<Item = u8> {\n        body();\n    }\n}\nimpl<T: Into<u8>> crate::x::Holder<T> for Box<T> {\n    fn h() {\n        hh();\n    }\n}\n");
        assert_eq!(at(&r, "body();").2, "Wal::it");
        assert_eq!(at(&r, "hh();").2, "Box::h");
        let r = read("impl Wal {\n    fn frames(\n        &self,\n    ) -> impl Iterator<Item = u8> + '_ {\n        inner();\n    }\n}\n");
        assert_eq!(
            at(&r, "inner();").2,
            "Wal::frames",
            "a return type on a line of its own"
        );
    }

    #[test]
    fn a_test_if_let_inside_a_fn_is_a_test_block() {
        let r = read(
            "fn f() {\n    #[cfg(test)]\n    if let Some(x) = y {\n        probe(x);\n    }\n    after();\n}\n",
        );
        assert!(at(&r, "probe(x);").1);
        assert!(!at(&r, "after();").1);
        assert_eq!(at(&r, "after();").2, "f");
    }

    #[test]
    fn a_test_field_of_a_literal_in_a_call_ends_at_its_comma() {
        let r = read(
            "fn f() {\n    make(S {\n        a: 1,\n        #[cfg(test)]\n        b: 2,\n        c: 3,\n    });\n    after();\n}\n",
        );
        assert!(at(&r, "b: 2").1);
        assert!(!at(&r, "c: 3").1);
        assert!(!at(&r, "after();").1);
    }

    #[test]
    fn only_a_cfg_that_implies_test_is_test() {
        let r = read(
            "#[cfg(any(test, debug_assertions))]\nfn a() { aa(); }\n#[cfg(not(test))]\nfn b() { bb(); }\n#[cfg(all(test, not(target_family = \"wasm\")))]\nfn c() { cc(); }\n#[cfg(any(test, all(test, unix)))]\nfn d() { dd(); }\n#[test]\nfn e() { ee(); }\n",
        );
        assert!(!at(&r, "aa();").1, "any(test, x) is compiled where x holds");
        assert!(!at(&r, "bb();").1);
        assert!(at(&r, "cc();").1);
        assert!(at(&r, "dd();").1);
        assert!(at(&r, "ee();").1);
    }

    #[test]
    fn a_fn_inside_a_closure_is_nested_in_the_fn() {
        let r = read("impl Db {\n    fn outer(&self) {\n        let c = |x| {\n            fn inner() {\n                deep();\n            }\n            inner()\n        };\n        tail();\n    }\n}\n");
        assert_eq!(at(&r, "deep();").2, "Db::outer>inner");
        assert_eq!(at(&r, "tail();").2, "Db::outer");
    }

    #[test]
    fn a_top_level_item_is_named_by_its_kind() {
        let r = read("const ATTN: u8 = 1;\npub struct Held {\n    v: u8,\n}\nmod m {\n    static S: u8 = 0;\n}\n");
        assert_eq!(at(&r, "ATTN").2, "const ATTN");
        assert_eq!(at(&r, "v: u8").2, "Held::<item>");
        assert_eq!(at(&r, "static S").2, "static S");
    }

    #[test]
    fn a_signature_across_lines_names_its_fn() {
        let r = read(
            "impl Db {\n    fn open(\n        dir: &Path,\n    ) -> u8 {\n        1\n    }\n}\n",
        );
        assert_eq!(at(&r, "dir: &Path").2, "Db::open");
    }

    #[test]
    fn a_one_line_fn_is_keyed_by_the_scope_around_it() {
        let r = read("impl Db {\n    fn tiny(&self) -> u8 { here() }\n}\n");
        assert_eq!(at(&r, "here()").2, "Db::<item>");
    }

    #[test]
    fn a_bracket_left_open_fails_the_self_check() {
        assert!(closes("fn f() { (\n}\n").is_err());
        assert!(closes("fn f() }{\n").is_err());
        assert!(closes("fn f() { let c = '{'; }\n").is_ok());
    }
}
