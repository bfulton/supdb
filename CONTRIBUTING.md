# Contributing to supdb

## Getting the code

    git clone https://github.com/bfulton/supdb
    cd supdb
    sh scripts/check.sh            # build, test, lint, wasm, bench -- what CI runs
    sh scripts/check.sh lint       # or one group; the script's header lists them

Rust stable with `rustfmt` and `clippy`; the browser reader also wants the
`wasm32-unknown-unknown` target (`web/build.sh`). The `bench` group builds
the comparators, which is a ten-minute C++ build the first time and needs
libclang; `bench/scripts/libclang.sh` names it when cargo cannot. CI calls
the same script with the same group names, so what passes here passes there.

## The suite

`bench/` is the benchmark suite and its own cargo workspace. It measures
every arm over a ladder of store sizes and writes one row of raw samples
under `bench/runs/<scale>/`; `bench/DESIGN.md` is the specification and
`bench/CLAUDE.md` the rules. Two scales:

- `quick` runs on every pull request, on a hosted runner, about three
  minutes. It gates the change against the last ten rows of that runner's
  class in `bench/runs/`, and until a class has three rows it can only prove
  the suite runs end to end.
- `full` sizes its ladder past the machine's memory and takes hours. Run it
  on a quiet machine (`sh scripts/check.sh quick` for the shape; `bench run
  --scale full` for the run), and commit the row if it is worth keeping:

      git add bench/runs/

A `quick` row from a machine you were also using for something else is not
worth keeping.

## How a change lands

One pull request. If it changes the engine and the suite together, say what
moved and why in the description; the row's `sha` names the commit that
produced it, so a suite change that moves a number draws a new band from
the first rows after it and nothing has to be pinned.

The gate runs on pull requests and tags, not on pushes to `main`: the pull
request run has already tested the tree the merge produces, and a tag is a
revision somebody means to cite.

## What goes where

Code, tests and the design notes (`docs/engine.md`, `docs/index-theory.md`)
here; the suite, its rows and its figures under `bench/`. Keep `README.md`
and the crate docs to what is true now and likely to stay so: no standing
figures, a figure attached to a change goes in the pull request that made
it, rounded. The reasoning and the history go in `CLAUDE.md` on each side,
where a reader who wants them can follow the pointer.
