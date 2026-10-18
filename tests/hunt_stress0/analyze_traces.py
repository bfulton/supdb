#!/usr/bin/env python3
"""Second reading of the failing logs' traces, with the publisher's store of the
position taken as the first one after the swap, on the same state.

For the scan that answered wrongly (the last SCAN_START before the failure
notice) and the last take of its that follows a swap of the same block on the
same state:

  R1  the scan's load of forms_at (SCAN_START, canonical 1, forms_at == bound)
  A   the swap of the slot the take read (PUBLISH_FORM of that block, on the
      state the scan reads, the last before the take) and its pos
  R2  the take
  B   the first store of forms_at (FORMS_AT_STORED) on that state after A

The window of the race: A < B. The race is exposed when R1 < B and A < R2, and
the swapped form's pos is past the scan's bound.

It reads the logs `loop.sh` keeps for failing runs of the single-reader
scenario. With several snapshot handles scanning at once the last scan before
the failure notice is not always the one that answered wrongly, and those logs
are not read right by this script.
Usage: analyze_traces.py LOG...
"""
import re
import sys

line_re = re.compile(
    r"^TRACE\s+(?P<t>[0-9.]+) us\s+(?P<who>\S+)\s+(?P<what>.*?)(?:\s+\[state gen (?P<gen>\d+) @(?P<ptr>[0-9a-f]+)\])?$"
)
notice_re = re.compile(r"^TRACE \(failure noticed at (?P<t>[0-9.]+) us\)")


def parse(path):
    ev = []
    noticed = None
    for l in open(path, errors="replace"):
        l = l.rstrip("\n")
        m = notice_re.match(l)
        if m:
            noticed = float(m["t"])
            continue
        m = line_re.match(l)
        if m:
            ev.append((float(m["t"]), m["who"], m["what"], m["gen"], m["ptr"]))
    return ev, noticed


def main():
    counts = {}
    odd = []
    for path in sys.argv[1:]:
        name = path.split("/")[-1]
        ev, noticed = parse(path)
        if noticed is None:
            counts["no notice"] = counts.get("no notice", 0) + 1
            continue
        hand = [e for e in ev if e[1].startswith("h") and e[0] <= noticed]
        starts = [e for e in hand if "SCAN_START" in e[2]]
        if not starts:
            counts["no scan start"] = counts.get("no scan start", 0) + 1
            continue
        r1 = starts[-1]
        end = next((e for e in hand if e[2].startswith("SCAN_END") and e[0] >= r1[0] and e[1] == r1[1]), None)
        t_end = end[0] if end else noticed
        takes = [e for e in hand if e[1] == r1[1] and e[2].startswith("TAKE") and r1[0] <= e[0] <= t_end]
        pubs = [e for e in ev if e[2].startswith("PUBLISH_FORM")]
        stores = [e for e in ev if "FORMS_AT_STORED" in e[2]]
        m = re.search(r"bound (\d+) forms_at (\d+) canonical (\d)", r1[2])
        bound, at, canon = int(m[1]), int(m[2]), int(m[3])
        hit = None
        for t in takes:
            blk = re.search(r"TAKE (p\d+ b\d+) kind (\d)", t[2])
            if not blk:
                continue
            swaps = [
                p
                for p in pubs
                if p[4] == t[4] and re.search(r"PUBLISH_FORM " + re.escape(blk.group(1)) + r" pos", p[2]) and p[0] <= t[0]
            ]
            if swaps:
                hit = (t, swaps[-1], blk.group(1))
        if hit is None:
            counts["no swap before a take on the state"] = counts.get("no swap before a take on the state", 0) + 1
            odd.append(name + ": no swap before a take on the state")
            continue
        t, a, blk = hit
        pos_m = re.search(r"pos (\d+)", a[2])
        pos = int(pos_m[1]) if pos_m else -1
        b = next((s for s in stores if s[4] == a[4] and s[0] >= a[0]), None)
        r1_t, a_t, r2_t = r1[0], a[0], t[0]
        b_t = b[0] if b else float("inf")
        exposed = canon == 1 and at == bound and r1_t < b_t and a_t < r2_t and pos > bound
        key = "exposed" if exposed else "NOT exposed"
        counts[key] = counts.get(key, 0) + 1
        if not exposed:
            odd.append(f"{name}: R1 {r1_t:.1f} A {a_t:.1f} R2 {r2_t:.1f} B {b_t:.1f} pos {pos} bound {bound} at {at} canon {canon} blk {blk}")
        else:
            order = "R1<A" if r1_t < a_t else "A<R1"
            order += "|R2<B" if r2_t < b_t else "|B<R2"
            counts["  " + order] = counts.get("  " + order, 0) + 1
    for k in sorted(counts):
        print(f"{k}: {counts[k]}")
    for o in odd[:40]:
        print("  ", o)


main()
