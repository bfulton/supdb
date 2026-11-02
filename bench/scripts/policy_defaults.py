#!/usr/bin/env python3
"""The suite's `supdb` arm is the engine's defaults.

`Policy` in `bench/src/engines.rs` says what an arm differs from `supdb` by,
and `with_policy` maps its fields onto `supdb::Options`. A Policy default
that is a value of its own -- `true`, `false`, a number -- is a second
definition of the engine's default, and the two drift the day the engine's
moves: the controller's drop of the raw pages a form serves and its builds
ahead at landing were both on in the engine and off in every arm, so every
measurement of the controller at scale was of a shape the engine does not
ship. A field that should follow the engine is an `Option`, `None` resolved
through `unwrap_or(supdb::Options::default().<field>)`, which this script
leaves alone; for every field mapped as the policy's value or its negation,
it holds the Policy default to the engine's.

Exit 0 with nothing to say, 1 naming each field that differs.
"""
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
ENG = (HERE / "src" / "engines.rs").read_text().split("\n")
DB = (HERE.parent / "src" / "db.rs").read_text().split("\n")


def block(lines, start):
    """The brace-balanced block opened at `start` (0-based line index)."""
    depth = 0
    out = []
    for j in range(start, len(lines)):
        out.append(lines[j])
        depth += lines[j].count("{") - lines[j].count("}")
        if depth <= 0 and j > start:
            break
    return "\n".join(out)


def find(lines, pat):
    for i, l in enumerate(lines):
        if re.search(pat, l):
            return i
    sys.exit(f"policy_defaults: no line matches {pat!r}")


FIELD = re.compile(r"^\s+([a-z_0-9]+): ([^,]+),\s*$", re.M)
policy = dict(FIELD.findall(block(ENG, find(ENG, r"^impl Default for Policy"))))
engine = dict(FIELD.findall(block(DB, find(DB, r"^impl Default for Options"))))
literal = block(ENG, find(ENG, r"let opts = supdb::Options \{"))
mapped = re.findall(r"^\s+([a-z_0-9]+): (!?)([a-z_0-9]+),\s*$", literal, re.M)
if len(policy) < 50 or len(engine) < 50 or len(mapped) < 50:
    sys.exit(
        f"policy_defaults: parsed {len(policy)} policy defaults, "
        f"{len(engine)} engine defaults, {len(mapped)} mapped fields; the layout moved"
    )


def value(v):
    v = v.strip()
    return {"true": True, "false": False}.get(v, v)


bad = []
compared = 0
for field, neg, var in mapped:
    if var not in policy or field not in engine:
        continue
    pv, ev = value(policy[var]), value(engine[field])
    if neg and isinstance(pv, bool):
        pv = not pv
    compared += 1
    if (pv != ev) if isinstance(ev, bool) else (str(pv) != str(ev)):
        bad.append(
            f"  Options::{field} <- Policy::{neg}{var}: the policy's default is "
            f"{policy[var].strip()}, the engine's {engine[field].strip()}"
        )
if bad:
    print("policy_defaults: the supdb arm differs from the engine's defaults:")
    print("\n".join(bad))
    print("pin the field with an Option resolved through unwrap_or(Options::default()...), or align it")
    sys.exit(1)
print(f"policy_defaults: {compared} fields follow the engine")
