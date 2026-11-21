#!/usr/bin/env python3
# T18's mutants: each applied to the tree, the killing command run, the
# verdict appended to mutants.results at once, and the file put back from
# keep/ and checked by its sum before the next.
import hashlib, os, subprocess, sys

ROOT = "/home/user/supdb"
B = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T18"
TMP = "/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/tmp/T18"
RED = ["sh", "scripts/ratchet-red.sh"]
CI = ["cargo", "test", "--profile", "checked", "--test", "source", "--", "ci_", "a_ci_step", "a_group_no", "a_workflow", "a_set_up", "every_file_under"]

M = [
    # base.sh
    ("B1_silent_fallback_no_origin", "scripts/base.sh",
     '    fail "this clone has no origin/main: git fetch origin main, or name the base in SUPDB_BASE"',
     '    cand=HEAD', RED),
    ("B2_shallow_unchecked", "scripts/base.sh",
     'if [ "$(git rev-parse --is-shallow-repository)" = true ]; then', 'if false; then', RED),
    ("B3_zeros_taken_as_before", "scripts/base.sh",
     '[ "$SUPDB_PUSH_BEFORE" != "$zeros" ]', 'true', RED),
    ("B4_no_merge_base", "scripts/base.sh",
     'mb=$(git merge-base HEAD "$sha") ||', 'mb=$sha ||', RED),
    ("B5_pr_before_override", "scripts/base.sh",
     'if [ -n "${SUPDB_BASE:-}" ]; then\n  cand=$SUPDB_BASE how="SUPDB_BASE (an explicit override)"\nelif [ -n "${SUPDB_PR_BASE:-}" ]; then\n  cand=$SUPDB_PR_BASE how="SUPDB_PR_BASE (a pull request\'s base)"',
     'if [ -n "${SUPDB_PR_BASE:-}" ]; then\n  cand=$SUPDB_PR_BASE how="SUPDB_PR_BASE (a pull request\'s base)"\nelif [ -n "${SUPDB_BASE:-}" ]; then\n  cand=$SUPDB_BASE how="SUPDB_BASE (an explicit override)"', RED),
    ("B6_unresolved_falls_to_head", "scripts/base.sh",
     'sha=$(git rev-parse --verify --quiet "$cand^{commit}") ||\n  fail',
     'sha=$(git rev-parse --verify --quiet "$cand^{commit}") || sha=$(git rev-parse HEAD) ||\n  fail', RED),
    # ratchet.py: readers and matching
    ("R1_growth_by_line_count", "scripts/ratchet.py",
     'out[f"{c[0]} {c[2]} {c[3]}"] += number(path, n, c[4])',
     'out[f"{c[0]} {c[2]} {c[3]}"] += 1', RED),
    ("R2_key_by_line_not_total", "scripts/ratchet.py",
     'out[f"{c[0]} {c[2]} {c[3]}"] += number(path, n, c[4])',
     'out[f"{c[0]} {c[1]} {c[2]} {c[3]}"] += number(path, n, c[4])', RED),
    ("R3_match_ignores_numbers", "scripts/ratchet.py",
     '        g = (d[0], d[1], d[2], d[3])\n        if g in growth and growth[g] is None:',
     '        g = next((x for x in growth if x[:2] == d[:2]), (d[0], d[1], d[2], d[3]))\n        if g in growth and growth[g] is None:', RED),
    ("R4_match_ignores_list", "scripts/ratchet.py",
     '        g = (d[0], d[1], d[2], d[3])\n        if g in growth and growth[g] is None:',
     '        g = next((x for x in growth if x[1:] == d[1:4]), (d[0], d[1], d[2], d[3]))\n        if g in growth and growth[g] is None:', RED),
    ("R5_match_key_prefix", "scripts/ratchet.py",
     '        g = (d[0], d[1], d[2], d[3])\n        if g in growth and growth[g] is None:',
     '        g = next((x for x in growth if x[0] == d[0] and x[1].startswith(d[1].rsplit(" ", 1)[0]) and x[2:] == d[2:4]), (d[0], d[1], d[2], d[3]))\n        if g in growth and growth[g] is None:', RED),
    ("R6_unregistered_rule_dropped", "scripts/ratchet.py",
     '        if f not in REGISTRY and f not in NOT_HELD:', '        if False:', RED),
    ("R7_unregistered_by_txt_only", "scripts/ratchet.py",
     '            out.append(os.path.relpath(p, root).replace(os.sep, "/"))',
     '            if p.endswith(".txt"):\n                out.append(os.path.relpath(p, root).replace(os.sep, "/"))', RED),
    ("R8_removed_list_passes", "scripts/ratchet.py",
     '            red.append(f"{path}: the base has it and the tree does not: a list that only goes down is not taken away")',
     '            pass', RED),
    ("R9_spent_not_subtracted", "scripts/ratchet.py",
     '    fresh = tree_d - base_d', '    fresh = tree_d', RED),
    ("R10_spent_by_numbers_alone", "scripts/ratchet.py",
     '    fresh = tree_d - base_d',
     '    fresh = Counter(d for d in tree_d.elements() if d[:4] not in {b[:4] for b in base_d})', RED),
    ("R11_duplicate_consumes_nothing", "scripts/ratchet.py",
     '        if g in growth and growth[g] is None:', '        if g in growth:', RED),
    ("R12_door_moved_offsets", "scripts/ratchet.py",
     '        k = f"{c[0]} {c[1]} {c[2]}"\n', '        k = f"{c[0]} {c[1]}"\n', RED),
    ("R13_cost_reason_not_exempt", "scripts/ratchet.py",
     '            out[c[0]] = 1\n        else:\n            out[c[0]] = 0',
     '            out[c[0]] = 0\n        else:\n            out[c[0]] = 0', RED),
    ("R14_neither_side_passes", "scripts/ratchet.py",
     '            red.append(f"{path}: registered, and neither the base nor the tree has it")',
     '            pass', RED),
    ("S1_ratchet_sh_swallows", "scripts/ratchet.sh",
     'python3 "$here/ratchet.py" "$root" "$base"', 'python3 "$here/ratchet.py" "$root" "$base" || true', RED),
    ("S2_ratchet_sh_base_falls_back", "scripts/ratchet.sh",
     'base=$(sh "$here/base.sh") || { echo "ratchet: no base (above)"; echo "ratchet: RED"; exit 2; }',
     'base=$(sh "$here/base.sh") || base=HEAD', RED),
    ("G1_group_swallows_red", "scripts/check.sh",
     '      sh scripts/ratchet-red.sh || exit 1', '      sh scripts/ratchet-red.sh || true', RED),
    ("G2_group_swallows_check", "scripts/check.sh",
     '      sh scripts/ratchet.sh || exit 1', '      sh scripts/ratchet.sh || true', RED),
    # The YAML test.
    ("Y1_group_nobody_runs_passes", "tests/source.rs",
     '        if !ran.contains(g) && !said {', '        if false && !ran.contains(g) && !said {', CI),
    ("Y2_setup_prefix_match", "tests/source.rs",
     '        if CI_SETUP.iter().any(|(s, _)| s == cmd) {', '        if CI_SETUP.iter().any(|(s, _)| cmd.starts_with(s)) {', CI),
    ("Y3_apt_any_tail", "tests/source.rs",
     '            if pkgs.split_whitespace().count() > 0 && pkgs.split_whitespace().all(name) {',
     '            if pkgs.split_whitespace().count() > 0 {', CI),
    ("Y4_check_call_any_words", "tests/source.rs",
     '                if known.contains(g) {', '                if true {', CI),
    ("Y5_block_first_line_only", "tests/source.rs",
     '                if !b.trim().is_empty() {\n                    body.push((i + 1, b.trim().to_string()));\n                }',
     '                if !b.trim().is_empty() && body.is_empty() {\n                    body.push((i + 1, b.trim().to_string()));\n                }', CI),
    ("Y6_uses_unchecked", "tests/source.rs",
     '        if !CI_ACTIONS.iter().any(|(x, _)| x == a) {', '        if false {', CI),
    ("Y7_shell_ignored", "tests/source.rs",
     '        } else if key == "shell" {', '        } else if key == "shell" && false {', CI),
    ("Y8_flow_style_ignored", "tests/source.rs",
     "        if v.starts_with(['[', '{']) && !FLOW.contains(&key) {", "        if false {", CI),
    ("Y9_check_prefix_any", "tests/source.rs",
     '        if let Some(rest) = cmd.strip_prefix("sh scripts/check.sh ") {',
     '        if let Some(rest) = cmd.find("sh scripts/check.sh ").map(|i| &cmd[i + 20..]) {', CI),
    ("Y10_setup_hides_a_check", "tests/source.rs",
     '        "installs libclang for bindgen and names it to the build",\n    ),\n',
     '        "installs libclang for bindgen and names it to the build",\n    ),\n    ("cargo test --profile checked --lib", "a check, hidden"),\n', CI),
    ("Y11_said_and_ran_passes", "tests/source.rs",
     '        if ran.contains(g) && said {', '        if false {', CI),
    ("Y12_stale_name_passes", "tests/source.rs",
     '        if !known.contains(*n) {', '        if false {', CI),
    ("Y13_no_reason_passes", "tests/source.rs",
     '        if why.trim().is_empty() {', '        if false {', CI),
    # Round 1: every key of the workflow, the labels, the registry.
    ("Y14_job_if_passes", "tests/source.rs",
     [('                if key == "if" {', '                if false {'),
      ('const JOB: &[&str] = &["strategy",', 'const JOB: &[&str] = &["if", "strategy",')], CI),
    ("Y15_step_if_unchecked", "tests/source.rs",
     '            if !CI_IF.iter().any(|(x, a, _)| x == c && *a == action) {', '            if false {', CI),
    ("Y16_cond_still_counts", "tests/source.rs",
     '        let counts = st.cond.is_none() && !cond_jobs.contains(&st.job);', '        let counts = true;', CI),
    ("Y17_continue_on_error_known", "tests/source.rs",
     [('        if key == "continue-on-error" {', '        if false {'),
      ('const JOB: &[&str] = &["strategy",', 'const JOB: &[&str] = &["continue-on-error", "strategy",'),
      ('const STEP: &[&str] = &["name",', 'const STEP: &[&str] = &["continue-on-error", "name",')], CI),
    ("Y18_env_by_name_only", "tests/source.rs",
     '                if !CI_ENV.iter().any(|(n, x, _)| *n == key && *x == v) {',
     '                if !CI_ENV.iter().any(|(n, _, _)| *n == key) {', CI),
    ("Y19_env_unchecked", "tests/source.rs",
     '                if !CI_ENV.iter().any(|(n, x, _)| *n == key && *x == v) {', '                if false {', CI),
    ("Y20_with_unchecked", "tests/source.rs",
     '            if !CI_WITH.iter().any(|(a, x, _)| *a == action && x == k) {', '            if false {', CI),
    ("Y21_with_any_action", "tests/source.rs",
     '            if !CI_WITH.iter().any(|(a, x, _)| *a == action && x == k) {',
     '            if !CI_WITH.iter().any(|(_, x, _)| x == k) {', CI),
    ("Y22_if_any_action", "tests/source.rs",
     '            if !CI_IF.iter().any(|(x, a, _)| x == c && *a == action) {',
     '            if !CI_IF.iter().any(|(x, _, _)| x == c) {', CI),
    ("Y23_anchor_ignored", "tests/source.rs",
     "        if v.starts_with(['&', '*']) {", "        if false {", CI),
    ("Y24_merge_key_read", "tests/source.rs",
     [("        if v.starts_with(['&', '*']) {", "        if false {"),
      ('.all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))', '.all(|c| c.is_ascii_alphanumeric() || "_-<".contains(c))'),
      ('const STEP: &[&str] = &["name",', 'const STEP: &[&str] = &["<<", "name",')], CI),
    ("Y25_unknown_key_passes", "tests/source.rs",
     '        } else if !known {', '        } else if false {', CI),
    ("Y26_run_and_uses_passes", "tests/source.rs",
     '        if st.uses.is_some() && !st.runs.is_empty() {', '        if false {', CI),
    ("Y27_item_outside_steps", "tests/source.rs",
     '            if !in_steps {', '            if false {', CI),
    ("Y28_key_before_step_attaches", "tests/source.rs",
     '        let step = steps.last_mut().filter(|_| open && in_steps);',
     '        let step = steps.last_mut().filter(|_| in_steps);', CI),
    ("Y29_label_indent_ignored", "tests/source.rs",
     '        if *at.get_or_insert(ind) != ind {', '        if false {', CI),
    ("Y30_label_first_alternative", "tests/source.rs",
     "g.split('|').map(|n| n.trim().to_string()).collect();",
     "g.split('|').take(1).map(|n| n.trim().to_string()).collect();", CI),
    ("Y31_label_bar_unread", "tests/source.rs",
     "g.split('|').map(|n| n.trim().to_string()).collect();",
     "g.split('#').map(|n| n.trim().to_string()).collect();", CI),
    ("R15_deregistration_unchecked", "scripts/ratchet.py",
     '        for p in sorted(registered(held, "the base") - set(REGISTRY)):', '        for p in []:', RED),
    ("R16_self_read_unchecked", "scripts/ratchet.py",
     '        if registered(h.read(), "the tree") != set(REGISTRY):', '        if False:', RED),
    ("R17_unread_base_registry_empty", "scripts/ratchet.py",
     '    if not m:\n        raise', '    if not m:\n        return set()\n        raise', RED),
    ("R18_base_registry_from_tree", "scripts/ratchet.py",
     '    held = at_base(root, mb, REGISTRY_FILE)', '    held = in_tree(root, REGISTRY_FILE)', RED),
    # Round 2: keys by path, triggers, nested cases, and the reviewers' extras.
    ("X1_inputs_by_parent_name", "tests/source.rs",
     '            ["on", "workflow_dispatch", "inputs"] => true,', '            [.., "inputs"] => true,', CI),
    ("X2_first_esac_ends", "tests/source.rs",
     '            (false, true) => depth -= 1,', '            (false, true) => depth = 0,', CI),
    ("X3_nested_labels_read", "tests/source.rs",
     '        if at_depth[i] != 1 {', '        if false {', CI),
    ("X4_odd_case_unchecked", "tests/source.rs",
     '        if words.iter().any(|w| *w == "case" || *w == "esac")', '        if false && words.iter().any(|w| *w == "case" || *w == "esac")', CI),
    ("X5_missing_trigger_passes", "tests/source.rs",
     '        if !got.contains(&(k.to_string(), v.to_string())) {', '        if false && !got.contains(&(k.to_string(), v.to_string())) {', CI),
    ("X6_trigger_value_unread", "tests/source.rs",
     [('        if !on.iter().any(|(x, y, _)| x == k && y == v) {', '        if !on.iter().any(|(x, _, _)| x == k) {'),
      ('        if !got.contains(&(k.to_string(), v.to_string())) {', '        if !got.iter().any(|(x, _)| x == k) {')], CI),
    ("X7_triggers_uncollected", "tests/source.rs",
     '            triggers.push((ln, at.join("."), v.to_string()));', '            triggers.push((ln, at.join("."), v.to_string()));\n            triggers.clear();', CI),
    ("X8_zeros_branch_deleted", "scripts/base.sh",
     'elif [ -n "${SUPDB_PUSH_BEFORE:-}" ]; then\n  cand=HEAD~1 how="HEAD~1 (a push that made its branch or tag has no before)"\n', '', RED),
    ("X9_first_push_head", "scripts/base.sh",
     '  cand=HEAD~1 how=', '  cand=HEAD how=', RED),
    ("X10_not_held_by_values", "scripts/ratchet.py",
     '        if f not in REGISTRY and f not in NOT_HELD:', '        if f not in REGISTRY and f not in NOT_HELD.values():', RED),
    ("X11_couplings_skip_vault", "scripts/ratchet.py",
     '    for n, c in rows(text, path, 6):\n', '    for n, c in rows(text, path, 6):\n        if c[0] == "src/db/vault.rs":\n            continue\n', RED),
    ("X12_door_skip_cfgout", "scripts/ratchet.py",
     "        k = f\"{c[0]} {c[1]} {c[2]}\"\n", "        if c[0] == \"cfgout\":\n            continue\n        k = f\"{c[0]} {c[1]} {c[2]}\"\n", RED),
    ("X13_couplings_skip_test_region", "scripts/ratchet.py",
     '    for n, c in rows(text, path, 6):\n', '    for n, c in rows(text, path, 6):\n        if c[3] == "test":\n            continue\n', RED),
    ("X14_not_held_typo_unchecked", "scripts/ratchet.py",
     '            red.append(f"{f}: NOT_HELD names a file the tree does not have")', '            pass', RED),
    ("X15_jobs_env_anywhere", "tests/source.rs",
     '            ["env"] | ["jobs", _, "env"] | ["jobs", _, "steps", "env"] => {', '            [.., "env"] => {', CI),

    ("X16_door_key_by_file_summed", "scripts/ratchet.py",
     '        k = f"{c[0]} {c[1]} {c[2]}"\n        if k in out:\n            raise ValueError(f"{path} line {n}: a key listed twice: {k}")\n        out[k] = number(path, n, c[3])',
     '        k = f"{c[0]} {c[1]}"\n        out[k] = out.get(k, 0) + number(path, n, c[3])', RED),

]

FAKE = ["env", "SUPDB_PR_BASE=1111111111111111111111111111111111111111",
        "SUPDB_PUSH_BEFORE=1111111111111111111111111111111111111111", "SUPDB_BASE=HEAD"]
EXTRA = "name: extra\non:\n  pull_request:\njobs:\n  t:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@v4\n      - run: cargo test\n"
M += [
    # Round 3: the reviewers' extra mutants.
    ("E1_extra_workflow", ".github/workflows/extra.yml", None, EXTRA, CI),
    ("E2_apple_checks_by_hand", ".github/workflows/apple-silicon.yml",
     "run: sh scripts/check.sh machine", "run: cargo test --profile checked", CI),
    ("E3_couplings_skip_test_region", "scripts/ratchet.py",
     '        if c[3] not in ("code", "test"):',
     '        if c[3] == "test":\n            continue\n        if c[3] not in ("code", "test"):', RED),
    ("E4_setup_writes_github_env", "bench/scripts/libclang.sh",
     '    echo "$1=$2" >> "$GITHUB_ENV"\n',
     '    echo "$1=$2" >> "$GITHUB_ENV"\n    echo RATCHET_RED_STEPS=G >> "$GITHUB_ENV"\n', CI),
    ("E5_short_zeros_are_zeros", "scripts/base.sh",
     '[ "$SUPDB_PUSH_BEFORE" != "$zeros" ]',
     '[ -n "$(printf %s "$SUPDB_PUSH_BEFORE" | tr -d 0)" ]', RED),
    ("E6_fresh_without_reason", "scripts/ratchet.py",
     '    fresh = tree_d - base_d',
     '    fresh = Counter({d: n for d, n in tree_d.items() if d[:4] not in {b[:4] for b in base_d}})', RED),
    ("E7_any_region", "scripts/ratchet.py",
     '        if c[3] not in ("code", "test"):', '        if False:', RED),
    ("E8a_apt_semicolon", "tests/source.rs",
     'c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c)',
     'c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-;".contains(c)', CI),
    ("E8b_apt_ampersand", "tests/source.rs",
     'c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c)',
     'c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-&".contains(c)', CI),
    ("E9_cost_base_override_dropped", "scripts/check.sh",
     'base=$(SUPDB_BASE="${COST_BASE:-${SUPDB_BASE:-}}" sh scripts/base.sh)',
     'base=$(SUPDB_BASE="${SUPDB_BASE:-}" sh scripts/base.sh)', RED),
    # E10: G's inner calls carry no clean_env now; G under a made-up outer
    # environment, and the group's mutants under it.
    ("E10_G1_under_fake_env", "scripts/check.sh",
     '      sh scripts/ratchet-red.sh || exit 1', '      sh scripts/ratchet-red.sh || true',
     FAKE + ["RATCHET_RED_STEPS=G"] + RED),
    ("E10_G2_under_fake_env", "scripts/check.sh",
     '      sh scripts/ratchet.sh || exit 1', '      sh scripts/ratchet.sh || true',
     FAKE + ["RATCHET_RED_STEPS=G"] + RED),
    # Where clean_env does decide: B's and L's calls, under the same.
    ("Z1_B_without_clean_env", "scripts/ratchet-red.sh",
     'base() { (cd "$1" && shift && env $clean_env "$@" sh "$ROOT/scripts/base.sh"); }',
     'base() { (cd "$1" && shift && env "$@" sh "$ROOT/scripts/base.sh"); }',
     FAKE + ["RATCHET_RED_STEPS=B"] + RED),
    ("Z2_L_without_clean_env", "scripts/ratchet-red.sh",
     'check() { (cd "$L" && env $clean_env sh "$ROOT/scripts/ratchet.sh" "$@"); }',
     'check() { (cd "$L" && env sh "$ROOT/scripts/ratchet.sh" "$@"); }',
     FAKE + ["RATCHET_RED_STEPS=L"] + RED),
    # Round 3: my own, on the lines this round wrote.
    ("Z3_other_allows_pull_request", "tests/source.rs",
     '["workflow_dispatch", "schedule", "push"].contains(&key)',
     '["workflow_dispatch", "schedule", "push", "pull_request"].contains(&key)', CI),
    ("Z4_unlisted_workflow_passes", "tests/source.rs",
     '        } else {\n            errs.push(format!(\n                "{name}: a workflow the test does not list',
     '        } else if false {\n            errs.push(format!(\n                "{name}: a workflow the test does not list', CI),
    ("Z5_listed_gone_passes", "tests/source.rs",
     '        if !files.iter().any(|(f, _)| f == n) {',
     '        if false && !files.iter().any(|(f, _)| f == n) {', CI),
    ("Z6_push_branches_unread", "tests/source.rs",
     '        } else if trig == "push" && t != "branches: [main]" {',
     '        } else if false && trig == "push" && t != "branches: [main]" {', CI),
    ("Z7_bare_push_passes", "tests/source.rs",
     '.any(|l| l.trim_start().starts_with("push:"))', '.any(|l| l.trim_start().starts_with("push-never:"))', CI),
    ("Z8b_no_trigger_passes", "tests/source.rs",
     '        errs.push(format!("{name}: an `on:` with no trigger"));', '', CI),
    ("Z9b_no_on_passes", "tests/source.rs",
     '        errs.push(format!("{name}: no one `on:` the test can read"));\n        return errs;',
     '        return errs;', CI),
    ("Z10_prints_unheld", "tests/source.rs",
     '        let prints = to_env\n', '        let prints = false && to_env\n', CI),
    ("Z11_emit_unheld", "tests/source.rs",
     '        if words.first() == Some(&"emit") {', '        if words.first() == Some(&"emit-never") {', CI),
    ("Z12_github_env_unread", "tests/source.rs",
     '    "GITHUB_ENV",\n    "GITHUB_PATH",', '    "GITHUB_PATH",', CI),
    ("Z13_paths_uncollected", "tests/source.rs",
     '            triggers.push((ln, "pull_request.paths.-".to_string(), rest.to_string()));\n',
     '', CI),
    ("Z14_workflows_not_recursive", "tests/source.rs",
     '            if p.is_dir() {\n                dirs.push(p);\n                continue;\n            }',
     '            if p.is_dir() {\n                continue;\n            }', CI),
    ("Z15_ci_yml_text_ignored", "tests/source.rs",
     '        if n == CI_YML {\n            *t = yaml.to_string();', '        if n == "never" {\n            *t = yaml.to_string();', CI),
    ("Z16_stderr_not_exempt", "tests/source.rs",
     '            && !t.contains(">&2");', '            ;', CI),
    ("Z17_sudo_script_unread", "tests/source.rs",
     '    let w = if w.first() == Some(&"sudo") {', '    let w = if w.first() == Some(&"sudo-never") {', CI),
]


def sha(p):
    return hashlib.sha256(open(p, "rb").read()).hexdigest()


def main():
    want = sys.argv[1:]
    res = os.path.join(B, "mutants.results")
    env = dict(os.environ, TMPDIR=TMP)
    for name, f, old, *rest in M:
        new, cmd = (None, rest[0]) if len(rest) == 1 else rest
        if want and name not in want:
            continue
        p = os.path.join(ROOT, f)
        if old is None:
            assert not os.path.exists(p), f"{f} exists before {name}"
            open(p, "w").write(new)
            log = os.path.join(B, f"mutant-{name}.log")
            with open(log, "w") as lf:
                st = subprocess.run(cmd, cwd=ROOT, env=env, stdout=lf, stderr=subprocess.STDOUT).returncode
            os.remove(p)
            assert not os.path.exists(p)
            with open(res, "a") as r:
                r.write(f"{name}\t{'KILLED' if st != 0 else 'SURVIVED'}\texit {st}\n")
            continue
        keep = os.path.join(B, "keep", f)
        orig = open(keep).read()
        assert sha(p) == sha(keep), f"{f} differs from keep/ before {name}"
        pairs = old if isinstance(old, list) else [(old, new)]
        bad = [o for o, _ in pairs if orig.count(o) != 1]
        if bad:
            with open(res, "a") as r:
                r.write(f"{name}\tNOT-APPLIED ({[orig.count(o) for o in bad]} matches)\n")
            continue
        mut = orig
        for o, n in pairs:
            mut = mut.replace(o, n, 1)
        open(p, "w").write(mut)
        log = os.path.join(B, f"mutant-{name}.log")
        with open(log, "w") as lf:
            st = subprocess.run(cmd, cwd=ROOT, env=env, stdout=lf, stderr=subprocess.STDOUT).returncode
        open(p, "w").write(orig)
        assert sha(p) == sha(keep), f"{f} not put back after {name}"
        os.chmod(p, os.stat(keep).st_mode)
        with open(res, "a") as r:
            r.write(f"{name}\t{'KILLED' if st != 0 else 'SURVIVED'}\texit {st}\n")
    print("done")


main()
