#!/bin/sh
# The wall's red inputs, as script steps: each copies src/ and tests/data to a
# scratch root, edits the copy, and runs the gate over it by SUPDB_WALL_ROOT.
# Each verdict is the gate's exit status.
B=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/G1-1
R=$B/redroot
cd /home/user/supdb || exit 2
export TMPDIR=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/G1-1
cargo test --profile checked --test source --no-run >/dev/null 2>&1 || { echo "BUILD FAILED"; exit 2; }
gate() { SUPDB_WALL_ROOT=$R cargo test -q --profile checked --test source the_forms_and_the_cipher_do_not_name_each_other >$B/red.out 2>&1; echo $?; }
fresh() { rm -rf "$R"; mkdir -p "$R/tests"; cp -r src "$R/src"; cp -r tests/data "$R/tests/data"; }
edit() { editf src/db/zfile.rs "$1" "$2"; }
editf() { python3 -I - "$R/$1" "$2" "$3" <<'PY'
import sys
p, fn, line = sys.argv[1:4]
s = open(p).read()
at = s.index('fn %s(' % fn); o = s.index('{\n', at) + 2
open(p, 'w').write(s[:o] + '    ' + line + '\n' + s[o:])
PY
}
fails=0
expect() { # name want got
  if [ "$2" = red ] && [ "$3" -ne 0 ]; then echo "$1: red (exit $3) as expected"; grep -m3 -E 'grew|lower|neither' $B/red.out | sed 's/^/    /';
  elif [ "$2" = green ] && [ "$3" -eq 0 ]; then echo "$1: green (exit 0) as expected";
  else echo "$1: WRONG (want $2, exit $3)"; fails=1; fi; }
fresh; expect "the tree as it is" green "$(gate)"
fresh; : > "$R/tests/data/couplings.txt"; expect "no couplings listed (the gate before its file)" red "$(gate)"
fresh; edit warm_file 'super::vault::kept(path);'; expect "vault::kept added as warm_file's first statement" red "$(gate)"
fresh; edit open_seg 'let _ = super::vault::kept(path);'; expect "a second vault::kept in open_seg" red "$(gate)"
fresh; edit warm_file '// super::vault::kept(path);'; expect "the same text as a comment" green "$(gate)"
fresh; echo 'pub fn idle() {}' > "$R/src/db/pin.rs"; expect "an unclassified src/db/pin.rs" red "$(gate)"
fresh; printf '\npub(crate) use db::vault::kept as keep_named;\n' >> "$R/src/lib.rs"; edit warm_file 'crate::keep_named(path);'; expect "a vault item re-exported at the root under a neutral name, used in warm_file" red "$(gate)"
fresh; edit warm_file 'let _: Option<crate::DirState> = None;'; expect "the root's DirState named in warm_file" red "$(gate)"
fresh; printf 'super::vault::kept(path);\n' > "$R/src/db/zz.in"; edit warm_file 'include!("zz.in");'; expect "a src/db/zz.in included into warm_file" red "$(gate)"
fresh; printf '\n#[macro_export]\nmacro_rules! kk { ($p:expr) => { $crate::db::vault::kept($p) } }\n' >> "$R/src/db/vault.rs"; edit warm_file 'crate::kk!(path);'; expect "an exported vault macro called in warm_file" red "$(gate)"
fresh; editf src/db.rs open_refusals 'let _ = Held::vault(h).counts();'; expect "Held::vault(h) in Db::open_refusals" red "$(gate)"
# Round 2: the encryption side naming the engine, and code from outside under cfg_attr.
fresh; printf '\nimpl super::Db {\n    pub(crate) fn wall_probe(&self, p: &Path) -> bool {\n        kept(p)\n    }\n}\n' >> "$R/src/db/vault.rs"; editf src/db.rs open_refusals 'let _ = self.wall_probe(Path::new("x"));'; expect "impl super::Db in the vault, called from Db::open_refusals" red "$(gate)"
fresh; printf '\n#[allow(unused_imports)]\nuse super::{Options, Shared};\n' >> "$R/src/db/vault.rs"; expect "use super::{Options, Shared} in the vault" red "$(gate)"
fresh; printf '\nimpl crate::bytes::MmapBytes {\n    pub(crate) fn wall_probe(&self) -> bool {\n        true\n    }\n}\n' >> "$R/src/db/pager.rs"; edit warm_file 'let _ = crate::bytes::MmapBytes::wall_probe;'; expect "the pager gives MmapBytes a method, named from warm_file" red "$(gate)"
fresh; printf '\n#[cfg_attr(all(), path = "../../elsewhere.rs")]\nmod elsewhere;\n' >> "$R/src/db.rs"; expect "a module's path under cfg_attr in db.rs" red "$(gate)"
fresh; printf '\n#[\n    path = "../../elsewhere.rs"\n]\nmod elsewhere;\n' >> "$R/src/db.rs"; expect "a module's path attribute across lines in db.rs" red "$(gate)"
fresh; edit warm_file 'let _ = super::vault:: kept(path);'; expect "vault:: kept, spaced, in warm_file" red "$(gate)"
fresh; edit warm_file 'let _ = super::vault::
        kept(path);'; expect "vault:: and kept on two lines in warm_file" red "$(gate)"
fresh; printf '\nimpl crate::bytes::MmapBytes {\n    pub(crate) fn wall_probe(&self) -> bool {\n        true\n    }\n}\n' >> "$R/src/db/zfile.rs"; expect "a form gives MmapBytes a method" red "$(gate)"
fresh; printf '\nmacro_rules! wall_probe {\n    () => {\n        Db::ord_name_for\n    };\n}\n' >> "$R/src/db/blocks.rs"; editf src/db/vault.rs kept 'let _ = wall_probe!();'; expect "a bare macro the vault does not define, called in kept" red "$(gate)"
fresh; editf src/db/vault.rs kept 'let _ = crate::Options::default();'; expect "crate::Options named in the vault" red "$(gate)"
fresh; editf src/db/vault.rs kept 'use super::*;'; expect "use super::* in the vault (the engine's module)" red "$(gate)"
fresh; editf src/db/pager.rs crowded 'let _ = super::vault::kept;'; expect "the pager naming the vault by path (both encryption side)" green "$(gate)"
fresh; printf '\nextern crate self as wall_probe;\n' >> "$R/src/lib.rs"; editf src/db/vault.rs kept 'let _ = wall_probe::Options::default();'; expect "the crate under a second name, used by the vault" red "$(gate)"
# Round 3: a seam type renamed or aliased by a form; the seam re-exporting the engine; a form macro shadowing a std one.
fresh; printf '\nuse crate::bytes::MmapBytes as WallM;\nimpl WallM {\n    pub(crate) fn wall_probe(&self) -> usize {\n        0\n    }\n}\n' >> "$R/src/db/zfile.rs"; expect "use crate::bytes::MmapBytes as WallM; impl WallM in zfile" red "$(gate)"
fresh; printf '\ntype WallM = crate::bytes::MmapBytes;\nimpl WallM {\n    pub(crate) fn wall_probe(&self) -> usize {\n        0\n    }\n}\n' >> "$R/src/db/zfile.rs"; expect "type WallM = crate::bytes::MmapBytes; impl WallM in zfile" red "$(gate)"
fresh; printf '\npub(crate) use crate::db::Options as Engine;\n' >> "$R/src/bytes.rs"; printf '\nfn wall_probe() -> crate::bytes::Engine {\n    crate::bytes::Engine::default()\n}\n' >> "$R/src/db/vault.rs"; expect "the seam re-exports Options as Engine, named by the vault" red "$(gate)"
fresh; python3 -I - "$R/src/db.rs" <<'PY2'
import sys
p = sys.argv[1]; s = open(p).read()
m = "macro_rules! matches { ($e:expr, $p:pat) => { { let _ = crate::db::Db::open_refusals; match $e { $p => true, _ => false } } }; }\n"
assert s.count("pub mod vault;") == 1
open(p, "w").write(s.replace("pub mod vault;", m + "pub mod vault;", 1))
PY2
expect "a form macro_rules! matches before pub mod vault; in db.rs" red "$(gate)"
rm -rf "$R" "$B/red.out"
[ $fails -eq 0 ] && echo "RED INPUTS: ALL AS EXPECTED" || { echo "RED INPUTS: MISMATCH"; exit 1; }
