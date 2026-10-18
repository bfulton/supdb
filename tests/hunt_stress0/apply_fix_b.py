#!/usr/bin/env python3
# Scratch experiment, not a proposal for the engine: the published form carries
# the log position it was settled at, and a reader at an older position treats
# a form past its own as one to build for itself. Applied to the src/db.rs of a
# scratch copy of the tree (never the worktree), by path: apply_fix_b.py PATH.
# Takes the hunt branch's tree and a clean one.
import sys

p = sys.argv[1]
s = open(p).read()


def rep(old, new, count=1):
    global s
    assert s.count(old) == count, (old[:80], s.count(old))
    s = s.replace(old, new)


rep(
    """struct CanonicalForm {
    form: std::sync::Arc<Cached>,
    bytes: usize,
}""",
    """struct CanonicalForm {
    form: std::sync::Arc<Cached>,
    bytes: usize,
    /// SCRATCH EXPERIMENT: the log position the form was settled at when it
    /// was swapped into its slot; zero for a form carried across a publish,
    /// which is current to whatever `forms_at` of the state says. A reader
    /// bound to an older position than this does not take it.
    pos: usize,
}""",
)
rep(
    """                let carried = Box::into_raw(Box::new(CanonicalForm {
                    form,
                    bytes: form_bytes,
                }));""",
    """                let carried = Box::into_raw(Box::new(CanonicalForm {
                    form,
                    bytes: form_bytes,
                    pos: 0,
                }));""",
)
rep(
    """        let new = Box::into_raw(Box::new(CanonicalForm {
            form: form.clone(),
            bytes,
        }));
        let old = slot.swap(new, AtomicOrdering::AcqRel);""",
    """        let new = Box::into_raw(Box::new(CanonicalForm {
            form: form.clone(),
            bytes,
            pos: self.mem().committed_log().min(self.log_bound.get()),
        }));
        let old = slot.swap(new, AtomicOrdering::AcqRel);""",
)
rep(
    """            let form = Box::into_raw(Box::new(CanonicalForm {
                form: std::sync::Arc::new(Cached::Clean),
                bytes: 0,
            }));""",
    """            let form = Box::into_raw(Box::new(CanonicalForm {
                form: std::sync::Arc::new(Cached::Clean),
                bytes: 0,
                pos: 0,
            }));""",
)
# the reader's check, in canonical(): after the slot's load and before the
# form is handed out (the hunt branch's tree has a trace event there; a clean
# tree returns the form)
CHECK = """        // SCRATCH EXPERIMENT: a form settled past the position this handle
        // reads at is not for it, whatever `forms_at` said when the scan
        // began: the slot was swapped after that, and the position is
        // stored after the swaps. A marker, so the block is built.
        if e.pos != 0 && self.forms_bound().is_some_and(|bound| e.pos > bound) {
            return Some(&REJECTED_FORM);
        }
"""
if s.count("        let e = unsafe { &*e };\n        dbgtrace::ev3(\n            4,") == 1:
    rep(
        "        let e = unsafe { &*e };\n        dbgtrace::ev3(\n            4,",
        "        let e = unsafe { &*e };\n" + CHECK + "        dbgtrace::ev3(\n            4,",
    )
else:
    rep(
        "        let e = unsafe { &*e };\n        Some(&e.form)",
        "        let e = unsafe { &*e };\n" + CHECK + "        Some(&e.form)",
    )
rep(
    """static NO_SNAPSHOT: std::sync::LazyLock<Snapshot> = std::sync::LazyLock::new(Snapshot::default);""",
    """static NO_SNAPSHOT: std::sync::LazyLock<Snapshot> = std::sync::LazyLock::new(Snapshot::default);

/// SCRATCH EXPERIMENT: what `Reader::canonical` answers for a published form
/// that is ahead of the reader: the mark a reader builds its own for.
static REJECTED_FORM: std::sync::LazyLock<Cached> = std::sync::LazyLock::new(|| {
    Cached::Wide(WideBlock {
        sorted: Vec::new(),
        seen: 0,
        covered: false,
        walks: 0,
    })
});""",
)
open(p, "w").write(s)
print("applied")
