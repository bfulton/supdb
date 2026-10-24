# Drafts for the next lever

Nothing is drafted here at the moment. The memo-size drafts that were here
(`Options::zpages_memo_bytes` with the arms `supdb-cache64tofitautozpagesmemo16`
and `...memo32`) landed with the commit that added them to the record
(`stage4.md`, "The memo's size, counted"); the lever C drafts were
superseded by `Options::zpages_block_cursor` (`stage4.md`, "A block read
through one cursor").

The next lever is the per-record form, probed and not yet built:
`bench/examples/zrec.rs` and `stage4.md`, "The per-record form, probed
against the comparators". Under 640 MiB at five million keys it reads
point reads at 0.31-0.42x of the group form's time and scans at 0.58x, in
the same process, both forms fitting the bound and faulting nothing, so
the gap is the group decode. Building it means the form file's page as a
container of per-record-decodable groups (offset table, first key raw,
keys front-coded, one dictionary a file in the trailer), the cursor's walk
and the point read decoding a record, and the group form kept as the arm;
the section's last paragraph lists the parts. The suite then prices it on
E and on the controller's passes, which the probe cannot run.

What the counts pointed at before the probe, for E: the decodes themselves, 147,000
groups a pass of 105,000 scans at the shipped memo, with 42% of the memo's
asks missing -- and the three sizes paired say a larger memo does not
buy them back (`stage4.md`, "The memo's size, counted"): the memo pays
for groups that come back while still warm in the last-level cache, and a
hit on a cold group costs about what a decode would. The design's 3.7
names an admission policy for the memo as the arm if the hit rate lands
under 0.45; after the sizes, the question it would have to answer is
which groups stay warm, not which are asked for again, and the instrument
it needs first is a micro-probe of a hit's parse against a decode over
more distinct groups than the last-level cache holds, timed as the decoder
was (`bench/examples/zdecode.rs`). The scans' start groups
(`ZCursor::seek`) are where the misses are, by the profile of the session
before (`ZFile::seek` 27% of E's thread, 22% of it the scans' start seeks).

A script of exact-once replacements is the shape a draft takes here: run it
to apply it, on a committed tree, and check `git diff` after -- never
dry-run one by disarming its `edit` function, since a draft may write a
file directly (the memo-size one did, and a dry run applied half of it).
