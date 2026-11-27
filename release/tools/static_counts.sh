#!/bin/sh
# The static counts of a tree: one "name<TAB>value" line a quantity, for the
# tree it is run in. Nothing here is a timing. Without --build nothing reads
# the network; with it cargo fetches the crates the lockfiles pin where its
# cache does not hold them.
#
#   sh release/tools/static_counts.sh            # from the repository root
#   sh release/tools/static_counts.sh --build    # the same, then two builds
#
# Without an argument it uses git, awk, grep, wc and sort, and reads the
# files git tracks in the working tree. Two runs over one tree print the
# same bytes. `worktree_dirty_paths` says whether the working tree is the
# commit: at 0 every count is of `tree`. `worktree_dirty_paths_outside_release`
# says the same of everything outside release/: at 0 every count but the five
# that read release/ is of the tree `tree_less_release` fingerprints.
#
# What each quantity counts.
#
# commit, tree
#   `git rev-parse HEAD` and `git rev-parse HEAD^{tree}`. The tree hash is
#   the one to compare by: it survives a re-message of history.
# tree_less_release
#   A fingerprint of everything the tree holds outside release/: the hash
#   `git hash-object --stdin` gives the listing `git ls-tree -r HEAD` with
#   the release/ lines taken out. Of the lines below five read release/:
#   `worktree_dirty_paths`, `tracked_files`, `tracked_bytes`,
#   `tracked_files_release` and `tracked_bytes_release`. Two commits that
#   agree here agree on every other count, whatever was committed under
#   release/ between them.
# worktree_dirty_paths
#   Lines of `git status --porcelain`: tracked files changed and untracked
#   files not ignored.
# worktree_dirty_paths_outside_release
#   The same lines for paths outside release/ (`git status --porcelain --
#   . ':(exclude)release'`). The counts are of the working tree and
#   `tree_less_release` is of HEAD: this line at 0 is what ties the two,
#   whatever is changed or untracked under release/.
#
# options_fields, options_doc_hidden, options_experiment,
# options_doc_hidden_and_experiment
#   Fields of `pub struct Options`; those with `#[doc(hidden)]` above them;
#   those whose `///` comment holds the word EXPERIMENT; those with both. A
#   field is a line at four spaces of the form `name: ` or `pub name: `
#   between the struct's opening line and its closing brace. The struct is
#   read from the first file of src/, in path order, that declares it:
#   src/db.rs when this was written.
# fields:<Struct>, fields_doc_hidden:<Struct>
#   The same count for SegmentOptions (src/db.rs then), BlobOptions
#   (src/blob.rs), StoreCipher (src/db/vault.rs), EncryptOptions and
#   ReadOptions (src/crypt.rs). `absent` where no file declares the struct.
# doc_hidden_lines_src
#   Lines of src/**/*.rs that hold `#[doc(hidden)]`.
#
# unsafe_lines_src
#   Lines of src/**/*.rs that hold the word `unsafe`, code or comment.
# unsafe_blocks_src, unsafe_fn_src, unsafe_impl_src
#   Occurrences of `unsafe {`, of `unsafe fn` (an `unsafe extern "C" fn`
#   with it), and of `unsafe impl`. Textual: a mention in a comment counts.
# safety_comment_lines_src, safety_doc_sections_src
#   Lines that hold `SAFETY`, and lines that are a `# Safety` rustdoc heading.
#
# src_files, src_lines
#   Tracked src/**/*.rs files and their lines.
# src_lines_in_test_mods, src_lines_outside_test_mods, src_test_mods
#   A test module is an inline `mod name {` whose attributes, directly above
#   it, include a `#[cfg(...)]` that names `test` outside a `not(...)` and
#   has no `any(...)`: `#[cfg(test)]`, `#[cfg(all(test, ...))]`. Its lines
#   run from that attribute to its closing brace, both included. The closing
#   brace is the first later line that is the module's own indentation and
#   `}` and nothing else, which is where rustfmt puts it. Everything else is
#   outside, the test-only items that are not modules among them.
# src_test_split_disagreements, src_test_split_unclosed,
# src_test_split_unbalanced_files
#   The check on that split, each 0 where it holds. The same modules are
#   found a second way, by counting braces through a lexer that skips
#   strings, raw strings, character literals and comments; a module the two
#   ways do not start and end on the same lines is a disagreement. Unclosed:
#   a module with no closing line. Unbalanced: a file whose brace depth by
#   the lexer is not zero at its end.
# src_files_one_test_mod_to_eof, src_files_test_mods_elsewhere,
# src_files_no_test_mod
#   Files whose one test module ends on the file's last line; files with a
#   test module that ends earlier, or with several; files with none.
# src_test_mods_elsewhere:<file>, src_test_mod_lines:<file>
#   For each file of the second kind its test modules, and for each file
#   with any its lines inside them.
# src_test_mods_external, src_files_test_only,
# src_test_mods_external_unresolved
#   `mod name;` declarations under a test cfg; the files they name, each
#   counted whole as inside a test module, with every file under the
#   module's directory; the declarations no tracked file answers. A
#   declaration in `a/b.rs` names `a/b/name.rs` or `a/b/name/mod.rs`, and
#   one in `a/mod.rs` or `a/lib.rs` names `a/name.rs` or `a/name/mod.rs`. A
#   `#[path]` is not read.
# src_cfg_test_items_not_mods
#   Test cfg attributes outside test modules on an item that is not a
#   module (a function, a field, a statement). Their lines count as outside.
# src_test_attrs_outside_test_mods
#   `#[test]` lines outside every test module and test-only file found: 0
#   where the split has found every module that holds a test.
# src_lines:<file>, src_largest:<rank>:<file>
#   Lines of each file, and the ten largest in order.
# lines:<dir>, files:<dir>
#   Lines and tracked files: `.rs` alone under tests/, bench/src,
#   bench/examples, bench/tests and examples/; every tracked file under
#   web/, scripts/ and bench/scripts/.
#
# test_attrs:<dir>, ignore_attrs:<dir>
#   Lines that are a `#[test]` attribute, and lines that are `#[ignore...]`
#   or a `#[cfg_attr(..., ignore)]`, in the `.rs` files of src/, tests/,
#   bench/src and bench/tests. Textual: a test a macro makes is not counted,
#   which the --build count is for.
# test_files_tests_top, test_files_tests_top_with_tests
#   `.rs` files directly under tests/ (each is a test binary), and those of
#   them with a `#[test]`.
# rustdoc_code_blocks_src, rustdoc_text_blocks_src
#   Code fences in rustdoc comments of src/, counted as fence lines over
#   two, and those opened as ```text. The difference is the doctests.
#
# bench_arms_open, bench_arms_open_supdb, bench_arms_open_other
#   Distinct arm names `open` in bench/src/engines.rs dispatches on: the
#   string literals in pattern position of its `match arm`, which are the
#   lines at eight spaces that begin with `"` or `| "`, read up to `=>`. A
#   name under two cfg arms counts once. Those that begin `supdb`, and the
#   rest.
# bench_roster:<CONST>[:cfg(...)]
#   The length each roster constant declares (`pub const X: [&str; N]`),
#   with the cfg above it where there is one. The compiler holds the length
#   to the array.
# bench_roster_arms_distinct, bench_roster_arms_not_in_open
#   Distinct names over the roster macros, the roster constants' arguments
#   and the array constants; those of them `open` has no arm for (0).
#
# doc_bytes:<file>, doc_bytes_total
#   Bytes of README.md, CLAUDE.md, bench/CLAUDE.md, ROADMAP.md,
#   CONTRIBUTING.md, bench/DESIGN.md, web/README.md and each tracked file
#   under docs/ outside docs/figures/.
# rustdoc_lines_src, rustdoc_outer_lines_src, rustdoc_inner_lines_src
#   Lines of src/ that are a `///` or a `//!` comment; each kind.
#
# deps:<manifest>:<section>, dep_names:..., deps_optional:...
#   Entries of [dependencies], [dev-dependencies] and [build-dependencies]
#   in Cargo.toml and bench/Cargo.toml, a `[target.*.<section>]` table
#   counted with its section; their names; those marked `optional = true`.
# lock_packages:<lockfile>
#   `[[package]]` entries, the workspace's own package among them.
#
# workflows, workflow_jobs:<file>, workflow_jobs_total
#   Files under .github/workflows, and the keys directly under each one's
#   `jobs:`.
# bench_rows:<dir>, bench_rows_total
#   Tracked `.json` files in each directory under bench/runs.
# tracked_files, tracked_bytes, tracked_files_release, tracked_bytes_release
#   Blobs of HEAD's tree and the sum of their sizes (`git ls-tree -r -l`);
#   the part of each under release/.
#
# With --build, after the lines above:
#
# rustc
#   `rustc --version`: the sizes and the test binaries are that compiler's.
# wasm_bytes, wasm_gzip, floor_bytes, floor_gzip
#   The four sizes `web/build.sh` prints: the browser module and the empty
#   cdylib it is measured against, raw and under `gzip -9`. The repository
#   states no number these are held to (web/README.md, "Size").
# tests_built:<target>, tests_built_ignored:<target>
#   For each test binary `cargo test --profile checked --no-run` makes for
#   the root package, the tests `<binary> --list` prints and the ones
#   `<binary> --list --ignored` prints. No test is run.
# tests_built_binaries, tests_built_total, tests_built_ignored_total,
# tests_built_list_failures
#   Their count and sums, and the binaries whose listing failed (0).
#   Doctests are not in these: `cargo test --doc` compiles them as it runs
#   them. The suite under bench/ is not built here.
set -eu
LC_ALL=C
export LC_ALL

build=0
case "${1:-}" in
  "") ;;
  --build) build=1 ;;
  *)
    echo "usage: sh release/tools/static_counts.sh [--build]" >&2
    exit 2
    ;;
esac

top=$(git rev-parse --show-toplevel 2>/dev/null) || {
  echo "static_counts: run from the repository root (git finds no repository here)" >&2
  exit 2
}
if [ "$top" != "$(pwd -P)" ] || [ ! -f src/db.rs ]; then
  echo "static_counts: run from the repository root ($top)" >&2
  exit 2
fi
if git ls-files | grep -q '[[:space:]]'; then
  echo "static_counts: a tracked path holds whitespace, which the file lists here do not carry" >&2
  exit 2
fi

TAB=$(printf '\t')
out() { printf '%s\t%s\n' "$1" "$2"; }
# Tracked files under the given paths, and the Rust ones among them.
tracked() { git ls-files -- "$@" | sort; }
rs_under() { git ls-files -- "$@" | grep '\.rs$' | sort; }
# Lines of the files named; no file is no line.
nlines() { awk 'END { print NR }' "$@" </dev/null; }
nfiles() { printf '%s\n' "$@" | awk 'NF { n++ } END { print n + 0 }'; }
# Lines of the files named that match an extended regular expression.
nmatch() {
  pat=$1
  shift
  if [ $# -eq 0 ]; then
    echo 0
  else
    grep -hE -e "$pat" -- "$@" </dev/null | awk 'END { print NR }'
  fi
}
# Occurrences of an awk regular expression in the files named.
noccur() {
  pat=$1
  shift
  awk -v pat="$pat" '{ n += gsub(pat, "&") } END { print n + 0 }' "$@" </dev/null
}

out commit "$(git rev-parse HEAD)"
out tree "$(git rev-parse 'HEAD^{tree}')"
out tree_less_release "$(git ls-tree -r HEAD | grep -v "${TAB}release/" | git hash-object --stdin)"
out worktree_dirty_paths "$(git --no-optional-locks status --porcelain | awk 'END { print NR }')"
out worktree_dirty_paths_outside_release "$(git --no-optional-locks status --porcelain -- . ':(exclude)release' | awk 'END { print NR }')"

SRC=$(rs_under src)

# --- options ---------------------------------------------------------------
# Prints: fields, doc(hidden) fields, EXPERIMENT fields, fields with both.
struct_fields() {
  awk -v name="$2" '
    !on && !done && $0 ~ ("^pub struct " name "( |<|\\{|$)") { on = 1; next }
    on && /^}/ { on = 0; done = 1 }
    on && /^    \/\/\// { if ($0 ~ /EXPERIMENT/) ex = 1; next }
    on && /^    #\[doc\(hidden\)\]/ { hid = 1; next }
    on && /^    (pub(\([a-z]+\))? )?[a-z_][A-Za-z0-9_]*: / {
      n++; h += hid; e += ex; b += (hid && ex); hid = 0; ex = 0
    }
    END { print n + 0, h + 0, e + 0, b + 0 }
  ' "$1"
}
# The first file of src/ that declares the struct, or nothing.
struct_file() {
  # shellcheck disable=SC2086
  awk -v name="$1" '
    $0 ~ ("^pub struct " name "( |<|\\{|$)") { print FILENAME; exit }
  ' $SRC </dev/null
}
f=$(struct_file Options)
if [ -n "$f" ]; then
  # shellcheck disable=SC2046
  set -- $(struct_fields "$f" Options)
  out options_fields "$1"
  out options_doc_hidden "$2"
  out options_experiment "$3"
  out options_doc_hidden_and_experiment "$4"
else
  out options_fields absent
fi
for s in SegmentOptions BlobOptions StoreCipher EncryptOptions ReadOptions; do
  f=$(struct_file "$s")
  if [ -n "$f" ]; then
    # shellcheck disable=SC2046
    set -- $(struct_fields "$f" "$s")
    out "fields:$s" "$1"
    out "fields_doc_hidden:$s" "$2"
  else
    out "fields:$s" absent
  fi
done
# shellcheck disable=SC2086
out doc_hidden_lines_src "$(nmatch '#\[doc\(hidden\)\]' $SRC)"

# --- unsafe ----------------------------------------------------------------
# shellcheck disable=SC2086
{
  out unsafe_lines_src "$(nmatch 'unsafe' $SRC)"
  out unsafe_blocks_src "$(noccur 'unsafe [{]' $SRC)"
  out unsafe_fn_src "$(noccur 'unsafe (extern "[A-Za-z-]*" )?fn' $SRC)"
  out unsafe_impl_src "$(noccur 'unsafe impl' $SRC)"
  out safety_comment_lines_src "$(nmatch 'SAFETY' $SRC)"
  out safety_doc_sections_src "$(nmatch '^[[:space:]]*//[/!] # Safety' $SRC)"
}

# --- engine lines, and the split by test module ------------------------------
# shellcheck disable=SC2086
split=$(awk '
  function is_test_cfg(s) {
    return s ~ /^[ \t]*#\[cfg\(/ && s ~ /[(, ]test[,)]/ && s !~ /not\(test/ && s !~ /any\(/
  }
  function is_mod_open(s) { return s ~ /^[ \t]*(pub(\([a-z]+\))? )?mod [A-Za-z0-9_]+ [{][ \t]*$/ }
  function is_mod_empty(s) { return s ~ /^[ \t]*(pub(\([a-z]+\))? )?mod [A-Za-z0-9_]+ [{][}][ \t]*$/ }
  function is_mod_decl(s) { return s ~ /^[ \t]*(pub(\([a-z]+\))? )?mod [A-Za-z0-9_]+;/ }
  function is_between(s) { return s ~ /^[ \t]*(#\[|\/\/)/ }
  function indent_of(s) { match(s, /^[ \t]*/); return substr(s, 1, RLENGTH) }

  # The lexer: brace depth over code alone. State carried across lines:
  # cdepth (nested block comments), instr (1 a string, 2 a raw string of
  # `hashes` hashes), depth.
  function lex(s,    i, n, c, two, j, h, p) {
    if (cdepth == 0 && instr == 0 && index(s, dq) == 0 && index(s, sq) == 0 && index(s, "/*") == 0) {
      i = index(s, "//")
      if (i) s = substr(s, 1, i - 1)
      depth += gsub(/[{]/, "{", s) - gsub(/[}]/, "}", s)
      return
    }
    n = length(s)
    i = 1
    while (i <= n) {
      c = substr(s, i, 1)
      if (cdepth > 0) {
        two = substr(s, i, 2)
        if (two == "*/") { cdepth--; i += 2 }
        else if (two == "/*") { cdepth++; i += 2 }
        else i++
        continue
      }
      if (instr == 1) {
        if (c == "\\") i += 2
        else { if (c == dq) instr = 0; i++ }
        continue
      }
      if (instr == 2) {
        if (c == dq && substr(s, i + 1, hashes) == hashstr) { instr = 0; i += 1 + hashes }
        else i++
        continue
      }
      if (c == "/") {
        two = substr(s, i, 2)
        if (two == "//") break
        if (two == "/*") { cdepth = 1; i += 2; continue }
        i++
        continue
      }
      if (c == dq) {
        j = i - 1
        h = 0
        while (j >= 1 && substr(s, j, 1) == "#") { h++; j-- }
        if (j >= 1 && substr(s, j, 1) == "r") {
          p = (j > 1) ? substr(s, j - 1, 1) : ""
          if (p == "b" || p == "c") p = (j > 2) ? substr(s, j - 2, 1) : ""
          if (p !~ /[A-Za-z0-9_]/) {
            instr = 2; hashes = h; hashstr = substr("################################", 1, h)
            i++
            continue
          }
        }
        instr = 1
        i++
        continue
      }
      if (c == sq) {
        if (substr(s, i + 1, 1) == "\\") {
          j = i + 3
          while (j <= n && substr(s, j, 1) != sq) j++
          i = j + 1
          continue
        }
        if (substr(s, i + 2, 1) == sq) { i += 3; continue }
        i++
        continue
      }
      if (c == "{") depth++
      else if (c == "}") depth--
      i++
    }
  }

  function end_file(   k, da, db) {
    if (file == "") return
    if (a_in || b_in) unclosed++
    if (depth != 0 || cdepth != 0 || instr != 0) unbalanced++
    da = 0
    db = 0
    for (k in amod) if (!(k in bmod)) da++
    for (k in bmod) if (!(k in amod)) db++
    disagree += (da > db ? da : db)
    for (k in amod) delete amod[k]
    for (k in bmod) delete bmod[k]
    order[++nf] = file
    lines_of[file] = flines
    test_of[file] = ftest
    mods_of[file] = fmods
    stray_of[file] = fstray
    if (fmods == 0) shape[file] = "none"
    else if (fmods == 1 && flast == flines) shape[file] = "eof"
    else shape[file] = "elsewhere"
  }

  BEGIN { dq = "\""; sq = sprintf("%c", 39) }

  FNR == 1 {
    end_file()
    file = FILENAME
    flines = 0; ftest = 0; fmods = 0; flast = 0; fstray = 0
    a_pend = 0; a_in = 0
    b_pend = 0; b_in = 0
    depth = 0; cdepth = 0; instr = 0
  }

  {
    line = $0
    flines = FNR

    # The first way: by the closing line.
    was_in = a_in
    if (a_in) {
      ftest++
      if (line == a_closer) { a_in = 0; amod[a_start ":" FNR] = 1; fmods++; flast = FNR }
    } else if (a_pend && is_mod_open(line)) {
      a_in = 1; a_start = a_pstart; a_closer = indent_of(line) "}"
      ftest += FNR - a_pstart + 1
      a_pend = 0
    } else if (a_pend && is_mod_empty(line)) {
      amod[a_pstart ":" FNR] = 1; fmods++; flast = FNR
      ftest += FNR - a_pstart + 1
      a_pend = 0
    } else if (a_pend && is_mod_decl(line)) {
      name = line
      sub(/^[ \t]*(pub(\([a-z]+\))? )?mod /, "", name)
      sub(/;.*$/, "", name)
      ext_file[++external] = FILENAME
      ext_name[external] = name
      a_pend = 0
    } else if (is_test_cfg(line)) {
      if (!a_pend) a_pstart = FNR
      a_pend = 1
    } else if (a_pend && is_between(line)) {
      # More attributes, or a comment, between the cfg and its item.
    } else {
      if (a_pend) seams++
      a_pend = 0
    }
    if (!was_in && !a_in && line ~ /^[ \t]*#\[test\]/) fstray++

    # The second way: by brace depth, over code alone.
    if (cdepth == 0 && instr == 0 && !b_in) {
      if (b_pend && (is_mod_open(line) || is_mod_empty(line))) {
        b_in = 1; b_start = b_pstart; b_base = depth
        b_pend = 0
      } else if (is_test_cfg(line)) {
        if (!b_pend) b_pstart = FNR
        b_pend = 1
      } else if (b_pend && is_between(line)) {
      } else {
        b_pend = 0
      }
    }
    lex(line)
    if (b_in && depth == b_base) { b_in = 0; bmod[b_start ":" FNR] = 1 }
  }

  END {
    end_file()
    # The files the external test modules name are test code whole.
    for (e = 1; e <= external; e++) {
      dir = ext_file[e]
      if (dir ~ /(^|\/)(mod|lib|main)\.rs$/) sub(/\/?[^\/]*$/, "", dir)
      else sub(/\.rs$/, "", dir)
      stem = (dir == "" ? "" : dir "/") ext_name[e]
      found = 0
      for (i = 1; i <= nf; i++) {
        f = order[i]
        if (f == stem ".rs" || f == stem "/mod.rs" || index(f, stem "/") == 1) {
          found = 1
          if (shape[f] != "whole") { shape[f] = "whole"; test_of[f] = lines_of[f]; stray_of[f] = 0 }
        }
      }
      if (!found) unresolved++
    }
    for (i = 1; i <= nf; i++) {
      f = order[i]
      total += lines_of[f]
      test += test_of[f]
      stray += stray_of[f]
      if (shape[f] != "whole") mods += mods_of[f]
      count[shape[f]]++
    }
    printf "src_files\t%d\n", nf
    printf "src_lines\t%d\n", total
    printf "src_lines_in_test_mods\t%d\n", test
    printf "src_lines_outside_test_mods\t%d\n", total - test
    printf "src_test_mods\t%d\n", mods
    printf "src_test_split_disagreements\t%d\n", disagree
    printf "src_test_split_unclosed\t%d\n", unclosed
    printf "src_test_split_unbalanced_files\t%d\n", unbalanced
    printf "src_files_one_test_mod_to_eof\t%d\n", count["eof"]
    printf "src_files_test_mods_elsewhere\t%d\n", count["elsewhere"]
    printf "src_files_no_test_mod\t%d\n", count["none"]
    for (i = 1; i <= nf; i++)
      if (shape[order[i]] == "elsewhere")
        printf "src_test_mods_elsewhere:%s\t%d\n", order[i], mods_of[order[i]]
    for (i = 1; i <= nf; i++)
      if (test_of[order[i]] > 0)
        printf "src_test_mod_lines:%s\t%d\n", order[i], test_of[order[i]]
    printf "src_test_mods_external\t%d\n", external
    printf "src_files_test_only\t%d\n", count["whole"]
    printf "src_test_mods_external_unresolved\t%d\n", unresolved
    printf "src_cfg_test_items_not_mods\t%d\n", seams
    printf "src_test_attrs_outside_test_mods\t%d\n", stray
    for (i = 1; i <= nf; i++) printf "src_lines:%s\t%d\n", order[i], lines_of[order[i]]
  }
' $SRC </dev/null)
printf '%s\n' "$split"
printf '%s\n' "$split" | grep '^src_lines:' | sort -t "$TAB" -k2,2nr -k1,1 |
  awk -F '\t' 'NR <= 10 { sub(/^src_lines:/, "", $1); printf "src_largest:%02d:%s\t%s\n", NR, $1, $2 }'

for d in tests bench/src bench/examples bench/tests examples; do
  l=$(rs_under "$d")
  # shellcheck disable=SC2086
  out "lines:$d" "$(nlines $l)"
  # shellcheck disable=SC2086
  out "files:$d" "$(nfiles $l)"
done
for d in web scripts bench/scripts; do
  l=$(tracked "$d")
  # shellcheck disable=SC2086
  out "lines:$d" "$(nlines $l)"
  # shellcheck disable=SC2086
  out "files:$d" "$(nfiles $l)"
done

# --- tests, textual ----------------------------------------------------------
for d in src tests bench/src bench/tests; do
  l=$(rs_under "$d")
  # shellcheck disable=SC2086
  out "test_attrs:$d" "$(nmatch '^[[:space:]]*#\[test\]' $l)"
  # shellcheck disable=SC2086
  out "ignore_attrs:$d" "$(nmatch '^[[:space:]]*#\[(ignore[] =]|cfg_attr\(.*,[[:space:]]*ignore)' $l)"
done
top_tests=$(git ls-files -- tests | grep '^tests/[^/]*\.rs$' | sort)
# shellcheck disable=SC2086
out test_files_tests_top "$(nfiles $top_tests)"
# shellcheck disable=SC2086
out test_files_tests_top_with_tests "$(
  awk 'FNR == 1 { seen = 0 } !seen && /^[ \t]*#\[test\]/ { seen = 1; n++ } END { print n + 0 }' $top_tests </dev/null
)"
# shellcheck disable=SC2086
fences=$(nmatch '^[[:space:]]*//[/!] ?```' $SRC)
out rustdoc_code_blocks_src "$((fences / 2))"
# shellcheck disable=SC2086
out rustdoc_text_blocks_src "$(nmatch '^[[:space:]]*//[/!] ?```text' $SRC)"

# --- bench arms ----------------------------------------------------------------
if [ -f bench/src/engines.rs ]; then
  awk '
    function names(s, set,    name) {
      while (match(s, /"[^"]*"/)) {
        name = substr(s, RSTART + 1, RLENGTH - 2)
        s = substr(s, RSTART + RLENGTH)
        if (set == "open") {
          if (!(name in opened)) { opened[name] = 1; n++; if (name ~ /^supdb/) ns++ }
        } else if (name ~ /^[a-z0-9-]+$/ && !(name in rostered)) {
          rostered[name] = 1
          nr++
        }
      }
    }
    /^pub fn open\(/ { in_open = 1; prev = $0; next }
    in_open && /^}/ { in_open = 0 }
    in_open && /^        (\| )?"/ { s = $0; sub(/=>.*/, "", s); names(s, "open") }

    /^macro_rules! (ooc_)?roster / { in_macro = 1 }
    in_macro && /^}/ { in_macro = 0 }
    in_macro && $0 !~ /^[ \t]*\/\// { names($0, "roster") }

    /^pub const [A-Z_]+: \[&str; [0-9]+\] = / {
      name = $3; sub(/:$/, "", name)
      len = $5; sub(/\]$/, "", len)
      label = "bench_roster:" name
      if (prev ~ /^#\[cfg\(.*\)\]$/) { c = prev; sub(/^#\[/, "", c); sub(/\]$/, "", c); label = label ":" c }
      roster_label[++nl] = label
      roster_len[nl] = len
      s = $0; sub(/^[^=]*= /, "", s); names(s, "roster")
      if ($0 ~ /\[$/) in_array = 1
      prev = $0
      next
    }
    in_array && /^\];/ { in_array = 0 }
    in_array && $0 !~ /^[ \t]*\/\// { names($0, "roster") }
    { prev = $0 }
    END {
      printf "bench_arms_open\t%d\n", n
      printf "bench_arms_open_supdb\t%d\n", ns
      printf "bench_arms_open_other\t%d\n", n - ns
      for (i = 1; i <= nl; i++) printf "%s\t%s\n", roster_label[i], roster_len[i]
      for (name in rostered) if (!(name in opened)) missing++
      printf "bench_roster_arms_distinct\t%d\n", nr
      printf "bench_roster_arms_not_in_open\t%d\n", missing
    }
  ' bench/src/engines.rs
else
  out bench_arms_open absent
fi

# --- docs --------------------------------------------------------------------------
doc_total=0
for f in README.md CLAUDE.md bench/CLAUDE.md ROADMAP.md CONTRIBUTING.md bench/DESIGN.md \
  web/README.md $(git ls-files -- docs | grep -v '^docs/figures/' | sort); do
  if [ -f "$f" ]; then
    b=$(($(wc -c <"$f")))
    out "doc_bytes:$f" "$b"
    doc_total=$((doc_total + b))
  else
    out "doc_bytes:$f" absent
  fi
done
out doc_bytes_total "$doc_total"
# shellcheck disable=SC2086
{
  out rustdoc_lines_src "$(nmatch '^[[:space:]]*//(!|/([^/]|$))' $SRC)"
  out rustdoc_outer_lines_src "$(nmatch '^[[:space:]]*///([^/]|$)' $SRC)"
  out rustdoc_inner_lines_src "$(nmatch '^[[:space:]]*//!' $SRC)"
}

# --- dependencies --------------------------------------------------------------------
for m in Cargo.toml bench/Cargo.toml; do
  [ -f "$m" ] || continue
  awk -v m="$m" '
    function add(kind, name) {
      if ((kind, name) in have) return
      have[kind, name] = 1
      list[kind, ++cnt[kind]] = name
    }
    /^\[/ {
      kind = ""
      h = $0
      sub(/[ \t]*(#.*)?$/, "", h)
      if (h ~ /^\[(target\..*\.)?dependencies\]$/) kind = "dependencies"
      else if (h ~ /^\[(target\..*\.)?dev-dependencies\]$/) kind = "dev-dependencies"
      else if (h ~ /^\[(target\..*\.)?build-dependencies\]$/) kind = "build-dependencies"
      else if (h ~ /^\[(target\..*\.)?(dev-|build-)?dependencies\.[A-Za-z0-9_-]+\]$/) {
        name = h; sub(/\]$/, "", name); sub(/^.*dependencies\./, "", name)
        k = "dependencies"
        if (h ~ /dev-dependencies\.[A-Za-z0-9_-]+\]$/) k = "dev-dependencies"
        if (h ~ /build-dependencies\.[A-Za-z0-9_-]+\]$/) k = "build-dependencies"
        add(k, name)
      }
      next
    }
    kind != "" && /^[A-Za-z0-9_-]+[ \t]*[=.]/ {
      name = $0
      sub(/[ \t]*[=.].*$/, "", name)
      add(kind, name)
      if ($0 ~ /optional[ \t]*=[ \t]*true/) opt[kind]++
    }
    END {
      nk = split("dependencies dev-dependencies build-dependencies", kinds, " ")
      for (ki = 1; ki <= nk; ki++) {
        k = kinds[ki]
        n = cnt[k] + 0
        for (i = 1; i <= n; i++) a[i] = list[k, i]
        for (i = 2; i <= n; i++) {
          v = a[i]
          for (j = i - 1; j >= 1 && a[j] > v; j--) a[j + 1] = a[j]
          a[j + 1] = v
        }
        s = ""
        for (i = 1; i <= n; i++) s = s (i > 1 ? "," : "") a[i]
        printf "deps:%s:%s\t%d\n", m, k, n
        if (n) printf "dep_names:%s:%s\t%s\n", m, k, s
        if (opt[k]) printf "deps_optional:%s:%s\t%d\n", m, k, opt[k]
      }
    }
  ' "$m"
done
for l in Cargo.lock bench/Cargo.lock; do
  [ -f "$l" ] && out "lock_packages:$l" "$(nmatch '^\[\[package\]\]' "$l")"
done

# --- workflows ----------------------------------------------------------------------
wf=$(tracked .github/workflows)
# shellcheck disable=SC2086
out workflows "$(nfiles $wf)"
jobs_total=0
for f in $wf; do
  j=$(awk '
    /^jobs:/ { on = 1; next }
    on && /^[^ \t#]/ { on = 0 }
    on && /^  [A-Za-z0-9_-]+:/ { n++ }
    END { print n + 0 }
  ' "$f")
  out "workflow_jobs:${f##*/}" "$j"
  jobs_total=$((jobs_total + j))
done
out workflow_jobs_total "$jobs_total"

# --- bench/runs -----------------------------------------------------------------------
git ls-files -- bench/runs | grep '\.json$' | awk '
  { d = $0; sub(/\/[^\/]*$/, "", d); n[d]++; t++ }
  END { for (d in n) printf "bench_rows:%s\t%d\n", d, n[d]; printf "bench_rows_total\t%d\n", t }
' | sort -t "$TAB" -k1,1

# --- tracked ----------------------------------------------------------------------------
git ls-tree -r -l HEAD | awk -F '\t' '
  { split($1, m, " ") }
  m[2] == "blob" { n++; b += m[4]; if ($2 ~ /^release\//) { rn++; rb += m[4] } }
  END {
    printf "tracked_files\t%d\ntracked_bytes\t%d\n", n, b
    printf "tracked_files_release\t%d\ntracked_bytes_release\t%d\n", rn, rb
  }
'

[ "$build" = 1 ] || exit 0

# --- the builds: one cargo at a time, the default target directory ---------------------
out rustc "$(rustc --version)"

# web/build.sh builds the module and its floor and prints the four sizes.
wasm_out=$(sh web/build.sh) || {
  echo "static_counts: web/build.sh failed" >&2
  exit 1
}
printf '%s\n' "$wasm_out" | awk '/^(wasm|floor)_(bytes|gzip) [0-9]+$/ { printf "%s\t%s\n", $1, $2 }'

# The root package's test binaries. The first call builds them and fails in
# words where a build fails; the second finds everything built and prints
# what cargo made as JSON, one artifact a line.
cargo test --profile checked --no-run >&2
cargo test --profile checked --no-run --message-format=json 2>/dev/null | awk -v top="$top" '
  /"reason":"compiler-artifact"/ && /"test":true/ && /"executable":"/ {
    match($0, /"executable":"[^"]*"/)
    exe = substr($0, RSTART + 14, RLENGTH - 15)
    match($0, /"src_path":"[^"]*"/)
    src = substr($0, RSTART + 12, RLENGTH - 13)
    if (index(src, top "/") == 1) src = substr(src, length(top) + 2)
    printf "%s\t%s\n", src, exe
  }
' | sort | while IFS="$TAB" read -r src exe; do
  [ -n "$exe" ] || continue
  if listed=$("$exe" --list 2>/dev/null) && ignored=$("$exe" --list --ignored 2>/dev/null); then
    out "tests_built:$src" "$(printf '%s\n' "$listed" | awk '/: test$/ { n++ } END { print n + 0 }')"
    out "tests_built_ignored:$src" "$(printf '%s\n' "$ignored" | awk '/: test$/ { n++ } END { print n + 0 }')"
  else
    out "tests_built_list_failed:$src" 1
  fi
done | awk -F '\t' '
  { print }
  /^tests_built:/ { b++; t += $2 }
  /^tests_built_ignored:/ { i += $2 }
  /^tests_built_list_failed:/ { f++ }
  END {
    printf "tests_built_binaries\t%d\n", b
    printf "tests_built_total\t%d\n", t
    printf "tests_built_ignored_total\t%d\n", i
    printf "tests_built_list_failures\t%d\n", f
  }
'
