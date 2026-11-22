# T21 mutants

Runner: mut.py (each mutant an edit of tests/source.rs; `cargo test --profile checked --test source seal`; verdict appended to mut-results.txt as it came; the file put back from keep/final/tests/source.rs and checked by sha256 2a9b3a4d... after each, and at the end). Logs: mut-<id>.log.

| mutant | verdict | killed by (case, from the log) |
|---|---|---|
| M1 a word left out of SEAL_WORDS (OrdLendToken) | KILLED | spelled_another_way: `src/ordindex.rs (hand): OrdLendToken::new() names OrdLendToken` passed |
| M2 a scope removed from the allowed sites (Sec::get) | KILLED | the tree test (the_seals_tokens_are_made_only_where_they_are_spent) red, so the list is read; the other three red too |
| M3 the allowed call sites keyed by file alone | KILLED | made_another_way: `src/db/zfile.rs (another)` passed; spelled: `names lend_whole_map` |
| M3b the declarations keyed by file alone | KILLED | made_another_way: blob `(token)` hand-out and `(Sec::lend) let t = lend_seal::Token::new()` |
| M4 the constructor's at-its-call rule dropped | KILLED | made_another_way: zeroed / transmuted / bound-to-a-local token at Sec::lend |
| M5 the readers' trait given to every type (impl check takes any header in the file) | KILLED | made_another_way: `impl<T> whole_map_readers::Reader for (T,)` and the BaseAddressToken impl |
| M6 (own) a use's bound name not held | KILLED | spelled: `use lend_seal::Token as Tk;`, `use self::lend_seal as mk;` in ordindex <item> |
| M7 (own) a use's group not refused | KILLED | spelled: `pub(crate) use lend_seal::{Token as Tk, Token as OrdLendToken};` (added for it) |
| M8 (own) a call in test code at a listed scope's name | KILLED | made_another_way: zfile `#[cfg(test)] mod seal_red { fn file_facts }` (added for it) |
| M9 (own) a call word with no parentheses let through | KILLED | made_another_way: `let f = <B as Bytes>::lend_at;` at Sec::lend, spent with a zeroed token (the case's token made zeroed so only this check refuses it) |
| M10 (own) a constructor at a declaration let through | KILLED | spelled: `const TOKEN: BaseAddressToken = BaseAddressToken(());` in trait Paging (added for it) |
| M11 (own) the help left off the failure | KILLED | made_another_way: the help's words |
| M12 (own) the token module's shape not held | KILLED | made_another_way: derive, public field, widened constructor, second fn |

Extra, as the brief asks (a route the new check passes and the old refused): a new bodiless signature in trait Paging that names `BaseAddressToken` (e.g. `fn hand(&self) -> BaseAddressToken;`), since `Paging::<item>` is a declaration site for the word and the old check keyed that line by its exact text. Answered, not a hole: no token can be made for it outside src/bytes.rs (the field is private, the compiler refuses), and inside src/bytes.rs an implementor's fn is a scope no list names (`X::hand` names `BaseAddressToken` outside the sites), while a default body or an associated const default is refused by the constructor rule (M10's case). Likewise a second `use` of the read path's token at bytes.rs <item> under a re-export's name passes where the old exact text refused; the token's constructor is `pub(in crate::blob)`, so no new maker results.

## Repair round 1 (runner mut2.py, results mut-results-r1.txt, logs mut-r1-<id>.log; tests/source.rs put back from keep/final by sha256 after each; tree = keep/final at the end)

The check changed: a word at a declaration site must now stand in a `use` head, a listed impl's header, the signature of a call word's `fn`, or a token module / `struct BaseAddressToken` scope (held whole by steps 1-2); each SEAL_CALLS scope is exactly one `fn` definition in its file. The constructor-at-a-declaration rule (`rest` starting `(` or `::new` after `::Token`/`>`) is gone: under the item rule no token can be made where a declaration's word may stand, so it reached nothing; its case (an associated const in trait Paging) is now refused by the item rule.

Red cases added first, shown red on the round's starting check (r1-before.log, EXIT 101: six "passed the check"), green after (r1-after.log, EXIT 0):
- X1 `hand!(OrdLendToken);` (macro that defines `fn hand() -> $t { $t::new() }`) at ordindex <item>: passed before, refused now.
- X2 `cl!(OrdLendToken);` (macro that gives the token Clone) at ordindex <item>: passed before, refused now.
- X3 `give!(whole_map_readers::Reader);` at bytes.rs <item>: passed before, refused now.
- zeroed one-line hand-out `pub(crate) fn hand() -> OrdLendToken { unsafe { std::mem::zeroed() } }` at ordindex <item>: passed before, refused now.
- X4 a non-test `mod x { fn file_facts(..) { m.lend_whole_map(super::lend_seal::Token::new()) } }` in zfile: passed before (both checks), refused now ("2 definitions of `file_facts`, where a site of the seal is one").
- X5 `let _t: crate::bytes::BaseAddressToken = unsafe { std::mem::zeroed() };` in Paged::base_address's body: passed before, refused now.
- The two type aliases (`type Tk = lend_seal::Token;` / `type Tk = OrdLendToken;` before `fn hand() -> Tk { Tk::new() }`): NOT a hole -- already red on the round's starting check, since the lexer gives a top-level `type` line the scope `type Tk` (tests/lex item_of), which no row lists. Added as red cases all the same.
- a `#[cfg(test)]` block inside the listed Sec::lend calling lend_at: already red (l.test), added.
- `impl Bytes for Fake { fn lend_at(..) }` in zfile (the review's ask for E3): already red, added; kills E3.
- a further signature `fn hand(&self) -> BaseAddressToken;` in trait Paging (the limit the first round recorded): refused now, added.

| mutant | verdict | killed by |
|---|---|---|
| M1-M9, M11, M12 (rerun on the new check) | all KILLED | as before (mut-results-r1.txt) |
| E1 drop `|| rest.starts_with("::new")` | N/A | the rule is removed (see above) |
| E2 drop `rest.strip_prefix('>')` / `("::Token")` | N/A | removed with it |
| E3 the `fn` branch's `if !decl { say }` removed | KILLED | made_another_way: `src/db/zfile.rs (Fake::lend_at): ... names lend_at` |
| E4 `.trim_end_matches(',')` dropped | KILLED | the_seal_leaves_ordinary_code_alone: "a call rewrapped" (its token line is `lend_seal::Token::new(),`, rustfmt's trailing comma) |
| N1 the item condition deleted (refuses more) | KILLED | the tree test (mod/struct-less declarations refused), so the item spans are read |
| N1b the item condition made `false` (a declaration's word let stand anywhere in its scope) | KILLED | X1, X2, X3, the zeroed hand-out, X5, the Paging const (7 "passed the check") |
| N2 the listed scopes' definitions not counted | KILLED | made_another_way: X4 |
| N3 a `use` is no item a declaration may name a word in | KILLED | the tree test |
| N4 any fn's signature (not only a call's) admitted | KILLED | made_another_way: the zeroed hand-out (`fn hand() -> OrdLendToken {` signature) |
| N5 the module/struct scopes not taken as held whole | KILLED | the tree test |

Known limits after the round: a name a macro builds from pieces; code outside a listed file; a second `use` binding a re-export's name at a declaration site (e.g. `use crate::blob::lend_seal::Token as LendToken;` twice) passes, harmless since a use makes nothing and the constructor is confined by the compiler; scopes do not carry their module path, which the one-definition count stands in for.

## Repair round 2 (runner mut3.py; results mut-results-r2.txt (final set, on keep/r2-final 18239a70...), mut-results-r2a.txt (first pass of the final set, before R1/O2/R2b were answered); logs mut-r2-<id>.log; tests/source.rs put back by sha256 after each; tree = keep/r2-final at the end)

Code changed this round: `seal_check_with(src, calls)` (the calls' table given, so a case can hold how it is read); a call's token read from the row that matched the call's file and scope (`home`), not the word's first row; `made` read as the first word of the seal inside the call to its close (an `Option`, not a `Vec` and a length test); the defs count's `>{next}` arm removed; `seal_until` counts parentheses and not brackets; the header comment names the closure limit.

| mutant | verdict | answer |
|---|---|---|
| X6 a closure in ZFile::stride_bytes that calls lend_whole_map, taken as a fn value (run as a green probe) | SURVIVED (the check passes it, as the old one did) | Recorded as a limit, in the header comment and CLAUDE.md: a closure is its scope's as the lexer has it; what a listed scope returns is read in review. A nested `fn` is a scope of its own (`ZFile::stride_bytes>inner`) and is refused: red case added. |
| X7 the defs count's `>{next}` arm dropped | SURVIVED (on keep/r2-mid) | Arm removed: a nested fn's scope `outer>inner` is a key no listed scope looks up, so the arm counted entries nobody read. |
| X8 `made.len() != 1` -> `made.is_empty()` | SURVIVED (on keep/r2-mid) | Simplified: `made` is the first seal word inside the call to the close, which covers any second word. |
| X9 the item spans' impl arm stops at `{` only | KILLED | made_another_way: new case, `trait SealRed { fn q(&self) -> impl Sized; }` then `give!(OrdLendToken);` at bytes.rs <item> |
| X9b (own) the impl heads stop at `{` only | KILLED | the_seal_leaves_ordinary_code_alone: new green case, a trait whose signature returns `impl Sized` before the listed impl |
| X10 the `struct ` exemption dropped | KILLED | the tree test (and the other three), as expected: the exemption is read |
| R1 seal_until counts parentheses only | SURVIVED (first pass) | Brackets taken out: a `;`/`{` in brackets at parenthesis depth zero only ends a span early, leaving the words after it to their scope, so counting them let nothing through (doc on seal_until). The green case first written for it (an array parameter) was inside parentheses and could not reach it; kept as "a signature grown". |
| R2 the token from the word's first row | KILLED | made_another_way: two tables with a `lend_whole_map | BaseAddressToken(())` row before and after SEAL_CALLS; red on the round's starting check (r2-before.log) |
| R2b the token from the word's last row | SURVIVED (first pass, one table) -> KILLED | the same case with the row appended |
| R3 the address token's attributes: any `#[cfg(` | KILLED | made_another_way: `#[cfg(not(test))]` and `#[cfg_attr(test, derive(Default))]` on BaseAddressToken |
| R4 a declaration's word held by its line containing `use `/`impl ` | KILLED | made_another_way: `hand!(OrdLendToken); use std::fmt as _;` at ordindex <item>; also the tree and the green test |
| O1 (own) a call's home row of any call word | KILLED | made_another_way: new case, `map.lend_at(0, 1, lend_seal::Token::new())` in OrdIndex::over (a scope listed for lend_whole_map) |
| O2 (own) the token read from the last seal word inside the call | SURVIVED (first pass) -> KILLED | new case: `src.lend_at(*off, keep(lend_seal::Token::new()), lend_seal::Token::new())`, a second token handed away in an earlier argument |
| O3 (own) a test line at a home row let through | KILLED | made_another_way: the `#[cfg(test)]` block in Sec::lend and the test `fn file_facts` |
| M1-M7, M9, M11, M12, E3, E4, N1-N5, N1b (rerun on the new code) | all KILLED | as before (mut-results-r2.txt) |
