#!/usr/bin/env python3
"""Group pager fills by the read that caused them, from an ftrace trace_pipe dump.

Uprobes (file offsets into bench/target/release/bench built from 3df85d22; vaddr = offset + 0x1000):
  zask:   entry of <Paged as Paging>::ensure (vaddr 0x306780): self=&Paged, off, len
  zpiece: entry of <Paged as Paging>::piece_from (vaddr 0x3068c0): self=&Paged, off
          (the by-the-piece record path, one chunk from off)
  zfill:  vaddr 0x3076ef inside Paged::slow, after a chunk's decrypt succeeded
          (pager.rs:1091-1095): self=&Paged, c
All three filtered to threads named "bench" (the pass's reading thread). Only ensure
(0x306824, 0x306864) and piece_from (0x3068fd) call slow, so every fill follows one of
the two asks on its thread.

Windows are marked by trace_marker lines "ARM <name>" / "ARM post-<name>". Within an arm,
the thread with the most asks is the reader; a read is:
  form arm: one 4096-byte ask at a 4096-aligned offset (a form page);
  raw arm:  a run of 8-byte ensure asks (the hash slots) and the piece asks after it,
            up to the next 8-byte ask that follows a non-slot ask.
Each read's chunks asked and filled are kept as sets of (self, c). Events are split in
two halves of the arm's window by time, the second being the steady state.
"""
import re
import sys
from collections import defaultdict, Counter

CHUNK = 4080
ev_re = re.compile(r"^\s*(.+?)-(\d+)\s+\[\d+\]\s+\S+\s+([\d.]+):\s+(zask|zfill|zpiece):\s+\(\w+\)\s*(.*)$")
mark_re = re.compile(r"^\s*.*?\s([\d.]+):\s+tracing_mark_write:\s*ARM\s+(\S+)")
kv_re = re.compile(r"(\w+)=(0x[0-9a-fA-F]+|\d+)")

path = sys.argv[1]

# First pass: windows and the reader thread per arm.
windows = {}  # arm -> [t0, t1]
asks_by = defaultdict(Counter)
arm = None
with open(path) as f:
    for raw in f:
        m = mark_re.match(raw)
        if m:
            t, name = float(m.group(1)), m.group(2)
            if name.startswith("post-"):
                windows[name[5:]][1] = t
                arm = None
            else:
                windows[name] = [t, None]
                arm = name
            continue
        if arm is None:
            continue
        m = ev_re.match(raw)
        if m and m.group(4) != "zfill":
            asks_by[arm][int(m.group(2))] += 1
reader = {a: c.most_common(1)[0][0] for a, c in asks_by.items()}
print("windows", {a: (w[0], w[1], round(w[1] - w[0], 3)) for a, w in windows.items()})
print("reader tids", reader, {a: c.most_common(3) for a, c in asks_by.items()})

stats = defaultdict(Counter)  # (arm, half) -> named counters
form_hist = defaultdict(Counter)  # (arm, half) -> fills per page ask
form_pat = defaultdict(Counter)
raw_rec_hist = defaultdict(Counter)  # (arm, half) -> (record chunks asked, record chunks filled)
raw_slot_hist = defaultdict(Counter)


def half_of(a, t):
    t0, t1 = windows[a]
    return 1 if t < (t0 + t1) / 2 else 2


class Read:
    __slots__ = ("kind", "half", "page", "page_self", "slot", "rec", "filled", "other")

    def __init__(self, kind, half):
        self.kind, self.half = kind, half
        self.page = None
        self.slot, self.rec, self.filled, self.other = set(), set(), set(), set()


cur = None
arm = None


def close(a):
    global cur
    r = cur
    cur = None
    if r is None:
        return
    key = (a, r.half)
    s = stats[key]
    if r.kind == "page":
        n = len(r.filled & r.page)
        form_hist[key][n] += 1
        c0, c1 = sorted(x[1] for x in r.page) if len(r.page) == 2 else (None, None)
        if c0 is not None:
            form_pat[key][(int((r.page_self, c0) in r.filled), int((r.page_self, c1) in r.filled))] += 1
        s["page_asks"] += 1
        s["page_fills"] += n
        s["page_chunks_asked"] += len(r.page)
        s["fills_not_in_page"] += len(r.filled - r.page)
    else:
        s["raw_reads"] += 1
        sf = len(r.filled & r.slot)
        rf = len(r.filled & (r.rec - r.slot))
        s["slot_chunks"] += len(r.slot)
        s["slot_fills"] += sf
        s["rec_chunks"] += len(r.rec - r.slot)
        s["rec_fills"] += rf
        s["fills_elsewhere"] += len(r.filled) - sf - rf
        raw_rec_hist[key][(len(r.rec - r.slot), rf)] += 1
        raw_slot_hist[key][(len(r.slot), sf)] += 1


with open(path) as f:
    last_slot = False
    for raw in f:
        m = mark_re.match(raw)
        if m:
            if arm is not None:
                close(arm)
            name = m.group(2)
            arm = None if name.startswith("post-") else name
            last_slot = False
            continue
        if arm is None:
            continue
        m = ev_re.match(raw)
        if not m:
            continue
        tid, t, ev, rest = int(m.group(2)), float(m.group(3)), m.group(4), m.group(5)
        kv = {k: int(v, 0) for k, v in kv_re.findall(rest)}
        h = half_of(arm, t)
        if tid != reader[arm]:
            stats[(arm, h)]["events_off_reader_" + ev] += 1
            continue
        stats[(arm, h)]["events_" + ev] += 1
        if ev == "zask":
            off, ln, me = kv["off"], kv["len"], kv["self"]
            if ln == 0:
                continue
            chunks = {(me, c) for c in range(off // CHUNK, (off + ln - 1) // CHUNK + 1)}
            if ln == 4096 and off % 4096 == 0:
                close(arm)
                cur = Read("page", h)
                cur.page = chunks
                cur.page_self = me
                last_slot = False
            elif ln == 8:
                if cur is None or cur.kind != "raw" or not last_slot:
                    close(arm)
                    cur = Read("raw", h)
                cur.slot |= chunks
                last_slot = True
            else:
                if cur is None:
                    cur = Read("raw", h)
                if cur.kind == "raw":
                    cur.rec |= chunks
                else:
                    cur.other |= chunks
                stats[(arm, h)]["ask_len_other"] += 1
                last_slot = False
        elif ev == "zpiece":
            off, me = kv["off"], kv["self"]
            if cur is None:
                cur = Read("raw", h)
            if cur.kind == "raw":
                cur.rec.add((me, off // CHUNK))
            else:
                cur.other.add((me, off // CHUNK))
            last_slot = False
        else:
            if cur is None:
                stats[(arm, h)]["fills_no_read"] += 1
            else:
                cur.filled.add((kv["self"], kv["c"]))
    if arm is not None:
        close(arm)

for key in sorted(stats):
    s = stats[key]
    print("==", key, dict(s))
    if s["page_asks"]:
        n = s["page_asks"]
        miss = n - form_hist[key][0]
        print(f"   form: page asks {n} fills/ask {s['page_fills']/n:.4f} misses/ask {miss/n:.4f} "
              f"fills/miss {s['page_fills']/max(miss,1):.4f} hist {dict(sorted(form_hist[key].items()))} "
              f"pattern(first,second) {dict(form_pat[key])}")
    if s["raw_reads"]:
        n = s["raw_reads"]
        print(f"   raw: reads {n} slot_chunks/read {s['slot_chunks']/n:.4f} slot_fills/read {s['slot_fills']/n:.4f} "
              f"rec_chunks/read {s['rec_chunks']/n:.4f} rec_fills/read {s['rec_fills']/n:.4f} "
              f"fills_elsewhere/read {s['fills_elsewhere']/n:.4f}")
        print("   raw (record chunks asked, filled):", dict(raw_rec_hist[key].most_common(8)))
        print("   raw (slot chunks asked, filled):", dict(raw_slot_hist[key].most_common(6)))
