# Two durable helpers that read a commit's answer on the stopped queue's flag

`sweep.sh` runs the four tests of the two helpers twenty-five times at each
width of a scratch window in `Shared::wedge` (a spin of `SUPDB_SCRATCH_WEDGE_US`
microseconds after the flag is set, before the failure is left for the
writer; put into `src/db.rs` for the hunt and taken out, the file put back by
its sha256). `sweep-before.log` is the tests as they stood, `sweep-after.log`
the fixed ones, `whole-after.log` the whole durable binary under three widths,
`red-at-*.out` two red runs' output.
