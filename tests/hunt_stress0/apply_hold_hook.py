"""Patch an older tree's src/db.rs with the forms publisher's hold: a dbgtrace
module (hold_forms_position, forms_hold_parked, forms_hold_wait) and a call to
the wait as the first statement of the upkeep pass's publish block. With
`trace` as the second argument the wait also says what the pass holds (the
bound it was named, the commit the writer is at, the position the state
vouches for) and the scan says what it decided, both only while the hold is
armed.
usage: apply_hold_hook.py PATH-TO-db.rs [trace]"""
import re
import sys

path = sys.argv[1]
trace = len(sys.argv) > 2 and sys.argv[2] == "trace"
text = open(path).read()

pat = re.compile(r"\n([ \t]*)if self\.publish_dirty\(false\) \{\n")
found = pat.findall(text)
assert len(found) == 1, f"expected one publish_dirty(false) block, found {len(found)}"
if trace:
    call = "dbgtrace::forms_hold_wait_note(self.log_bound.get(), self.mem().committed_log(), st.forms_at.load(AtomicOrdering::Acquire), st.gen);"
else:
    call = "dbgtrace::forms_hold_wait();"
text = pat.sub(
    lambda m: f"\n{m.group(1)}if self.publish_dirty(false) {{\n{m.group(1)}    {call}\n",
    text,
)

if trace:
    scan_pat = re.compile(
        r"(\n[ \t]*let canonical = self\.opts\.commit_forms\n[ \t]*&& self\n[ \t]*\.forms_bound\(\)\n[ \t]*\.is_some_and\(\|n\| st\.forms_at\.load\(AtomicOrdering::Acquire\) == n\);\n)"
    )
    assert len(scan_pat.findall(text)) == 1, "scan decision not found once"
    text = scan_pat.sub(
        lambda m: m.group(1)
        + "        dbgtrace::scan_note(self.forms_bound(), st.forms_at.load(AtomicOrdering::Acquire), canonical, self.slot.is_some(), st.gen);\n",
        text,
    )

text += '''

/// HUNT (scratch, not for the engine): the forms publisher's hold, to force
/// the race between its slot swaps and its store of the position.
#[doc(hidden)]
pub mod dbgtrace {
    use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
    static FORMS_HOLD: AtomicU8 = AtomicU8::new(0);
    static NOTES: AtomicU64 = AtomicU64::new(0);

    pub fn hold_forms_position(on: bool) {
        if on {
            FORMS_HOLD.fetch_or(1, Ordering::AcqRel);
        } else {
            FORMS_HOLD.fetch_and(!1, Ordering::AcqRel);
        }
    }

    pub fn forms_hold_parked() -> bool {
        FORMS_HOLD.load(Ordering::Acquire) & 2 != 0
    }

    #[allow(dead_code)]
    pub(super) fn forms_hold_wait_note(log_bound: usize, committed: usize, forms_at: usize, gen: u64) {
        if FORMS_HOLD.load(Ordering::Acquire) & 1 != 0 {
            eprintln!("PARK pass log_bound={log_bound} committed_log={committed} forms_at={forms_at} state_gen={gen}");
        }
        forms_hold_wait();
    }

    #[allow(dead_code)]
    pub(super) fn scan_note(bound: Option<usize>, at: usize, canonical: bool, handle: bool, gen: u64) {
        if handle && FORMS_HOLD.load(Ordering::Acquire) & 3 == 3 {
            let n = NOTES.fetch_add(1, Ordering::Relaxed);
            if n < 3 {
                eprintln!("SCAN handle bound={bound:?} forms_at={at} canonical={canonical} state_gen={gen} (parked)");
            }
        }
    }

    pub(super) fn forms_hold_wait() {
        if FORMS_HOLD.load(Ordering::Acquire) & 1 == 0 {
            return;
        }
        FORMS_HOLD.fetch_or(2, Ordering::AcqRel);
        let t = std::time::Instant::now();
        while FORMS_HOLD.load(Ordering::Acquire) & 1 != 0
            && t.elapsed() < std::time::Duration::from_secs(60)
        {
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        FORMS_HOLD.fetch_and(!2, Ordering::AcqRel);
    }
}
'''
open(path, "w").write(text)
print("patched", path, "(trace)" if trace else "")
