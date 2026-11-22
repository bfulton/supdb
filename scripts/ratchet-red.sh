#!/bin/sh
# The red input of the lists' check (scripts/base.sh, scripts/ratchet.sh):
# each case is run on a scratch repository and read by its exit status, so
# the check is trusted only once it has failed where it must and passed
# where it must.
#
#   sh scripts/ratchet-red.sh
#
# B. scripts/base.sh: an override, a pull request's base, a push's before
#    that are no commit of the clone, no origin/main, a shallow clone, a
#    first push whose HEAD~1 is not there, and a candidate with no
#    merge-base with HEAD each exit 2 and print no sha; an override, a
#    pull request's base, a push's before, a first push's HEAD~1 and
#    origin/main each print the merge-base, the override first, and empty
#    event values fall through to origin/main.
# L. scripts/ratchet.sh over copies of this tree's lists, committed as a
#    base: the tree against itself, a count lowered, a coupling moved
#    between scopes, a new entry point with a probe and a raise declared
#    exactly are green, the raise printed; a raise undeclared (a count, a
#    new key), declared with a wrong number, declared under a key that
#    differs, declared twice, a declaration for nothing, a list removed,
#    an unregistered text file under tests/data (any extension), a door
#    allow moved to another scope, a new libc exemption, a reason in place
#    of a probe, a registered list neither side has, a list the base's
#    registry held taken out of the tree's (to NOT_HELD or deleted), a list
#    that does not parse (a count, a region or a door kind that is none, on
#    one side or both), a registry its own reader cannot read, a base
#    whose registry cannot be read and a base that is no commit are red; a list new since the base
#    is green and says so, and so is a raise declared for such a list,
#    which is red against a base that has the list; a declaration at the
#    base is spent, and green with the raise kept or the line removed.
# G. the group: `sh scripts/check.sh lists` exits non-zero where this script
#    fails, and runs no check after it, and where scripts/ratchet.sh fails;
#    and `sh scripts/check.sh cost` takes its base from COST_BASE before the
#    event's values. Each case names the variables its verdict turns on, so
#    the environment it is run in decides none of them.
# RATCHET_RED_STEPS picks the steps (default `B L G`; G runs the group,
# which runs this script with the steps it names).
# The verdict is the last line: `ratchet-red: green` or `ratchet-red: RED`.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
R=$(mktemp -d "${TMPDIR:-/tmp}/supdb-ratchet-red.XXXXXX")
trap 'rm -rf "$R"' EXIT
mkdir "$R/out"
BAD=0
N=0

# expect NAME STATUS COMMAND...: the command's exit status is STATUS.
expect() {
  name=$1 want=$2
  shift 2
  N=$((N + 1))
  st=0
  "$@" >"$R/out/$name" 2>&1 || st=$?
  if [ "$st" != "$want" ]; then
    echo "ratchet-red: $name: exit $st, wanted $want"
    sed 's/^/    /' "$R/out/$name"
    BAD=$((BAD + 1))
  fi
}
# has NAME TEXT: the last case's output says TEXT.
has() {
  grep -qF -- "$2" "$R/out/$1" || {
    echo "ratchet-red: $1: the output does not say: $2"
    sed 's/^/    /' "$R/out/$1"
    BAD=$((BAD + 1))
  }
}
# last NAME LINE: the case's verdict line is LINE.
last() {
  [ "$(tail -n 1 "$R/out/$1")" = "$2" ] || {
    echo "ratchet-red: $1: the last line is not '$2'"
    BAD=$((BAD + 1))
  }
}
mk() { # a repository at $1 with one empty commit
  git init -q "$1"
  git -C "$1" -c user.name=r -c user.email=r@r commit -q --allow-empty -m "$2"
}
commit() { git -C "$1" add -A && git -C "$1" -c user.name=r -c user.email=r@r commit -q -m "$2"; }
none=1111111111111111111111111111111111111111
zeros=0000000000000000000000000000000000000000
# The event values CI sets are cleared first: each case says its own.
clean_env="-u SUPDB_BASE -u SUPDB_PR_BASE -u SUPDB_PUSH_BEFORE"
# shellcheck disable=SC2086
base() { (cd "$1" && shift && env $clean_env "$@" sh "$ROOT/scripts/base.sh"); }
want_sha() { # NAME SHA: the case printed SHA alone on stdout
  [ "$(grep -v '^base:' "$R/out/$1")" = "$2" ] || {
    echo "ratchet-red: $1: printed something other than $2"
    sed 's/^/    /' "$R/out/$1"
    BAD=$((BAD + 1))
  }
}

step_B() {
B=$R/b
mk "$B" one
git -C "$B" -c user.name=r -c user.email=r@r commit -q --allow-empty -m two
two=$(git -C "$B" rev-parse HEAD)
one=$(git -C "$B" rev-parse HEAD~1)
# No origin/main in this one yet.
expect b_no_origin 2 base "$B"
expect b_override_none 2 base "$B" SUPDB_BASE=$none
expect b_pr_none 2 base "$B" SUPDB_PR_BASE=$none
expect b_before_none 2 base "$B" SUPDB_PUSH_BEFORE=$none
expect b_empty_events 2 base "$B" SUPDB_PR_BASE= SUPDB_PUSH_BEFORE=
expect b_override 0 base "$B" SUPDB_BASE=HEAD~1 SUPDB_PR_BASE=$none
want_sha b_override "$one"
expect b_pr 0 base "$B" SUPDB_PR_BASE=$one SUPDB_PUSH_BEFORE=$none
want_sha b_pr "$one"
expect b_before 0 base "$B" SUPDB_PUSH_BEFORE=$one
want_sha b_before "$one"
expect b_first_push 0 base "$B" SUPDB_PUSH_BEFORE=$zeros
want_sha b_first_push "$one"
# Only the event's forty zeros are a first push: a short string of zeros
# is a before that names no commit.
expect b_short_zero 2 base "$B" SUPDB_PUSH_BEFORE=0
expect b_short_zeros 2 base "$B" SUPDB_PUSH_BEFORE=0000000
# origin/main on another line of history: the merge-base, not its tip.
git -C "$B" checkout -q -b side "$one"
git -C "$B" -c user.name=r -c user.email=r@r commit -q --allow-empty -m side
git -C "$B" update-ref refs/remotes/origin/main "$(git -C "$B" rev-parse HEAD)"
git -C "$B" checkout -q -
expect b_origin 0 base "$B"
want_sha b_origin "$one"
expect b_origin_empty_events 0 base "$B" SUPDB_PR_BASE= SUPDB_PUSH_BEFORE= SUPDB_BASE=
want_sha b_origin_empty_events "$one"
# A candidate with no merge-base with HEAD.
git -C "$B" checkout -q --orphan alone
git -C "$B" -c user.name=r -c user.email=r@r commit -q --allow-empty -m alone
alone=$(git -C "$B" rev-parse HEAD)
git -C "$B" checkout -q -f "$two"
expect b_unrelated 2 base "$B" SUPDB_BASE="$alone"
# A first push of a root commit has no HEAD~1.
B1=$R/b1
mk "$B1" root
expect b_first_push_root 2 base "$B1" SUPDB_PUSH_BEFORE=$zeros
# A shallow clone, whatever it is asked.
git clone -q --depth 1 "file://$B" "$R/shallow" 2>/dev/null
expect b_shallow 2 base "$R/shallow" SUPDB_BASE=HEAD
expect b_shallow_origin 2 base "$R/shallow"
has b_shallow "shallow"
for c in b_short_zero b_short_zeros b_no_origin b_override_none b_pr_none b_before_none b_unrelated b_first_push_root b_shallow; do
  want_sha "$c" ""
done

}

step_L() {
L=$R/l
mkdir -p "$L/tests/data" "$L/scripts"
git init -q "$L"
cp "$ROOT"/tests/data/couplings.txt "$ROOT"/tests/data/door.txt "$ROOT"/tests/data/cost-entries.txt \
  "$ROOT"/tests/data/crypt-v1.bin "$L/tests/data/"
cp "$ROOT/scripts/raises.txt" "$L/scripts/raises.txt"
# The check itself, so a case can change the tree's registry and run it.
cp "$ROOT/scripts/ratchet.py" "$ROOT/scripts/ratchet.sh" "$ROOT/scripts/base.sh" "$L/scripts/"
commit "$L" "c0: no door-libc.txt"
c0=$(git -C "$L" rev-parse HEAD)
cp "$ROOT/tests/data/door-libc.txt" "$L/tests/data/"
commit "$L" "c1: every list"
c1=$(git -C "$L" rev-parse HEAD)
at() { git -C "$L" checkout -q -f "$1" && git -C "$L" clean -fdq; }
# shellcheck disable=SC2086
check() { (cd "$L" && env $clean_env sh "$ROOT/scripts/ratchet.sh" "$@"); }
CP=$L/tests/data/couplings.txt
# The first data line of couplings.txt and its key's total.
first=$(grep -v '^#' "$CP" | grep -m1 .)
f1=$(printf '%s\n' "$first" | cut -f1)
t3=$(printf '%s\n' "$first" | cut -f3)
r4=$(printf '%s\n' "$first" | cut -f4)
ckey="$f1 $t3 $r4"
ctotal=$(awk -F'\t' -v f="$f1" -v t="$t3" -v r="$r4" '!/^#/ && $1==f && $3==t && $4==r {s+=$5} END{print s+0}' "$CP")
craise=$((ctotal + 1))
bump() { # FILE COLUMN DELTA: the first data line's COLUMN moved by DELTA
  awk -F'\t' -v OFS='\t' -v c="$2" -v d="$3" '!/^#/ && NF && !done {$c=$c+d; done=1} {print}' "$1" >"$1.t" && mv "$1.t" "$1"
}
decl() { printf '%s\t%s\t%s\t%s\t%s\n' "$@" >>"$L/scripts/raises.txt"; }
cpl=tests/data/couplings.txt

at "$c1"
expect l_self 0 check "$c1"
last l_self "ratchet: green"
expect l_unresolved 2 check
last l_unresolved "ratchet: RED"
expect l_base_none 2 check $none

at "$c1"; bump "$CP" 5 1
expect l_raise 1 check "$c1"
last l_raise "ratchet: RED"
has l_raise "$ckey: $ctotal -> $craise"

at "$c1"; bump "$CP" 5 1; decl $cpl "$ckey" "$ctotal" "$craise" "a fixture's raise"
expect l_declared 0 check "$c1"
last l_declared "ratchet: green"
has l_declared "ratchet: raise let through: $cpl $ckey $ctotal -> $craise: a fixture's raise"

at "$c1"; bump "$CP" 5 1; decl $cpl "$ckey" "$ctotal" "$((craise + 1))" "one too many"
expect l_wrong_to 1 check "$c1"
at "$c1"; bump "$CP" 5 1; decl $cpl "$ckey" "$((ctotal + 1))" "$((craise + 1))" "from moved"
expect l_wrong_from 1 check "$c1"
at "$c1"; bump "$CP" 5 1; decl $cpl "$f1 $t3 tes" "$ctotal" "$craise" "a key that differs"
expect l_wrong_key 1 check "$c1"
at "$c1"; bump "$CP" 5 1; decl $cpl "$ckey" "$ctotal" "$craise" "once"; decl $cpl "$ckey" "$ctotal" "$craise" "twice"
expect l_declared_twice 1 check "$c1"
at "$c1"; decl $cpl "$ckey" "$ctotal" "$craise" "nothing raised"
expect l_for_nothing 1 check "$c1"
has l_for_nothing "nothing observed"
at "$c1"; bump "$CP" 5 1; decl tests/data/door.txt "$ckey" "$ctotal" "$craise" "another list"
expect l_wrong_list 1 check "$c1"

at "$c1"; printf 'src/new.rs\tf\tpaged\tcode\t1\tstays\ta new key\n' >>"$CP"
expect l_new_key 1 check "$c1"
has l_new_key "src/new.rs paged code: 0 -> 1"
at "$c1"; bump "$CP" 5 -1
expect l_lowered 0 check "$c1"
last l_lowered "ratchet: green"
# A coupling moved between scopes: the line's scope renamed, the total kept.
at "$c1"; awk -F'\t' -v OFS='\t' '!/^#/ && NF && !done {$2=$2 "_moved"; done=1} {print}' "$CP" >"$CP.t" && mv "$CP.t" "$CP"
expect l_moved_scope 0 check "$c1"

at "$c1"; rm "$L/tests/data/door.txt"
expect l_removed 1 check "$c1"
has l_removed "tests/data/door.txt: the base has it and the tree does not"
at "$c1"; printf 'a\tb\n' >"$L/tests/data/new-list.txt"
expect l_unregistered 1 check "$c1"
has l_unregistered "tests/data/new-list.txt: a text file under tests/data"
at "$c1"; mkdir -p "$L/tests/data/sub"; printf 'x\n' >"$L/tests/data/sub/skips.tsv"
expect l_unregistered_tsv 1 check "$c1"
# The door: an allow moved to another scope is an allow where there was none.
at "$c1"; awk -F'\t' -v OFS='\t' '$1=="allow" && !done {$3=$3 "_moved"; done=1} {print}' "$L/tests/data/door.txt" >"$L/t" && mv "$L/t" "$L/tests/data/door.txt"
expect l_door_moved 1 check "$c1"
has l_door_moved "_moved: 0 -> 1"
# A raise in each file, region and kind a list names is read there: one
# line of each first seen is raised, and every raised key is named.
every() { # FILE COUNT-COLUMN KEY-COLUMNS COLUMN...: raise the first line of each value of each COLUMN; the keys raised to $R/keys
  f=$1 cnt=$2 kc=$3
  shift 3
  awk -F'\t' -v OFS='\t' -v c="$cnt" -v kc="$kc" -v cols="$*" -v keys="$R/keys" '
    BEGIN { n = split(cols, cs, " "); m = split(kc, ks, " ") }
    !/^#/ && NF {
      hit = 0
      for (i = 1; i <= n; i++) if (!((cs[i], $cs[i]) in seen)) { seen[cs[i], $cs[i]] = 1; hit = 1 }
      if (hit) {
        $c = $c + 1
        k = $ks[1]; for (j = 2; j <= m; j++) k = k " " $ks[j]
        print k >keys
      }
    }
    { print }' "$f" >"$f.t" && mv "$f.t" "$f"
}
held_every() { # NAME LIST: every key in $R/keys is named raised in LIST
  while IFS= read -r k; do has "$1" "$2: $k: "; done <"$R/keys"
  [ -s "$R/keys" ] || { echo "ratchet-red: $1: no key was raised"; BAD=$((BAD + 1)); }
}
at "$c1"; : >"$R/keys"; every "$CP" 5 "1 3 4" 1 4
expect l_raise_every_file 1 check "$c1"
held_every l_raise_every_file $cpl
at "$c1"; : >"$R/keys"; every "$L/tests/data/door.txt" 4 "1 2 3" 1 2
expect l_door_every_file 1 check "$c1"
held_every l_door_every_file tests/data/door.txt
at "$c1"; printf 'fopen\tsays it touches no file\n' >>"$L/tests/data/door-libc.txt"
expect l_libc_new 1 check "$c1"
has l_libc_new "tests/data/door-libc.txt: fopen: 0 -> 1"
# cost-entries: a probe given up for a reason is red; a new entry with a probe is not.
at "$c1"; awk -F'\t' -v OFS='\t' '!/^#/ && NF==2 && !done {$2="-"; $3="a reason now"; done=1} {print}' "$L/tests/data/cost-entries.txt" >"$L/t" && mv "$L/t" "$L/tests/data/cost-entries.txt"
expect l_entry_reason 1 check "$c1"
at "$c1"; printf 'Reader::red_new\tread\n' >>"$L/tests/data/cost-entries.txt"
expect l_entry_probe 0 check "$c1"
at "$c1"; printf 'Reader::red_new2\t-\tno probe\n' >>"$L/tests/data/cost-entries.txt"
expect l_entry_new_reason 1 check "$c1"
# A list new since the base, and one neither side has.
at "$c1"
expect l_new_list 0 check "$c0"
has l_new_list "tests/data/door-libc.txt: new since the base, nothing to hold yet"
# A raise declared for a list the base does not have yet holds nothing, as
# its list holds nothing: green, and said so. The same line against a base
# that has the list names no growth and is red, and a line for a path that
# is no list is refused as it always is, a list being new or not.
libc=tests/data/door-libc.txt
at "$c1"; decl $libc red_new_libc 0 1 "a list not at the base yet"
expect l_new_list_declared 0 check "$c0"
last l_new_list_declared "ratchet: green"
has l_new_list_declared "not held yet (its list is new since the base): $libc red_new_libc 0 -> 1"
expect l_new_list_declared_held 1 check "$c1"
has l_new_list_declared_held "$libc red_new_libc 0 -> 1, which matches no raise"
at "$c1"; decl tests/data/no-list.txt red_new_libc 0 1 "no list of the registry"
expect l_new_list_no_list 2 check "$c0"
has l_new_list_no_list "tests/data/no-list.txt is no list of the registry"
at "$c0"
expect l_neither 1 check "$c0"
has l_neither "neither the base nor the tree has it"
# A list the base's registry held, let go of in the tree's: taken out of
# REGISTRY and said in NOT_HELD to be no list, with a count raised that
# nothing then holds; and the same with NOT_HELD left alone. A base whose
# registry cannot be read is no base to hold the registry to.
# shellcheck disable=SC2086
own() { (cd "$L" && env $clean_env sh scripts/ratchet.sh "$@"); }
unregister() { # PATH NOT_HELD-REASON (empty: NOT_HELD left alone)
  python3 - "$L/scripts/ratchet.py" "$1" "$2" <<'PY'
import re, sys
p, path, why = sys.argv[1:]
s = open(p).read()
m = re.search(r'\n    "' + re.escape(path) + r'": \(\n.*?\n    \),', s, re.S)
assert m, "the entry is in the registry"
s = s.replace(m.group(0), "", 1)
if why:
    assert "NOT_HELD = {}" in s
    s = s.replace("NOT_HELD = {}", "NOT_HELD = {" + repr(path) + ": " + repr(why) + "}", 1)
open(p, "w").write(s)
PY
}
at "$c1"; unregister $cpl "no list now"; bump "$CP" 5 1
expect l_deregistered 1 own "$c1"
has l_deregistered "tests/data/couplings.txt: the base's registry (scripts/ratchet.py) holds it and the tree's does not"
at "$c1"; unregister tests/data/door.txt ""
expect l_deregistered_bare 1 own "$c1"
has l_deregistered_bare "tests/data/door.txt: the base's registry (scripts/ratchet.py) holds it and the tree's does not"
at "$c1"; printf 'x = 1\n' >"$L/scripts/ratchet.py"; commit "$L" "c5: a registry nobody can read"
c5=$(git -C "$L" rev-parse HEAD)
cp "$ROOT/scripts/ratchet.py" "$L/scripts/ratchet.py"
expect l_base_registry_unread 2 own "$c5"
has l_base_registry_unread "scripts/ratchet.py at the base has no REGISTRY the check can read"
# An entry written so the registry's reader misses it: the tree's own
# registry is read as the base's will be, or the check refuses.
at "$c1"; python3 - "$L/scripts/ratchet.py" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
m = re.search(r'\n    "tests/data/door-libc.txt": \(\n        (\w+),\n        (".*?"),\n    \),', s, re.S)
assert m, "the entry is in the registry"
s = s.replace(m.group(0), '\n    "tests/data/door-libc.txt": (' + m.group(1) + ", " + m.group(2) + "),", 1)
open(p, "w").write(s)
PY
expect l_own_unread 2 own "$c1"
has l_own_unread "the registry's reader (registered) does not read this file's REGISTRY as Python does"
at "$c1"
expect l_own_self 0 own "$c1"
# NOT_HELD: a text file said to be no list passes, a NOT_HELD entry for a
# file the tree lacks fails, and so does the file it was meant for.
not_held() { # PATH REASON
  python3 - "$L/scripts/ratchet.py" "$1" "$2" <<'PY'
import sys
p, path, why = sys.argv[1:]
s = open(p).read()
assert "NOT_HELD = {}" in s
open(p, "w").write(s.replace("NOT_HELD = {}", "NOT_HELD = {" + repr(path) + ": " + repr(why) + "}", 1))
PY
}
at "$c1"; printf 'a\tb\n' >"$L/tests/data/notes.txt"; not_held tests/data/notes.txt "a fixture's notes, no list"
expect l_not_held 0 own "$c1"
last l_not_held "ratchet: green"
at "$c1"; printf 'a\tb\n' >"$L/tests/data/notes.txt"; not_held tests/data/note.txt "a typo of the path"
expect l_not_held_typo 1 own "$c1"
has l_not_held_typo "tests/data/note.txt: NOT_HELD names a file the tree does not have"
has l_not_held_typo "tests/data/notes.txt: a text file under tests/data the registry"
# A list that does not parse.
at "$c1"; printf 'src/x.rs\tf\tpaged\tcode\tmany\tstays\twhy\n' >>"$CP"
expect l_unparsed 2 check "$c1"
# A field a list's reader holds to its values: a region typo, on both
# sides as on one, and a door kind that is none.
at "$c1"; printf 'src/x.rs\tf\tpaged\ttset\t1\tstays\twhy\n' >>"$CP"
expect l_bad_region 2 check "$c1"
has l_bad_region "a region that is neither code nor test"
at "$c1"; printf 'src/x.rs\tf\tpaged\ttset\t1\tstays\twhy\n' >>"$CP"
commit "$L" "c4: a region typo at the base"
c4=$(git -C "$L" rev-parse HEAD)
expect l_bad_region_both 2 check "$c4"
at "$c1"; printf 'allows\tsrc/x.rs\tf\t1\twhy\n' >>"$L/tests/data/door.txt"
expect l_bad_kind 2 check "$c1"
has l_bad_kind "a kind the door does not have"

# Spent: the raise and its declaration merged; a change after it is green
# with the line kept, the line removed, or the raise lowered again.
at "$c1"; bump "$CP" 5 1; decl $cpl "$ckey" "$ctotal" "$craise" "a fixture's raise"
commit "$L" "c2: a raise, declared"
c2=$(git -C "$L" rev-parse HEAD)
expect l_raise_on_branch 0 check "$c1"
expect l_spent 0 check "$c2"
has l_spent "spent (at the base): $cpl $ckey $ctotal -> $craise"
at "$c2"; grep -v "a fixture's raise" "$L/scripts/raises.txt" >"$L/t" && mv "$L/t" "$L/scripts/raises.txt"
expect l_spent_removed 0 check "$c2"
at "$c2"; bump "$CP" 5 -1
expect l_spent_lowered 0 check "$c2"
# The same raise again after it is spent needs a line of its own.
at "$c2"; bump "$CP" 5 -1; commit "$L" "c3: lowered"
c3=$(git -C "$L" rev-parse HEAD)
bump "$CP" 5 1
expect l_raised_again 1 check "$c3"
at "$c3"; bump "$CP" 5 1; decl $cpl "$ckey" "$ctotal" "$craise" "raised again"
expect l_raised_again_declared 0 check "$c3"

}

step_G() {
# G. The group: `check.sh lists` fails where ratchet-red.sh fails, without
# running the check after it, and fails where the check does.
st=0
RATCHET_RED_STEPS=no_such_step sh "$ROOT/scripts/check.sh" lists >"$R/out/g_red" 2>&1 || st=$?
N=$((N + 1))
if [ "$st" = 0 ] || ! grep -q 'ratchet-red: no step no_such_step' "$R/out/g_red"; then
  echo "ratchet-red: G: the group exited $st where ratchet-red.sh failed"
  BAD=$((BAD + 1))
fi
if grep -q 'ratchet: the lists at' "$R/out/g_red"; then
  echo "ratchet-red: G: the group ran the check after ratchet-red.sh failed"
  BAD=$((BAD + 1))
fi
st=0
RATCHET_RED_STEPS=B SUPDB_BASE=$none sh "$ROOT/scripts/check.sh" lists >"$R/out/g_check" 2>&1 || st=$?
N=$((N + 1))
if [ "$st" = 0 ] || ! grep -q "names '$none', which is no commit" "$R/out/g_check"; then
  echo "ratchet-red: G: the group exited $st where ratchet.sh failed"
  sed 's/^/    /' "$R/out/g_check"
  BAD=$((BAD + 1))
fi
# The cost group's base: COST_BASE, its override, before the event's
# values, through scripts/base.sh (cost-red's step A alone, and a probe
# that is none, so nothing is built).
st=0
want=$(git -C "$ROOT" rev-parse HEAD~1)
(cd "$ROOT" && env -u SUPDB_BASE -u COST_RED_DIR -u COST_DIR COST_RED_STEPS=A \
  COST_PROBES=no_such_probe COST_BASE=HEAD~1 SUPDB_PR_BASE=HEAD SUPDB_PUSH_BEFORE=HEAD \
  sh scripts/check.sh cost) >"$R/out/g_cost" 2>&1 || st=$?
N=$((N + 1))
if [ "$st" = 0 ] || ! grep -qx "cost: the base is $want" "$R/out/g_cost"; then
  echo "ratchet-red: G: the cost group's base is not COST_BASE's ($want), exit $st"
  sed 's/^/    /' "$R/out/g_cost"
  BAD=$((BAD + 1))
fi
}

STEPS=${RATCHET_RED_STEPS:-B L G}
for s in $STEPS; do
  case "$s" in
    B | L | G) ;;
    *)
      echo "ratchet-red: no step $s (B L G)"
      echo "ratchet-red: RED"
      exit 2
      ;;
  esac
done
for s in $STEPS; do "step_$s"; done

if [ "$BAD" -ne 0 ]; then
  echo "ratchet-red: $BAD of the cases' checks failed ($N cases)"
  echo "ratchet-red: RED"
  exit 1
fi
echo "ratchet-red: $N cases, each as it must be"
echo "ratchet-red: green"
