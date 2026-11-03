//! What the resident count reads and what a trim returns of it.
//!
//! Its own test binary, with one test, because the count is the process's:
//! the anonymous bytes resident beyond the allocator's use, which any
//! thread's allocation from free chunks already resident lowers. In the
//! `db` binary, beside two hundred tests that allocate and free, a dead
//! arena of thirty-two megabytes read as nine more between the two readings
//! of one run of the gate, the other tests' allocations taking the rest.

use supdb::Db;

mod common;
use common::dead_arena;

/// What the resident count reads (`Options::trim_by_resident`) and what a
/// trim returns of it: a dead arena of thirty-two megabytes reads as that
/// much more resident beyond use, and a trim of every arena gives it back.
/// Each difference is held to half the arena and not to its bytes, since
/// the process's own threads allocate beside it.
#[test]
fn the_resident_count_reads_a_dead_arenas_pages_and_a_trim_returns_them() {
    if !Db::trim_supported() {
        return;
    }
    // The process's resident free pages returned first, so the dead arena's
    // pieces come from pages that have to be faulted in: in the gate's whole
    // `db` run the process held 118 MB resident and free from the other
    // tests' dead arenas, the pieces came from pages those had left
    // resident, and the count rose 3 MB for an arena of 32.
    Db::trim_now();
    let before =
        Db::resident_beyond_use().expect("the resident count reads where the free count does");
    let keeper = dead_arena(32 << 20);
    let freed = Db::resident_beyond_use().unwrap();
    assert!(
        freed >= before + (16 << 20),
        "a dead arena of 32 MB read {before} -> {freed} resident beyond use"
    );
    Db::trim_now();
    let trimmed = Db::resident_beyond_use().unwrap();
    assert!(
        freed - trimmed.min(freed) >= 16 << 20,
        "a trim returned {freed} -> {trimmed} of the dead arena's pages"
    );
    drop(keeper);
}
