# Contributing to supdb

## The tests you can run here are not the whole gate

This repository is the engine: the library, its tests, the browser reader and
the design notes. `cargo test` and `sh scripts/check.sh` are real, and they
are the smaller half. The falsification suite, the comparison against other
engines, the correctness suite, the browser test and `claims.json` -- the file
that records what the engine is expected to do, including what it is expected
to do badly -- live in [supdb-bench](https://github.com/bfulton/supdb-bench),
and this repository's CI runs that suite against every pull request. A change
that passes everything here can still be red, and it is red for a reason about
the engine rather than about the tests: the suite fails in both directions,
when a known limitation gets worse and when one quietly gets fixed.

So most engine work is done from a clone of supdb-bench with this repository
checked out at `supdb/` inside it, where the experiment that will judge a
change is open while the change is made. The rest of this file is the short
form; supdb-bench's `CONTRIBUTING.md` has the flow in full and its `CLAUDE.md`
has the reasons.

## Getting the code

    git clone https://github.com/bfulton/supdb
    cd supdb
    sh scripts/check.sh            # everything that can be checked here
    sh scripts/check.sh lint       # or one group; the script's header lists them

Rust stable with `rustfmt` and `clippy`; the browser reader also wants the
`wasm32-unknown-unknown` target (`web/build.sh`). CI calls the same script with
the same group names, so what passes here passes there -- for this half.

Or, and usually better:

    git clone --recurse-submodules https://github.com/bfulton/supdb-bench
    cd supdb-bench/supdb           # this repository, at the revision the suite last recorded
    git checkout -b my-change main

and work there, running the suite from the directory above.

## How a change lands

Engine CI clones supdb-bench at `main` and runs the suite against the pull
request. Two cases.

**The change moves no claim.** Open the pull request; it is green against
supdb-bench `main`; merge it. Nothing else to do.

**The change moves a claim** -- fixes a recorded limitation, moves a measured
number, gives an experiment something new to measure. It cannot be green on
both sides at once, and the order is:

1. Push the engine branch and open its pull request. It is red, because
   supdb-bench `main` still carries the old claims. Expected, and not yet
   actionable.
2. In supdb-bench, on a branch: point the submodule at the pushed engine
   commit (`git -C supdb checkout <sha> && git add supdb`), re-record what
   moved, edit `claims.json`, open that pull request. It is green -- a
   submodule pointer is a SHA, and a SHA can name a commit no branch has
   merged. The whole workflow rests on that.
3. **Merge supdb-bench first.**
4. The engine pull request re-runs against the new `main` and is green. Merge
   it.

To see the engine pull request green before step 3, the repository variable
`SUPDB_BENCH_BRANCH` points engine CI at a supdb-bench branch. Set it by hand
and clear it when the bench merges. Never commit an override: it would have to
survive the merge -- the engine merges after the bench, so nothing could
remove it in time -- and would leave `main` pointed at a branch that was about
to be deleted. While the variable is set, every engine pull request is tested
against that branch, not only the one it was set for.

## Two rules of this repository

**Merge commits only.** No squash, no rebase merge, and no force-push to a
branch once supdb-bench has pinned one of its commits. supdb-bench names
engine commits by SHA, and rewriting them leaves those pointers naming commits
reachable from nothing. supdb-bench itself may squash -- nothing pins its
commits -- and the asymmetry is easy to get backwards: the repository that is
pointed at must not rewrite; the one that points may.

**The gate runs on pull requests and tags**, not on pushes to `main`. The pull
request run has already tested the tree the merge produces; a tag is a
revision somebody means to cite, and carries a run of its own.

## What goes where

Code, tests and the design notes (`docs/engine.md`, `docs/index-theory.md`)
here. Every number, claim, plan file and result in supdb-bench. Keep
`README.md` and the crate docs to what is true now and likely to stay so; the
reasoning and the history go in supdb-bench's `CLAUDE.md` and plan files,
where a reader who wants them can follow the pointer.
