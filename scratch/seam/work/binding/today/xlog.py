"""Cross-log recovery model: the WAL, ordered runs' temp files, the manifest.

Commits are numbered 1..n in the order the store made them; a recovered
store is a set of commit numbers, and a state the store held is a prefix
{1..j}.  Every variant below is run over every event sequence up to a depth,
then every crash (each log keeps a prefix of its unsynced writes) and every
single damage (one file cut short, deleted, or the newest manifest slot
unreadable so the one before it is chosen).

Variants (what recovery reads and checks):
  CIPHER   today's cipher: a run's commits are WAL records (log_run), no temp.
  PLAIN    today's plain store and the design's (a): run temp is its own log.
  LIT      PLAIN + the lead's binding: a WAL record written at the run's close
           carrying the run's synced count; the run's segment named at the WAL
           sequence *before* that record; replayed binding with the temp short
           or missing -> refuse.
  SEQ      LIT, but the run's segment named *after* the binding record, so a
           manifest that names the run covers the binding.
  FULL     SEQ + each run's first marker carries the WAL's user-commit
           position at the run's open and the previous run's (id, count,
           end) when no WAL commit came between; all checks before anything.
  LOOSE    FULL + sync ordering (a sync syncs the log holding the oldest
           unsynced write first) + loose links: a link carries (committed,
           synced) and a recovered log short of committed but not of synced,
           with no completed sync after the link, ends the history there.

Policy ALWAYS: every commit syncs its own log before it returns.
Policy EVERYN: explicit SY events; a commit syncs nothing.

Usage: xlog.py VARIANT POLICY DEPTH [MAXRUNS]
"""

import sys
from collections import deque

VARIANTS = ("CIPHER", "PLAIN", "LIT", "SEQ", "FULL", "LOOSE")


class St:
    __slots__ = (
        "n", "wal", "wal_seq", "wal_synced", "wal_user", "retired", "live",
        "run_open", "runs", "inflight", "manifests", "segs", "last_closed",
    )

    def key(self):
        return (self.n, self.wal, self.wal_seq, self.wal_synced, self.wal_user,
                self.retired, self.live, self.run_open, self.runs,
                self.inflight, self.manifests, self.segs, self.last_closed)

    def copy(self):
        s = St()
        for f in St.__slots__:
            setattr(s, f, getattr(self, f))
        return s


# A run: (id, commits, synced, closed, end_seq, present, hdr_pos, hdr_prev, hdr_loose)
R_ID, R_C, R_S, R_CL, R_END, R_PR, R_POS, R_PREV, R_LOOSE = range(9)
# A WAL record: (seq, kind, a, b, c): 'c' commit a; 'b' binding run a, committed b, synced c


def initial():
    s = St()
    s.n = 0
    s.wal = ()
    s.wal_seq = 0
    s.wal_synced = 0          # records of `wal` synced (a prefix)
    s.wal_user = 0            # seq after the last 'c' record
    s.retired = 0             # records with seq below this are gone
    s.live = ()               # WAL commits in the live hashed table
    s.run_open = None
    s.runs = ()
    s.inflight = ()           # (kind, id_or_end, end_seq, commits)
    s.manifests = ((0, frozenset()),)
    s.segs = ()               # (segid, commits)
    s.last_closed = None      # FULL: the run closed with no WAL commit since
    return s


def setrun(s, rid, **kw):
    runs = list(s.runs)
    for i, r in enumerate(runs):
        if r[R_ID] == rid:
            r = list(r)
            for k, v in kw.items():
                r[{"commits": R_C, "synced": R_S, "closed": R_CL, "end": R_END,
                   "present": R_PR}[k]] = v
            runs[i] = tuple(r)
    s.runs = tuple(runs)


def getrun(s, rid):
    for r in s.runs:
        if r[R_ID] == rid:
            return r
    raise KeyError(rid)


def close_run(s, var, pol):
    r = getrun(s, s.run_open)
    rid = r[R_ID]
    if var == "CIPHER":
        end = s.wal_seq
    elif var == "PLAIN":
        end = s.wal_seq
    elif var == "LIT":
        end = s.wal_seq
        s.wal = s.wal + ((s.wal_seq, "b", rid, len(r[R_C]), r[R_S]),)
        s.wal_seq += 1
    else:
        if var == "LOOSE" and pol == "EVERYN" and r[R_S] < len(r[R_C]):
            # The close of a run with unsynced commits: the WAL's frames,
            # all from before the run, synced first, then the run's own,
            # and only then the binding written behind them.
            s.wal_synced = len(s.wal)
            setrun(s, rid, synced=len(r[R_C]))
            r = getrun(s, rid)
        s.wal = s.wal + ((s.wal_seq, "b", rid, len(r[R_C]), r[R_S]),)
        s.wal_seq += 1
        end = s.wal_seq
    setrun(s, rid, closed=True, end=end)
    s.inflight = s.inflight + (("r", rid, end, r[R_C]),)
    s.run_open = None
    return rid


def oldest_unsynced_log(s):
    """('w',) or ('r', id): the log holding the oldest unsynced write."""
    best = None
    if s.wal_synced < len(s.wal):
        rec = s.wal[s.wal_synced]
        t = rec[2] if rec[1] == "c" else max(getrun(s, rec[2])[R_C]) + 0.5
        best = (t, ("w",))
    for r in s.runs:
        if r[R_PR] and r[R_S] < len(r[R_C]):
            t = r[R_C][r[R_S]]
            if best is None or t < best[0]:
                best = (t, ("r", r[R_ID]))
    return None if best is None else best[1]


def events(s, var, pol, maxn, maxruns):
    out = []
    # WC: a commit through the WAL (closes an open run first: leave_direct)
    if s.n < maxn:
        t = s.copy()
        if t.run_open is not None:
            close_run(t, var, pol)
        t.n += 1
        t.wal = t.wal + ((t.wal_seq, "c", t.n, 0, 0),)
        t.wal_seq += 1
        t.wal_user = t.wal_seq
        t.live = t.live + (t.n,)
        t.last_closed = None
        if pol == "ALWAYS":
            t.wal_synced = len(t.wal)
        out.append(("WC", t))
    # RC: a run commit (opens a run over an empty table)
    if s.n < maxn and (s.run_open is not None or (not s.live and len(s.runs) < maxruns)):
        t = s.copy()
        if t.run_open is None:
            rid = len(t.runs) + 1
            prev = None
            loose = False
            if var in ("FULL", "LOOSE"):
                if t.last_closed is not None:
                    p = getrun(t, t.last_closed)
                    prev = (p[R_ID], len(p[R_C]), p[R_S], p[R_END])
                loose = t.wal_synced < len(t.wal)
            t.runs = t.runs + ((rid, (), 0, False, -1, var != "CIPHER", t.wal_user, prev, loose),)
            t.run_open = rid
            t.last_closed = None
        t.n += 1
        r = getrun(t, t.run_open)
        setrun(t, t.run_open, commits=r[R_C] + (t.n,))
        if var == "CIPHER":
            t.wal = t.wal + ((t.wal_seq, "c", t.n, 0, 0),)
            t.wal_seq += 1
            t.wal_user = t.wal_seq
            if pol == "ALWAYS":
                t.wal_synced = len(t.wal)
        elif pol == "ALWAYS":
            r = getrun(t, t.run_open)
            setrun(t, t.run_open, synced=len(r[R_C]))
        out.append(("RC", t))
    # CL: the run closed by a seal (no WAL commit after it)
    if s.run_open is not None:
        t = s.copy()
        rid = close_run(t, var, pol)
        t.last_closed = rid
        out.append(("CL", t))
    # SE: the live WAL table frozen and handed to a seal
    if s.run_open is None and s.live:
        t = s.copy()
        t.inflight = t.inflight + (("w", t.wal_seq, t.wal_seq, t.live),)
        t.live = ()
        out.append(("SE", t))
    # LA: the oldest seal lands durable: its segment fsynced, the manifest
    if s.inflight:
        t = s.copy()
        kind, ident, end, commits = t.inflight[0]
        t.inflight = t.inflight[1:]
        if kind == "r" and var != "CIPHER":
            setrun(t, ident, synced=len(commits))
        segid = (kind, ident)
        cov, named = t.manifests[-1]
        t.manifests = t.manifests + ((max(cov, end), named | {segid}),)
        t.segs = t.segs + ((segid, commits),)
        out.append(("LA", t))
    # RT: retirement behind the newest manifest
    cov, named = s.manifests[-1]
    if cov > s.retired or any(r[R_PR] and ("r", r[R_ID]) in named for r in s.runs):
        t = s.copy()
        t.retired = max(t.retired, cov)
        for r in t.runs:
            if r[R_PR] and ("r", r[R_ID]) in named:
                setrun(t, r[R_ID], present=False)
        # retired records are gone; the synced prefix counts what is left
        out.append(("RT", t))
    # SY: a sync by policy
    if pol == "EVERYN":
        t = s.copy()
        if var == "LOOSE":
            # A run's sync syncs the WAL first where it holds unsynced
            # frames (all from before the run while one is open); with no
            # run open, closed runs were synced at their close.
            if t.wal_synced < len(t.wal):
                t.wal_synced = len(t.wal)
                out.append(("SY", t))
            elif t.run_open is not None:
                r = getrun(t, t.run_open)
                if r[R_S] < len(r[R_C]):
                    setrun(t, t.run_open, synced=len(r[R_C]))
                    out.append(("SY", t))
        else:
            changed = False
            if var != "CIPHER" and t.run_open is not None:
                r = getrun(t, t.run_open)
                if r[R_S] < len(r[R_C]):
                    setrun(t, t.run_open, synced=len(r[R_C]))
                    changed = True
            elif t.wal_synced < len(t.wal):
                t.wal_synced = len(t.wal)
                changed = True
            if changed:
                out.append(("SY", t))
    return out


def outcomes(s, var):
    """Every (damaged, manifest, wal records, wal synced evidence, temps)."""
    wal_live = [r for r in s.wal if r[0] >= s.retired]
    synced_live = sum(1 for r in s.wal[: s.wal_synced] if r[0] >= s.retired)
    temps = [r for r in s.runs if r[R_PR]]
    # crash: each log keeps its synced prefix and any prefix of the rest
    wal_lens = range(synced_live, len(wal_live) + 1)
    run_lens = [range(r[R_S], len(r[R_C]) + 1) for r in temps]

    def prod(ranges):
        if not ranges:
            yield ()
            return
        for x in ranges[0]:
            for rest in prod(ranges[1:]):
                yield (x,) + rest

    newest = s.manifests[-1]
    for wl in wal_lens:
        for rl in prod(run_lens):
            base_t = {r[R_ID]: (c, min(c, r[R_S])) for r, c in zip(temps, rl)}
            wal_s = min(wl, synced_live)
            yield (None, newest, wal_live[:wl], wal_s, base_t)
            # damage: the WAL cut short
            for w2 in range(0, wl):
                yield ("wal-cut", newest, wal_live[:w2], min(w2, wal_s), base_t)
            # damage: one run temp cut short (an older copy, or a changed
            # byte that ends its walk) or deleted
            for r, c in zip(temps, rl):
                for c2 in list(range(0, c)) + [None]:
                    t2 = dict(base_t)
                    t2[r[R_ID]] = None if c2 is None else (c2, min(c2, base_t[r[R_ID]][1]))
                    yield ("run-del" if c2 is None else "run-cut", newest, wal_live[:wl], wal_s, t2)
            # damage: the newest slot unreadable, the one before it chosen
            if len(s.manifests) >= 2:
                yield ("newest-slot", s.manifests[-2], wal_live[:wl], wal_s, base_t)


def recover(s, var, man, wal, wal_s, temps):
    """REFUSE, or the set of commits the open answers."""
    cov, named = man
    segc = {sid: c for sid, c in s.segs}
    got = set()
    for sid in named:
        got.update(segc[sid])
    floor = set(got)
    rec = {}
    for r in s.runs:
        rid = r[R_ID]
        if ("r", rid) in named:
            continue
        if rid in temps and temps[rid] is not None:
            rec[rid] = temps[rid]
    # WAL replay from the covered sequence
    recs = [x for x in wal if x[0] >= cov]
    if recs and recs[0][0] != cov:
        return "REFUSE", "wal gap"
    reach = cov if not recs else recs[-1][0] + 1
    bindings = []
    for i, x in enumerate(recs):
        if x[1] == "c":
            got.add(x[2])
        else:
            # whether a completed sync of the WAL covers this record
            idx = wal.index(x)
            bindings.append((x, idx < wal_s))
    cut = None   # LOOSE: history ends before this commit

    def end_at(t):
        nonlocal cut
        cut = t if cut is None else min(cut, t)

    if var in ("LIT", "SEQ", "FULL", "LOOSE"):
        for (seq, _, rid, kc, ks), synced_after in bindings:
            have = rec.get(rid)
            n = None if have is None else have[0]
            if var == "LOOSE":
                if n is None or n < ks:
                    return "REFUSE", "binding: run below its synced count"
                if n < kc:
                    if synced_after:
                        return "REFUSE", "binding: run short behind a sync"
                    r = getrun(s, rid)
                    end_at(r[R_C][n])
            else:
                if n is None or n < ks:
                    return "REFUSE", "binding"
    if var in ("FULL", "LOOSE"):
        for r in s.runs:
            rid = r[R_ID]
            if rid not in rec or rec[rid][0] == 0:
                continue
            n, nsync = rec[rid]
            # the WAL before the run
            if max(cov, reach) < r[R_POS]:
                if var == "LOOSE" and nsync == 0:
                    end_at(r[R_C][0])
                else:
                    return "REFUSE", "predecessor: WAL short of the run's open"
            # the run before it, with no WAL commit between
            if r[R_PREV] is not None:
                pid, kc, ks, pend = r[R_PREV]
                if cov >= pend:
                    pass
                else:
                    have = rec.get(pid)
                    m = None if have is None else have[0]
                    if var == "LOOSE":
                        if m is None or m < ks:
                            return "REFUSE", "previous run below its synced count"
                        if m < kc:
                            if nsync > 0:
                                return "REFUSE", "previous run short behind a sync"
                            end_at(getrun(s, pid)[R_C][m])
                    elif m is None or m < kc:
                        return "REFUSE", "previous run short"
    for rid, (n, _) in rec.items():
        got.update(getrun(s, rid)[R_C][:n])
    if cut is not None:
        assert all(c < cut for c in floor), "a cut through a named segment"
        got = {c for c in got if c < cut}
    return "OPEN", got


def durable(s, var):
    d = set()
    for sid, c in s.segs:
        d.update(c)
    for x in s.wal[: s.wal_synced]:
        if x[1] == "c" and x[0] >= s.retired:
            d.add(x[2])
    if var != "CIPHER":
        for r in s.runs:
            d.update(r[R_C][: r[R_S]])
    return d


def main():
    var, pol, depth = sys.argv[1], sys.argv[2], int(sys.argv[3])
    maxruns = int(sys.argv[4]) if len(sys.argv) > 4 else 2
    maxn = int(sys.argv[5]) if len(sys.argv) > 5 else 5
    assert var in VARIANTS and pol in ("ALWAYS", "EVERYN")
    seen = {}
    q = deque([(initial(), ())])
    seen[initial().key()] = ()
    counts = {"never": 0, "false_refusal": 0, "lost_durable": 0,
              "refused": 0, "rollback": 0, "whole": 0}
    witness = {}
    states = 0
    while q:
        s, path = q.popleft()
        states += 1
        whole_set = set(range(1, s.n + 1))
        dur = durable(s, var)
        for damaged, man, wal, wal_s, temps in outcomes(s, var):
            res, got = recover(s, var, man, wal, wal_s, temps)
            if res == "REFUSE":
                if damaged:
                    counts["refused"] += 1
                else:
                    counts["false_refusal"] += 1
                    witness.setdefault("false_refusal", (path, got, man, wal, temps))
                continue
            j = len(got)
            if got != set(range(1, j + 1)):
                counts["never"] += 1
                lab = damaged or "no-damage"
                counts.setdefault("never_by", {})
                counts["never_by"][lab] = counts["never_by"].get(lab, 0) + 1
                witness.setdefault("never/" + lab, (path, sorted(got), man, wal, temps))
                continue
            if not damaged and not dur <= got:
                counts["lost_durable"] += 1
                witness.setdefault("lost_durable", (path, sorted(got), man, wal, temps))
                continue
            if got == whole_set:
                counts["whole"] += 1
            else:
                counts["rollback"] += 1
                if damaged and not dur <= got:
                    counts.setdefault("rollback_losing_durable_by", {})
                    d = counts["rollback_losing_durable_by"]
                    d[damaged] = d.get(damaged, 0) + 1
                    witness.setdefault("rollback_losing_durable/" + damaged, (path, sorted(got), man, wal, temps))
        if len(path) < depth:
            for name, t in events(s, var, pol, maxn, maxruns):
                k = t.key()
                if k not in seen:
                    seen[k] = path + (name,)
                    q.append((t, path + (name,)))
    print(f"{var} {pol} depth={depth} maxruns={maxruns} states={states} {counts}")
    for k, (path, got, man, wal, temps) in sorted(witness.items()):
        print(f"  {k}: events={' '.join(path)} -> {got}; manifest(cov={man[0]}, "
              f"named={sorted(man[1])}); wal={[(x[0], x[1], x[2]) for x in wal]}; temps={temps}")


if __name__ == "__main__":
    main()
