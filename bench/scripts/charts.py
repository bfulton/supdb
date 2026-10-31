#!/usr/bin/env python3
"""charts.py ROW OUT [--companion ROW] [--ooc TITLE=SITTING.out,SITTING.out ...]

The suite's figures from one row of `bench run` (its JSON, written under
`bench/runs/<scale>/` or wherever `--out` put it), into the directory OUT:
every figure as a PNG, a PDF and an SVG, a dark PNG under OUT/dark/, every
cell's samples in OUT/cells.csv, and OUT/index.html with every figure and a
table view of each.

The statistic is the suite's own (`bench/src/stats.rs`): the median, and a
95% bootstrap interval of it from 2000 resamples seeded from the samples'
bits, ported here bit for bit so that a cell prints what `bench` would.

Arms: supdb, lmdb, rocksdb-tuned are the durable three; supdb-ingest,
lmdb-nosync, rocksdb-tuned-nosync the buffered three (a row whose buffered
RocksDB is the untuned rocksdb-nosync is drawn without it). A row without one of them
leaves its lines out. The published set is drawn from a full row with
`--companion` a quick row, both of those six arms on an otherwise idle
machine, the quick one first:

    A=supdb,lmdb,rocksdb-tuned,supdb-ingest,lmdb-nosync,rocksdb-tuned-nosync
    bench run --scale quick --arms $A --out OUT
    bench run --scale full --top 3000000 --arms $A --out OUT

`--ooc` adds the out-of-core figure from `bench ab` tables (two arms
paired under `--bound`, one sitting in each order), one argument a
comparator: a title, then the sittings' tables as `bench ab` prints them
to stdout, saved one sitting a file as `NAME.out`. The `.err` beside each, where it carries the
controller's trace, gives the install's delay after the load per rep.
"""
import argparse
import csv
import html
import json
import math
import os
import re
import struct
import textwrap

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib.colors import LinearSegmentedColormap, to_rgb  # noqa: E402
from matplotlib.lines import Line2D  # noqa: E402
from matplotlib.patches import Rectangle  # noqa: E402
from matplotlib.ticker import MaxNLocator, NullLocator  # noqa: E402

# ---------------------------------------------------------------- statistic

MASK = (1 << 64) - 1


def _bits(v):
    return struct.unpack("<Q", struct.pack("<d", v))[0]


def quantile(vals, q):
    v = sorted(vals)
    if not v:
        return math.nan
    pos = min(max(q, 0.0), 1.0) * (len(v) - 1)
    lo, hi = math.floor(pos), math.ceil(pos)
    return v[lo] + (v[hi] - v[lo]) * (pos - lo)


def median_ci(vals, conf=0.95, resamples=2000):
    """`Samples::median_ci`, bit for bit."""
    n = len(vals)
    if n < 2:
        m = quantile(vals, 0.5)
        return m, m
    seed = 0x9E3779B97F4A7C15
    for v in vals:
        seed ^= _bits(v)
        seed = (seed * 0x100000001B3) & MASK
        seed = ((seed << 27) | (seed >> 37)) & MASK
    meds = []
    buf = [0.0] * n
    for _ in range(resamples):
        for i in range(n):
            seed ^= (seed << 13) & MASK
            seed ^= seed >> 7
            seed ^= (seed << 17) & MASK
            buf[i] = vals[seed % n]
        s = sorted(buf)
        meds.append(s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2)
    tail = (1 - conf) / 2
    return quantile(meds, tail), quantile(meds, 1 - tail)


# ---------------------------------------------------------------- the row

# Each engine and guarantee is drawn from one arm: the first of its
# candidates the row holds -- the roster's (LMDB as it opens, which in
# memory is how its documentation configures it; RocksDB tuned in both
# groups), then a bounded row's LMDB with its readahead off.
SLOTS = {
    ("supdb", "durable"): ["supdb"],
    ("LMDB", "durable"): ["lmdb", "lmdb-nordahead"],
    ("RocksDB", "durable"): ["rocksdb-tuned"],
    ("supdb", "buffered"): ["supdb-ingest"],
    ("LMDB", "buffered"): ["lmdb-nosync"],
    ("RocksDB", "buffered"): ["rocksdb-tuned-nosync"],
}
# A row whose only buffered RocksDB arm is RocksDB as it ships is drawn
# without it: ROADMAP.md quotes no buffered RocksDB number until the arm is
# tuned, and `rocksdb-nosync` is not.
ARM_TEXT = {
    "lmdb": "LMDB as it opens by default",
    "lmdb-nordahead": "LMDB with readahead off",
    "lmdb-nosync": "LMDB nosync",
    "rocksdb-tuned": "RocksDB tuned (256 MB block cache, Bloom filters, bulk-load write buffers)",
    "rocksdb-tuned-nosync": "RocksDB with the same tuning, nosync",
    "rocksdb-nosync": "RocksDB nosync as it ships (untuned)",
}
ENGINES = ["supdb", "LMDB", "RocksDB"]
MARKERS = {"supdb": "o", "LMDB": "s", "RocksDB": "^"}
LOWER = {"p99_us", "bytes_on_disk_per_byte", "device_bytes_per_byte"}


def load_row(path):
    row = json.load(open(path))
    present = {m["arm"] for m in row["measurements"]}
    arms = {}
    for slot, cands in SLOTS.items():
        pick = next((a for a in cands if a in present), None)
        if pick:
            arms[pick] = slot
    row["_arms"] = {slot: arm for arm, slot in arms.items()}
    row["_untuned"] = "rocksdb-nosync" in present and ("RocksDB", "buffered") not in row["_arms"]
    cells = {}
    for m in row["measurements"]:
        a = arms.get(m["arm"])
        if not a or not m["samples"]:
            continue
        eng, g = a
        s = [float(x) for x in m["samples"]]
        lo, hi = median_ci(s)
        cells[(m["workload"], m["quantity"], g, eng, m["size"])] = dict(
            arm=m["arm"], unit=m["unit"], samples=s, med=quantile(s, 0.5), lo=lo, hi=hi
        )
    sizes = sorted({k[4] for k in cells})
    return row, cells, sizes


def size_label(s):
    if s >= 1_000_000 and s % 1_000_000 == 0:
        return f"{s // 1_000_000}M"
    if s >= 1000 and s % 1000 == 0:
        return f"{s // 1000}k"
    return str(s)


def fmt3(v):
    """Three significant figures with K and M, as the old tables print."""
    if v != v:
        return "–"
    for div, suf in ((1e9, "G"), (1e6, "M"), (1e3, "K"), (1, "")):
        if abs(v) >= div or div == 1:
            s = f"{v / div:.3g}"
            if "e" in s:  # 999.7K rounds to 1e+03K
                continue
            return s + suf
    return f"{v:.3g}"


def fmt_ratio(r):
    if r != r:
        return "–"
    if r >= 100:
        return f"{r:.0f}×"
    return f"{r:.2g}×"


# ---------------------------------------------------------------- themes

THEMES = {
    "light": dict(
        surface="#fcfcfb",
        plot="#f6f6f3",
        ink="#0b0b0b",
        ink2="#52514e",
        muted="#898781",
        grid="#e1e0d9",
        axis="#c3c2b7",
        series={"supdb": "#2a78d6", "LMDB": "#eb6834", "RocksDB": "#1baf7a"},
        neg=["#7a1e1e", "#b02e2d", "#e34948", "#ee8f8e", "#f6cfce"],
        mid="#f0efec",
        pos=["#cde2fb", "#86b6ef", "#3987e5", "#1c5cab", "#0d366b"],
    ),
    "dark": dict(
        surface="#1a1a19",
        plot="#151514",
        ink="#f0efec",
        ink2="#c3c2b7",
        muted="#898781",
        grid="#2c2c2a",
        axis="#383835",
        series={"supdb": "#3987e5", "LMDB": "#d95926", "RocksDB": "#199e70"},
        neg=["#f2a3a3", "#e66767", "#b43f3e", "#7a3332", "#4d3332"],
        mid="#383835",
        pos=["#2f3f55", "#24548f", "#2a6fc4", "#3987e5", "#86b6ef"],
    ),
}

# The diverging scale's domain, log2 of the ratio: a quarter to sixteen times.
DIV_LO, DIV_HI = -2.0, 4.0


def div_color(T, r):
    x = math.log2(r)
    if x >= 0:
        cm = LinearSegmentedColormap.from_list("p", [T["mid"]] + T["pos"])
        return cm(min(x / DIV_HI, 1.0))
    cm = LinearSegmentedColormap.from_list("n", [T["mid"]] + T["neg"][::-1])
    return cm(min(x / DIV_LO, 1.0))


def luminance(c):
    def lin(u):
        return u / 12.92 if u <= 0.03928 else ((u + 0.055) / 1.055) ** 2.4

    r, g, b = to_rgb(c)
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def style(T):
    plt.rcParams.update(
        {
            "font.family": ["Inter", "DejaVu Sans"],
            "font.size": 10,
            "axes.unicode_minus": False,
            "svg.fonttype": "path",
            "pdf.fonttype": 42,
            "figure.facecolor": T["surface"],
            "savefig.facecolor": T["surface"],
            "text.color": T["ink"],
        }
    )


# ---------------------------------------------------------------- axes


def axis_unit(vmax):
    if vmax >= 1e6:
        return 1e6, "M"
    if vmax >= 1e4:
        return 1e3, "K"
    return 1, ""


def tick_text(v, div, suf):
    if v == 0:
        return "0"
    s = f"{v / div:.3f}".rstrip("0").rstrip(".")
    return s + suf


def ylimits(ax, vmax):
    top = vmax * 1.08 if vmax > 0 else 1
    ticks = MaxNLocator(nbins=4, steps=[1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]).tick_values(0, top)
    ticks = [t for t in ticks if 0 <= t <= top * 1.0001]
    ax.set_ylim(0, top)
    div, suf = axis_unit(max(ticks))
    ax.set_yticks(ticks)
    ax.set_yticklabels([tick_text(t, div, suf) for t in ticks])


def end_labels(ax, ends, T, fs=9.5):
    """Labels at the lines' right ends, spread where they would collide,
    with a leader to the end each one names once it has moved."""
    fig = ax.figure
    pt = fig.dpi / 72
    gap = fs * 1.25 * pt
    items = sorted(
        [(ax.transData.transform((x, y))[1], eng, x, y) for eng, x, y in ends],
        key=lambda t: t[0],
    )
    # Clusters placed at the mean of their targets, merged while they overlap.
    clusters = [[i] for i in range(len(items))]

    def place(c):
        mean = sum(items[i][0] for i in c) / len(c)
        start = mean - gap * (len(c) - 1) / 2
        return [start + gap * k for k in range(len(c))]

    while True:
        pos = [place(c) for c in clusters]
        merged = False
        for k in range(len(clusters) - 1):
            if pos[k + 1][0] - pos[k][-1] < gap - 1e-9:
                clusters[k] = clusters[k] + clusters[k + 1]
                del clusters[k + 1]
                merged = True
                break
        if not merged:
            break
    ybottom = ax.transAxes.transform((0, 0))[1] - 2 * pt
    flat = [p for ps in pos for p in ps]
    lift = max(0.0, ybottom - flat[0]) if flat else 0.0
    for c, ps in zip(clusters, pos):
        for i, p in zip(c, ps):
            target, eng, x, y = items[i]
            dy = (p + lift - target) / pt
            moved = abs(dy) > 2.5
            ax.annotate(
                eng,
                xy=(x, y),
                xytext=(10, dy),
                textcoords="offset points",
                ha="left",
                va="center",
                fontsize=fs,
                color=T["ink2"],
                annotation_clip=False,
                arrowprops=(
                    dict(arrowstyle="-", color=T["muted"], lw=0.7, shrinkA=1, shrinkB=5)
                    if moved
                    else None
                ),
            )


def line_panel(ax, T, cells, spec, sizes, sparse_x):
    wl, q, g = spec["workload"], spec["quantity"], spec["guarantee"]
    ax.set_facecolor(T["plot"])
    for sp in ("top", "right"):
        ax.spines[sp].set_visible(False)
    for sp in ("left", "bottom"):
        ax.spines[sp].set_color(T["axis"])
        ax.spines[sp].set_linewidth(1)
    ax.set_xscale("log")
    ax.xaxis.set_minor_locator(NullLocator())
    ax.set_xticks(sizes)
    labels = [size_label(s) for s in sizes]
    if sparse_x:
        keep = {sizes[0], sizes[-1]} | {s for s in sizes if str(s).startswith("1")}
        labels = [lab if s in keep else "" for s, lab in zip(sizes, labels)]
    ax.set_xticklabels(labels)
    ax.set_xlim(sizes[0] / 1.35, sizes[-1] * 1.35)
    ax.tick_params(colors=T["muted"], labelsize=9.5, length=3, width=0.8)
    ax.tick_params(axis="y", length=0, pad=4)
    ax.yaxis.grid(True, color=T["grid"], lw=0.9)
    ax.set_axisbelow(True)
    vmax, ends = 0.0, []
    for eng in ["RocksDB", "LMDB", "supdb"]:
        pts = [(s, cells.get((wl, q, g, eng, s))) for s in sizes]
        pts = [(s, c) for s, c in pts if c]
        if not pts:
            continue
        xs = [s for s, _ in pts]
        med = [c["med"] for _, c in pts]
        lo = [c["lo"] for _, c in pts]
        hi = [c["hi"] for _, c in pts]
        col = T["series"][eng]
        ax.errorbar(
            xs,
            med,
            yerr=[[m - v for m, v in zip(med, lo)], [v - m for m, v in zip(med, hi)]],
            fmt="none",
            ecolor=col,
            elinewidth=1.3,
            capsize=3,
            capthick=1.3,
            zorder=2,
        )
        ax.plot(
            xs,
            med,
            color=col,
            lw=2.2,
            marker=MARKERS[eng],
            ms=7.5,
            mec=T["surface"],
            mew=1.4,
            zorder=3,
            solid_joinstyle="round",
            solid_capstyle="round",
        )
        vmax = max(vmax, max(hi), max(med))
        ends.append((eng, xs[-1], med[-1]))
    ylimits(ax, vmax)
    end_labels(ax, ends, T)


def legend(fig, T, y):
    handles = [
        Line2D(
            [0],
            [0],
            color=T["series"][e],
            lw=2.2,
            marker=MARKERS[e],
            ms=7.5,
            mec=T["surface"],
            mew=1.4,
        )
        for e in ENGINES
    ]
    leg = fig.legend(
        handles,
        ENGINES,
        loc="upper right",
        bbox_to_anchor=(0.995, y),
        ncol=3,
        frameon=False,
        fontsize=11,
        handlelength=2.6,
        columnspacing=1.6,
    )
    for t in leg.get_texts():
        t.set_color(T["ink"])


def footer(fig, T, text, width_in):
    chars = int(width_in * 72 / (8.5 * 0.5))
    fig.text(
        0.008,
        0.008,
        textwrap.fill(text, chars),
        fontsize=8.5,
        color=T["muted"],
        ha="left",
        va="bottom",
        linespacing=1.35,
    )


def lines_figure(T, cells, sizes, fig_spec, foot):
    grid = fig_spec["grid"]
    nrows, ncols = len(grid), max(len(r) for r in grid)
    w = {2: 9.0, 3: 10.6, 4: 11.6}[ncols]
    h = 2.95 * nrows + 1.6
    fig = plt.figure(figsize=(w, h), dpi=200)
    top = 1 - 1.15 / h
    bottom = 1.05 / h
    gs = fig.add_gridspec(
        nrows,
        ncols,
        left=0.105 if ncols == 2 else 0.085,
        right=0.885 if ncols == 2 else 0.9,
        top=top,
        bottom=bottom,
        wspace={2: 0.55, 3: 0.6, 4: 0.72}[ncols],
        hspace=0.42,
    )
    fig.text(0.008, 1 - 0.18 / h, fig_spec["title"], fontsize=17, weight="bold", va="top", color=T["ink"])
    legend(fig, T, 1 - 0.1 / h)
    for r, row in enumerate(grid):
        for c, spec in enumerate(row):
            ax = fig.add_subplot(gs[r, c])
            line_panel(ax, T, cells, spec, sizes, sparse_x=ncols >= 4)
            sub = spec.get("sub", "")
            lower = spec["quantity"] in LOWER
            if lower:
                sub = (sub + "  ·  " if sub else "") + "lower is better"
            if spec.get("title"):
                ax.text(0, 1.105, spec["title"], transform=ax.transAxes, fontsize=13, weight="bold", va="bottom", color=T["ink"])
            if sub:
                ax.text(0, 1.025, sub, transform=ax.transAxes, fontsize=9.5, va="bottom", color=T["muted"])
            if c == 0 or spec.get("ylabel_each"):
                ax.set_ylabel(spec["ylabel"], color=T["ink2"], fontsize=11, labelpad=6)
            if r == nrows - 1:
                ax.set_xlabel("keys in store", color=T["ink2"], fontsize=11, labelpad=6)
    footer(fig, T, foot, w)
    return fig


# ---------------------------------------------------------------- summary

SUMMARY_ROWS = [
    ("Ordered load", "load", "ops_per_s"),
    ("Shuffled load", "load-shuffled", "ops_per_s"),
    ("Point reads", "read", "reads_per_s"),
    ("Point reads, 3 threads", "read", "reads_per_s_3t"),
    ("Read p99 latency", "read", "p99_us"),
    ("Scans", "scan", "entries_per_s"),
    ("Scans, 3 threads", "scan", "entries_per_s_3t"),
    ("Scans after the mixes, 3 thr.", "scan-mixed", "entries_per_s_3t"),
    ("Scans after a 1% burst", "scan-lag", "entries_per_s_lag1pct"),
    ("Scans after a 10% burst", "scan-lag", "entries_per_s_lag10pct"),
    ("Scans after a 100% burst", "scan-lag", "entries_per_s_lag100pct"),
    ("The 100% burst itself", "scan-lag", "updates_per_s_lag100pct"),
    ("YCSB-A", "ycsb-A", "ops_per_s"),
    ("YCSB-B", "ycsb-B", "ops_per_s"),
    ("YCSB-C", "ycsb-C", "ops_per_s"),
    ("YCSB-D", "ycsb-D", "ops_per_s"),
    ("YCSB-E", "ycsb-E", "ops_per_s"),
    ("YCSB-F", "ycsb-F", "ops_per_s"),
    ("Bytes on disk per byte", "load", "bytes_on_disk_per_byte"),
    ("Device bytes per byte", "load", "device_bytes_per_byte"),
]
BLOCKS = [("durable", "LMDB"), ("durable", "RocksDB"), ("buffered", "LMDB"), ("buffered", "RocksDB")]


def advantage(cells, wl, q, g, comp, s):
    """supdb's advantage, inverted for a lower-is-better quantity, and
    whether the two intervals miss each other."""
    a = cells.get((wl, q, g, "supdb", s))
    b = cells.get((wl, q, g, comp, s))
    if not a or not b or not b["med"] or not a["med"]:
        return math.nan, False
    r = a["med"] / b["med"]
    if q in LOWER:
        r = 1 / r
    return r, a["lo"] > b["hi"] or a["hi"] < b["lo"]


def summary_figure(T, cells, sizes, foot, title):
    nr, ns = len(SUMMARY_ROWS), len(sizes)
    blocks = [
        (g, comp)
        for g, comp in BLOCKS
        if any(cells.get((wl, q, g, comp, s)) for _, wl, q in SUMMARY_ROWS for s in sizes)
    ]
    cell_w, label_w, gap_w = 0.44, 2.45, 0.22
    w = label_w + len(blocks) * (ns * cell_w + gap_w) + 0.1
    cell_h = 0.325
    h = nr * cell_h + 2.45
    fig = plt.figure(figsize=(w, h), dpi=200)
    left, bw, gapb = label_w / w, ns * cell_w / w, gap_w / w
    bottom = 1.15 / h
    height = nr * cell_h / h
    fig.text(
        0.008,
        1 - 0.15 / h,
        title,
        fontsize=16,
        weight="bold",
        va="top",
        color=T["ink"],
    )
    fig.text(
        0.008,
        1 - 0.5 / h,
        "×, above 1 means supdb is faster or smaller; bold where the two 95% CIs do not overlap",
        fontsize=10.5,
        va="top",
        color=T["ink2"],
    )
    for bi, (g, comp) in enumerate(blocks):
        ax = fig.add_axes([left + bi * (bw + gapb), bottom, bw, height])
        ax.set_xlim(0, ns)
        ax.set_ylim(nr, 0)
        ax.axis("off")
        ax.text(ns / 2, -0.35, f"{g.capitalize()} · vs {comp}", ha="center", va="bottom", fontsize=11.5, weight="bold", color=T["ink"])
        for ri, (_, wl, q) in enumerate(SUMMARY_ROWS):
            for si, s in enumerate(sizes):
                r, sig = advantage(cells, wl, q, g, comp, s)
                if r != r:
                    continue
                col = div_color(T, r)
                ax.add_patch(Rectangle((si + 0.04, ri + 0.06), 0.92, 0.88, facecolor=col, lw=0))
                txt = "#ffffff" if luminance(col) < 0.33 else "#0b0b0b"
                ax.text(
                    si + 0.5,
                    ri + 0.52,
                    fmt_ratio(r),
                    ha="center",
                    va="center",
                    fontsize=8.3 if r < 10 else 8.6,
                    weight="bold" if sig else "normal",
                    color=txt,
                    alpha=1.0 if sig else 0.62,
                )
        for si, s in enumerate(sizes):
            ax.text(si + 0.5, nr + 0.25, size_label(s), ha="center", va="top", fontsize=9.5, color=T["muted"])
        if bi == 0:
            for ri, (lab, _, _) in enumerate(SUMMARY_ROWS):
                ax.text(-0.18, ri + 0.52, lab, ha="right", va="center", fontsize=10.5, color=T["ink2"])
    footer(
        fig,
        T,
        "Ratio of medians, inverted for lower-is-better quantities so above 1 always favours supdb. "
        "Bold: the two 95% CIs do not overlap; faint: they do. The last two rows are space. " + foot,
        w,
    )
    return fig


# ---------------------------------------------------------------- out of core

OOC_ROWS = [
    ("Ordered load", "load", "ops_per_s"),
    ("Shuffled load", "load-shuffled", "ops_per_s"),
    ("Point reads", "read", "reads_per_s"),
    ("Point reads, 2 threads", "read", "reads_per_s_2t"),
    ("Point reads, 3 threads", "read", "reads_per_s_3t"),
    ("Read p99 latency", "read", "p99_us"),
    ("Scans", "scan", "entries_per_s"),
    ("Scans, 2 threads", "scan", "entries_per_s_2t"),
    ("Scans, 3 threads", "scan", "entries_per_s_3t"),
    ("YCSB-A", "ycsb-A", "ops_per_s"),
    ("YCSB-B", "ycsb-B", "ops_per_s"),
    ("YCSB-C", "ycsb-C", "ops_per_s"),
    ("YCSB-D", "ycsb-D", "ops_per_s"),
    ("YCSB-E", "ycsb-E", "ops_per_s"),
    ("YCSB-F", "ycsb-F", "ops_per_s"),
]


def parse_ab(path):
    """A `bench ab` table: each arm's median of the measured reps by
    quantity, and the arms in their column order."""
    lines = open(path).read().splitlines()
    hdr = next(line for line in lines if line.startswith("quantity"))
    arms = hdr.split()[1:3]
    vals = {}
    for line in lines:
        p = line.split()
        if len(p) >= 5 and p[4].endswith("x") and p[0] != "quantity":
            try:
                vals[(p[0], p[1])] = {arms[0]: float(p[2]), arms[1]: float(p[3])}
            except ValueError:
                pass
    return arms, vals


def installs(err_path, arm):
    """Seconds from each rep's load end to the controller's install, for
    the reps of `arm`, the warmup first."""
    out, rep = [], None
    if not os.path.exists(err_path):
        return out
    for line in open(err_path):
        p = line.split()
        if len(p) < 2:
            continue
        if p[1] == "phasemark" and " load start " in line:
            rep = {"arm": p[2], "le": None, "inst": None}
            out.append(rep)
        elif rep is None:
            continue
        elif p[1] == "phasemark" and " load end " in line:
            rep["le"] = float(p[-1])
        elif p[1] == "zselect" and rep["le"] is not None and rep["inst"] is None and "Some(Install)" in line:
            rep["inst"] = float(p[0]) - rep["le"]
    # A rep opens two stores, the ordered load's (whose passes the
    # controller serves) and the shuffled load's; the first is the rep's.
    return [r["inst"] for r in out if r["arm"] == arm][::2]


def load_ooc(specs):
    groups = []
    for spec in specs:
        title, paths = spec.split("=", 1)
        sittings = []
        for path in paths.split(","):
            arms, vals = parse_ab(path)
            sup = next(a for a in arms if a.startswith("supdb"))
            comp = next(a for a in arms if a != sup)
            head = path.rsplit("-", 1)[0] + "-head.txt"
            sha = open(head).read().split()[0] if os.path.exists(head) else "?"
            sittings.append(
                dict(
                    path=path,
                    first=arms[0],
                    sup=sup,
                    comp=comp,
                    vals=vals,
                    sha=sha,
                    inst=installs(path[: -len(".out")] + ".err", sup),
                )
            )
        groups.append(dict(title=title, sittings=sittings))
    return groups


def ooc_ratio(sit, wl, q):
    v = sit["vals"].get((wl, q))
    if not v or not v[sit["comp"]] or not v[sit["sup"]]:
        return math.nan
    r = v[sit["sup"]] / v[sit["comp"]]
    return 1 / r if q in LOWER else r


def ooc_figure(T, groups, foot):
    n = len(OOC_ROWS)
    ng = len(groups)
    w = 11.0 if ng > 1 else 7.6
    h = n * 0.36 + 3.15
    fig = plt.figure(figsize=(w, h), dpi=200)
    fig.text(0.008, 1 - 0.15 / h, "Out of core: 5M keys under 640 MiB", fontsize=17, weight="bold", va="top", color=T["ink"])
    fig.text(
        0.008,
        1 - 0.55 / h,
        "supdb's advantage over LMDB per cell, one mark per sitting (the median of its three pairs)",
        fontsize=10.5,
        va="top",
        color=T["ink2"],
    )
    left = 0.2 if ng > 1 else 0.27
    span = 0.97 - left
    gap = 0.06
    pw = (span - gap * (ng - 1)) / ng - 0.07
    bottom, height = 1.45 / h, n * 0.36 / h
    marks = ["o", "D", "s", "^"]
    allr = [ooc_ratio(s, wl, q) for g in groups for s in g["sittings"] for _, wl, q in OOC_ROWS]
    allr = [r for r in allr if r == r and r > 0]
    lo = min(0.5, min(allr) / 1.25)
    hi = max(allr) * 1.3
    for gi, g in enumerate(groups):
        ax = fig.add_axes([left + gi * (pw + gap + 0.07), bottom, pw, height])
        ax.set_facecolor(T["plot"])
        ax.set_xscale("log")
        ax.set_xlim(lo, hi)
        ax.set_ylim(n - 0.4, -0.6)
        for sp in ("top", "right", "left"):
            ax.spines[sp].set_visible(False)
        ax.spines["bottom"].set_color(T["axis"])
        ticks = [t for t in (0.25, 0.5, 1, 2, 4, 8, 16, 32) if lo <= t <= hi]
        ax.set_xticks(ticks)
        ax.set_xticklabels([f"{t:g}×" for t in ticks])
        ax.xaxis.set_minor_locator(NullLocator())
        ax.tick_params(colors=T["muted"], labelsize=9.5, length=3)
        ax.set_yticks([])
        ax.xaxis.grid(True, color=T["grid"], lw=0.9)
        ax.set_axisbelow(True)
        ax.axvline(1, color=T["axis"], lw=1.4, zorder=1)
        ax.text(1, -0.75, "level", ha="center", va="bottom", fontsize=9, color=T["muted"])
        ax.text(0, 1.06, g["title"], transform=ax.transAxes, fontsize=13, weight="bold", va="bottom", color=T["ink"])
        for ri, (lab, wl, q) in enumerate(OOC_ROWS):
            ax.axhline(ri, color=T["grid"], lw=0.6, zorder=0)
            rs = [ooc_ratio(s, wl, q) for s in g["sittings"]]
            rs_ok = [r for r in rs if r == r]
            if len(rs_ok) > 1:
                ax.plot([min(rs_ok), max(rs_ok)], [ri, ri], color=T["series"]["supdb"], lw=2, alpha=0.35, zorder=2, solid_capstyle="round")
            for si, r in enumerate(rs):
                if r != r:
                    continue
                ax.plot(
                    [r],
                    [ri],
                    marker=marks[si % len(marks)],
                    ms=7.5,
                    color=T["series"]["supdb"] if si == 0 else T["surface"],
                    mec=T["series"]["supdb"],
                    mew=1.6 if si else 1.4,
                    zorder=3,
                    ls="none",
                )
            if rs_ok:
                a, b = min(rs_ok), max(rs_ok)
                txt = fmt_ratio(a) if fmt_ratio(a) == fmt_ratio(b) else f"{fmt_ratio(a)[:-1]}–{fmt_ratio(b)}"
                ax.text(1.02, ri, txt, transform=ax.get_yaxis_transform(), ha="left", va="center", fontsize=9.5, color=T["ink2"])
            if gi == 0:
                ax.text(-0.03, ri, lab, transform=ax.get_yaxis_transform(), ha="right", va="center", fontsize=10.5, color=T["ink2"])
        ax.set_xlabel("supdb over LMDB (log scale; p99 inverted, so right is better everywhere)", color=T["ink2"], fontsize=9.5, labelpad=6)
    sittings = groups[0]["sittings"]
    handles = [
        Line2D(
            [0],
            [0],
            marker=marks[i % len(marks)],
            ms=7.5,
            ls="none",
            color=T["series"]["supdb"] if i == 0 else T["surface"],
            mec=T["series"]["supdb"],
            mew=1.6 if i else 1.4,
        )
        for i in range(len(sittings))
    ]
    labels = [("sitting %d: %s first" % (i + 1, "LMDB" if s["first"] == s["comp"] else "supdb")) for i, s in enumerate(sittings)]
    leg = fig.legend(handles, labels, loc="upper left", bbox_to_anchor=(0.002, 1 - 0.8 / h), ncol=len(labels), frameon=False, fontsize=10)
    for t in leg.get_texts():
        t.set_color(T["ink"])
    footer(fig, T, foot, w)
    return fig


# ---------------------------------------------------------------- figures


def two_rows(cols, ylabel):
    """A figure's grid: the durable arms over the buffered, one column a
    quantity."""
    grid = []
    for g in ("durable", "buffered"):
        row = []
        for col in cols:
            spec = dict(col)
            spec["guarantee"] = g
            spec["ylabel"] = f"{g.capitalize()}\n{col.get('ylabel', ylabel)}"
            if g == "buffered":
                spec["title"] = ""
                spec["sub"] = ""
            if col.get("ylabel") and col is not cols[0]:
                spec["ylabel"] = col["ylabel"]
                spec["ylabel_each"] = True
            row.append(spec)
        grid.append(row)
    return grid


YCSB_SUB = {
    "A": "50/50 read/update, zipfian",
    "B": "95/5 read/update, zipfian",
    "C": "read-only, zipfian",
    "D": "95/5 read/insert, read latest",
    "E": "95/5 scan/insert, scans of 50",
    "F": "read-modify-write, zipfian",
}


def ycsb_grid(g):
    rows = []
    for letters in ("ABC", "DEF"):
        rows.append(
            [
                dict(
                    workload=f"ycsb-{x}",
                    quantity="ops_per_s",
                    guarantee=g,
                    title=f"YCSB-{x}",
                    sub=YCSB_SUB[x],
                    ylabel="operations / s",
                )
                for x in letters
            ]
        )
    return rows


FIGURES = [
    dict(
        name="fig1-loads",
        title="Loads",
        grid=two_rows(
            [
                dict(workload="load", quantity="ops_per_s", title="Ordered load", sub="keys arrive in order"),
                dict(workload="load-shuffled", quantity="ops_per_s", title="Shuffled load", sub="keys arrive shuffled"),
            ],
            "operations / s",
        ),
    ),
    dict(
        name="fig2-reads",
        title="Point reads",
        grid=two_rows(
            [
                dict(workload="read", quantity="reads_per_s", title="1 thread", sub="point reads, uniform keys"),
                dict(workload="read", quantity="reads_per_s_2t", title="2 threads", sub="point reads"),
                dict(workload="read", quantity="reads_per_s_3t", title="3 threads", sub="point reads"),
                dict(workload="read", quantity="p99_us", title="Read latency", sub="p99, 1 thread", ylabel="µs"),
            ],
            "reads / s",
        ),
    ),
    dict(
        name="fig3-scans",
        title="Ordered scans",
        grid=two_rows(
            [
                dict(workload="scan", quantity="entries_per_s", title="1 thread", sub="scans of 100 entries"),
                dict(workload="scan", quantity="entries_per_s_2t", title="2 threads", sub="scans of 100 entries"),
                dict(workload="scan", quantity="entries_per_s_3t", title="3 threads", sub="scans of 100 entries"),
            ],
            "entries / s",
        ),
    ),
    dict(
        name="fig3b-scans-after-mixes",
        title="Scans on the store the YCSB mixes leave",
        grid=two_rows(
            [
                dict(workload="scan-mixed", quantity="entries_per_s_2t", title="2 threads", sub="after the six mixes"),
                dict(workload="scan-mixed", quantity="entries_per_s_3t", title="3 threads", sub="after the six mixes"),
            ],
            "entries / s",
        ),
    ),
    dict(
        name="fig4-scan-lag",
        title="Scans against merge lag",
        grid=two_rows(
            [
                dict(workload="scan-lag", quantity="entries_per_s_lag0pct", title="Scans, drained", sub="the store just loaded"),
                dict(workload="scan-lag", quantity="entries_per_s_lag1pct", title="After a 1% burst", sub="before the engine catches up"),
                dict(workload="scan-lag", quantity="entries_per_s_lag10pct", title="After a 10% burst", sub="of updates"),
                dict(workload="scan-lag", quantity="entries_per_s_lag100pct", title="After a 100% burst", sub="every key rewritten"),
            ],
            "entries / s",
        ),
    ),
    dict(
        name="fig4b-bursts",
        title="The update bursts of the lag sweep",
        grid=two_rows(
            [
                dict(workload="scan-lag", quantity="updates_per_s_lag1pct", title="The 1% burst", sub="1% of the keys rewritten"),
                dict(workload="scan-lag", quantity="updates_per_s_lag10pct", title="The 10% burst", sub="10% of the keys"),
                dict(workload="scan-lag", quantity="updates_per_s_lag100pct", title="The 100% burst", sub="every key"),
            ],
            "updates / s",
        ),
    ),
    dict(name="fig5a-ycsb-durable", title="YCSB mixes, durable commits", grid=ycsb_grid("durable")),
    dict(name="fig5b-ycsb-buffered", title="YCSB mixes, buffered commits", grid=ycsb_grid("buffered")),
    dict(
        name="fig6-space",
        title="Space",
        grid=two_rows(
            [
                dict(workload="load", quantity="bytes_on_disk_per_byte", title="Bytes on disk", sub="per byte stored, after the ordered load"),
                dict(workload="load", quantity="device_bytes_per_byte", title="Device bytes written", sub="per byte stored, by the ordered load"),
            ],
            "bytes per byte stored",
        ),
    ),
]

QUANTITY_TEXT = {
    "ops_per_s": "operations / s",
    "reads_per_s": "reads / s",
    "reads_per_s_2t": "reads / s, 2 threads",
    "reads_per_s_3t": "reads / s, 3 threads",
    "p99_us": "p99 read latency (µs)",
    "entries_per_s": "entries / s",
    "entries_per_s_2t": "entries / s, 2 threads",
    "entries_per_s_3t": "entries / s, 3 threads",
    "entries_per_s_lag0pct": "entries / s, drained",
    "entries_per_s_lag1pct": "entries / s after a 1% burst",
    "entries_per_s_lag10pct": "entries / s after a 10% burst",
    "entries_per_s_lag100pct": "entries / s after a 100% burst",
    "updates_per_s_lag1pct": "burst updates / s (1%)",
    "updates_per_s_lag10pct": "burst updates / s (10%)",
    "updates_per_s_lag100pct": "burst updates / s (100%)",
    "bytes_on_disk_per_byte": "bytes on disk per byte stored",
    "device_bytes_per_byte": "device bytes written per byte stored",
}


def captions(cells, top, groups):
    def r(wl, q, g, comp):
        return fmt_ratio(advantage(cells, wl, q, g, comp, top)[0])

    def raw(wl, q, g, comp):
        a = cells.get((wl, q, g, "supdb", top))
        b = cells.get((wl, q, g, comp, top))
        return fmt_ratio(a["med"] / b["med"]) if a and b else "–"

    T = size_label(top)
    cap = {
        "fig0-summary": "Every cell of the row as supdb's advantage over each comparator; bold where the 95% CIs do not overlap.",
        "fig1-loads": f"Ordered and shuffled loads, operations per second. At {T} keys the durable ordered load reads "
        f"{r('load', 'ops_per_s', 'durable', 'LMDB')} LMDB and {r('load', 'ops_per_s', 'durable', 'RocksDB')} RocksDB; "
        f"the shuffled load {r('load-shuffled', 'ops_per_s', 'durable', 'LMDB')} and {r('load-shuffled', 'ops_per_s', 'durable', 'RocksDB')}.",
        "fig2-reads": f"Point reads on one, two and three threads, and the single-thread p99 latency. At {T} keys, one thread, durable: "
        f"{r('read', 'reads_per_s', 'durable', 'LMDB')} LMDB, {r('read', 'reads_per_s', 'durable', 'RocksDB')} RocksDB.",
        "fig3-scans": f"Ordered scans of 100 entries on one, two and three threads. At {T} keys, one thread, durable: "
        f"{r('scan', 'entries_per_s', 'durable', 'LMDB')} LMDB, {r('scan', 'entries_per_s', 'durable', 'RocksDB')} RocksDB.",
        "fig3b-scans-after-mixes": "Threaded scans on the store the six mixes leave: a sixth of the keys rewritten and in the memtable or level 0. "
        f"At {T} keys, 3 threads, durable: {r('scan-mixed', 'entries_per_s_3t', 'durable', 'LMDB')} LMDB, "
        f"{r('scan-mixed', 'entries_per_s_3t', 'durable', 'RocksDB')} RocksDB.",
        "fig4-scan-lag": "Scans against merge lag: a burst of updates over 1%, 10% or 100% of the keys, then a pass of scans before "
        "the engine has caught up. The drained point is the store just after its load. "
        f"At {T} keys after the full rewrite, durable: {r('scan-lag', 'entries_per_s_lag100pct', 'durable', 'LMDB')} LMDB, "
        f"{r('scan-lag', 'entries_per_s_lag100pct', 'durable', 'RocksDB')} RocksDB.",
        "fig4b-bursts": "The lag sweep's bursts themselves: update rates while rewriting 1%, 10% and 100% of the keys of a loaded store. "
        f"At {T} keys the full rewrite, durable: {r('scan-lag', 'updates_per_s_lag100pct', 'durable', 'LMDB')} LMDB, "
        f"{r('scan-lag', 'updates_per_s_lag100pct', 'durable', 'RocksDB')} RocksDB.",
    }
    def vs(wl, q, g):
        have = [(c, advantage(cells, wl, q, g, c, top)[0]) for c in ("LMDB", "RocksDB")]
        return " and ".join(f"{fmt_ratio(v)} {c}" for c, v in have if v == v) or "–"

    for g, name in (("durable", "fig5a-ycsb-durable"), ("buffered", "fig5b-ycsb-buffered")):
        parts = [f"{x} {vs(f'ycsb-{x}', 'ops_per_s', g)}" for x in "ABCDEF"]
        cap[name] = (
            f"The six YCSB mixes with {g} commits (zipfian keys except D, which reads the latest inserts), operations per second. "
            f"At {T} keys: " + ", ".join(parts) + "."
        )
    cap["fig6-space"] = (
        "Space: bytes on disk and device bytes written per byte stored by the ordered load. "
        f"At {T} keys, durable, supdb's files are {raw('load', 'bytes_on_disk_per_byte', 'durable', 'LMDB')} LMDB's and "
        f"{raw('load', 'bytes_on_disk_per_byte', 'durable', 'RocksDB')} RocksDB's, and it writes "
        f"{raw('load', 'device_bytes_per_byte', 'durable', 'LMDB')} LMDB's device bytes and "
        f"{raw('load', 'device_bytes_per_byte', 'durable', 'RocksDB')} RocksDB's."
    )
    return cap


# ---------------------------------------------------------------- page

PAGE_CSS = """
:root{--surface:#fcfcfb;--page:#f9f9f7;--ink:#0b0b0b;--ink2:#52514e;--muted:#898781;--grid:#e1e0d9;--chip:#f0efec}
@media (prefers-color-scheme: dark){:root:not([data-theme="light"]){--surface:#1a1a19;--page:#0d0d0d;--ink:#f0efec;--ink2:#c3c2b7;--muted:#898781;--grid:#2c2c2a;--chip:#262624;color-scheme:dark}}
:root[data-theme="dark"]{--surface:#1a1a19;--page:#0d0d0d;--ink:#f0efec;--ink2:#c3c2b7;--muted:#898781;--grid:#2c2c2a;--chip:#262624;color-scheme:dark}
body{background:var(--page);color:var(--ink);font-family:Inter,system-ui,-apple-system,"Segoe UI",sans-serif;font-size:14px;line-height:1.5;padding-block:16px 32px;padding-left:max(16px,env(safe-area-inset-left,0px));padding-right:max(16px,env(safe-area-inset-right,0px))}
main{max-width:1100px;margin:0 auto}
h1{font-size:21px;margin:0 0 6px;text-wrap:balance}
h2{font-size:16px;margin:32px 0 8px;text-wrap:balance}
p{max-width:75ch}
p.lede{color:var(--ink2);margin:0 0 12px}
.note{color:var(--ink2);border-left:3px solid var(--grid);padding:2px 0 2px 12px;margin:12px 0 0}
figure{margin:0;background:var(--surface);border:1px solid var(--grid);border-radius:6px;padding:8px}
figure img{width:100%;height:auto;display:block}
figcaption{color:var(--ink2);font-size:13px;padding:8px 4px 2px;max-width:none}
details{margin:8px 0 0;overflow-x:auto}
summary{cursor:pointer;color:var(--ink2)}
summary:focus-visible,button:focus-visible,a:focus-visible{outline:2px solid var(--ink2);outline-offset:2px}
table{border-collapse:collapse;font-size:12px;margin-top:8px;width:100%;font-variant-numeric:tabular-nums}
th,td{text-align:left;padding:3px 6px;border-bottom:1px solid var(--grid);white-space:nowrap}
th{color:var(--muted);font-weight:500}
.ci{color:var(--muted);font-size:11px}
.key{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:6px;vertical-align:-1px}
.theme{float:right;font-size:12px;color:var(--ink2)}
.theme button{font:inherit;background:var(--surface);border:1px solid var(--grid);border-radius:4px;color:var(--ink);padding:2px 8px;cursor:pointer;margin-left:4px}
a{color:var(--ink2)}
code{font-size:12.5px;background:var(--chip);padding:1px 4px;border-radius:3px}
"""

KEYS = {"supdb": "#2a78d6", "LMDB": "#eb6834", "RocksDB": "#1baf7a"}


def figure_html(name, title, caption):
    t = html.escape(title)
    return (
        f"<h2>{t}</h2><figure><picture><source srcset=\"dark/{name}.png\" media=\"(prefers-color-scheme: dark)\">"
        f"<img src=\"{name}.png\" alt=\"{t}\"></picture><figcaption>{html.escape(caption)} "
        f"<a href=\"{name}.pdf\">PDF</a> · <a href=\"{name}.svg\">SVG</a> · <a href=\"dark/{name}.png\">dark</a></figcaption></figure>\n"
    )


def table_html(cells, sizes, specs):
    out = ["<details><summary>Table view</summary><table><thead><tr><th>workload</th><th>quantity</th><th>guarantee</th><th>engine</th>"]
    out += [f"<th>{size_label(s)}</th>" for s in sizes]
    out.append("</tr></thead><tbody>\n")
    for label, wl, q, g in specs:
        for eng in ENGINES:
            tds = []
            for s in sizes:
                c = cells.get((wl, q, g, eng, s))
                tds.append(
                    f"<td>{fmt3(c['med'])} <span class=ci>[{fmt3(c['lo'])}–{fmt3(c['hi'])}]</span></td>" if c else "<td>–</td>"
                )
            out.append(
                f"<tr><td>{html.escape(label)}</td><td>{html.escape(QUANTITY_TEXT.get(q, q))}</td><td>{g}</td>"
                f"<td><span class=key style=\"background:{KEYS[eng]}\"></span>{eng}</td>{''.join(tds)}</tr>\n"
            )
    out.append("</tbody></table></details>\n")
    return "".join(out)


def table_specs(fig_spec):
    specs = []
    for row in fig_spec["grid"]:
        for p in row:
            title = p.get("title") or ""
            specs.append((title, p["workload"], p["quantity"], p["guarantee"]))
    # The buffered row has no titles: carry the durable row's.
    titles = {}
    for t, wl, q, g in specs:
        if t:
            titles[(wl, q)] = t
    specs = [(titles.get((wl, q), wl), wl, q, g) for t, wl, q, g in specs]
    # Group by panel, durable first, as the old tables did.
    order = []
    for t, wl, q, g in specs:
        if (t, wl, q) not in [(a, b, c) for a, b, c, _ in order]:
            order.append((t, wl, q, None))
    out = []
    for t, wl, q, _ in order:
        for g in ("durable", "buffered"):
            if (t, wl, q, g) in specs:
                out.append((t, wl, q, g))
    return out


def ooc_table_html(groups):
    out = ["<details><summary>Table view</summary><table><thead><tr><th>comparator</th><th>sitting</th><th>cell</th>"
           "<th>supdb</th><th>LMDB</th><th>supdb over LMDB</th></tr></thead><tbody>\n"]
    for g in groups:
        for i, s in enumerate(g["sittings"]):
            for lab, wl, q in OOC_ROWS:
                v = s["vals"].get((wl, q))
                if not v:
                    continue
                out.append(
                    f"<tr><td>{html.escape(g['title'])}</td><td>{i + 1} ({'LMDB' if s['first'] == s['comp'] else 'supdb'} first)</td>"
                    f"<td>{html.escape(lab)}</td><td>{fmt3(v[s['sup']])}</td><td>{fmt3(v[s['comp']])}</td>"
                    f"<td>{fmt_ratio(ooc_ratio(s, wl, q))}</td></tr>\n"
                )
    out.append("</tbody></table></details>\n")
    return "".join(out)


def meta_of(row, cells):
    m = row["machine"]
    reps = max(len(c["samples"]) for c in cells.values())
    date = f"{row['utc'][0:4]}-{row['utc'][4:6]}-{row['utc'][6:8]}"
    return (
        f"One run, every arm interleaved rep by rep · {reps} reps per cell · median with 95% bootstrap CI of the median "
        f"(the suite's statistic) · supdb {row['sha'][:7]} · {date} · {m['cpu_model']}, {m['cpus']} vCPU, "
        f"{m['mem_total_kb'] / 1048576:.1f} GiB, {m['virtualised']} · 16-byte keys, 100-byte values"
    )


def arm_text(row, eng, g):
    arm = row["_arms"].get((eng, g))
    if arm is None and eng == "RocksDB" and g == "buffered" and row["_untuned"]:
        return "RocksDB left out (this row's buffered arm is untuned)"
    if arm is None:
        return f"{eng} (not in this row)"
    if arm == "supdb":
        return "supdb as it ships (supdb)"
    if arm == "supdb-ingest":
        return "supdb with buffered commits (supdb-ingest)"
    return f"{ARM_TEXT.get(arm, arm)} ({arm})"


def save(fig, out, name, dark=False):
    if dark:
        os.makedirs(os.path.join(out, "dark"), exist_ok=True)
        fig.savefig(os.path.join(out, "dark", name + ".png"))
    else:
        fig.savefig(os.path.join(out, name + ".png"))
        fig.savefig(os.path.join(out, name + ".pdf"))
        fig.savefig(os.path.join(out, name + ".svg"))
    plt.close(fig)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("row")
    ap.add_argument("out")
    ap.add_argument("--ooc", action="append", default=[])
    ap.add_argument("--ooc-note", default="")
    ap.add_argument("--companion", help="a second row, drawn as the summary matrix only (the gate's quick row)")
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    row, cells, sizes = load_row(args.row)
    meta = meta_of(row, cells)
    foot = meta + " · higher is better unless the panel says otherwise"
    groups = load_ooc(args.ooc) if args.ooc else []
    comp = load_row(args.companion) if args.companion else None

    with open(os.path.join(args.out, "cells.csv"), "w", newline="") as f:
        wr = csv.writer(f)
        wr.writerow(["workload", "quantity", "unit", "guarantee", "engine", "arm", "keys", "median", "ci_lo", "ci_hi", "reps", "samples"])
        for (wl, q, g, eng, s), c in sorted(cells.items()):
            wr.writerow([wl, q, c["unit"], g, eng, c["arm"], s, f"{c['med']:.6g}", f"{c['lo']:.6g}", f"{c['hi']:.6g}", len(c["samples"]), " ".join(f"{v:.6g}" for v in c["samples"])])

    for theme in ("light", "dark"):
        T = THEMES[theme]
        style(T)
        save(
            summary_figure(T, cells, sizes, meta, f"supdb's advantage over each comparator, the {row['scale']} row to {size_label(sizes[-1])} keys"),
            args.out,
            "fig0-summary",
            theme == "dark",
        )
        if comp:
            crow, ccells, csizes = comp
            save(
                summary_figure(T, ccells, csizes, meta_of(crow, ccells), f"The pull-request gate's view: the {crow['scale']} row to {size_label(csizes[-1])} keys"),
                args.out,
                "fig0b-gate-summary",
                theme == "dark",
            )
        for spec in FIGURES:
            save(lines_figure(T, cells, sizes, spec, foot), args.out, spec["name"], theme == "dark")
        if groups:
            ooc_foot = (
                "bench ab, two arms a process, three pairs a sitting after a warmup pair, the order swapped between sittings; "
                "--bound 671088640, a cgroup memory limit of 640 MiB over the process and its page cache; reads divided by ten; "
                + " · ".join(f"{g['title']}: supdb {', '.join(sorted({s['sha'] for s in g['sittings']}))}" for g in groups)
            )
            save(ooc_figure(T, groups, ooc_foot), args.out, "fig7-out-of-core", theme == "dark")

    cap = captions(cells, sizes[-1], groups)
    page = [
        "<title>supdb benchmark figures</title>\n<meta name=\"claude:viewport-fit-sides\" content=\"cover\">\n",
        f"<style>{PAGE_CSS}</style>\n<main>\n",
        "<div class=theme>theme <button type=button onclick=\"document.documentElement.dataset.theme='light'\">light</button>"
        "<button type=button onclick=\"document.documentElement.dataset.theme='dark'\">dark</button>"
        "<button type=button onclick=\"delete document.documentElement.dataset.theme\">auto</button></div>\n",
        "<h1>supdb against LMDB and RocksDB over the size ladder</h1>\n",
        f"<p class=lede>{html.escape(meta)}.</p>\n",
        "<p class=lede>Durable arms: "
        + html.escape(", ".join(arm_text(row, e, "durable") for e in ENGINES))
        + ". Buffered arms: "
        + html.escape(", ".join(arm_text(row, e, "buffered") for e in ENGINES))
        + ". Every comparison is within a guarantee. Every figure is also a table (expand below each); PDF and SVG beside each PNG.</p>\n",
    ]
    if row["_untuned"]:
        page.append(
            "<p class=note>This row's buffered RocksDB arm is RocksDB as it ships (<code>rocksdb-nosync</code>: no Bloom filter, "
            "default block cache and write buffers) while its durable arm is tuned, so buffered RocksDB is left out of every figure "
            "and table here: <code>ROADMAP.md</code> quotes no buffered RocksDB number until that arm is tuned.</p>\n"
        )
    page.append(figure_html("fig0-summary", "Summary: supdb's advantage over each comparator", cap["fig0-summary"]))
    if comp:
        crow, ccells, csizes = comp
        top = csizes[-1]
        rd = advantage(ccells, "read", "reads_per_s", "durable", "LMDB", top)[0]
        sc = advantage(ccells, "scan", "entries_per_s", "durable", "LMDB", top)[0]
        ccap = (
            f"The same matrix from the {crow['scale']} row the pull-request gate reads ({os.path.basename(args.companion)}): "
            f"{size_label(csizes[0])} to {size_label(top)} keys, {meta_of(crow, ccells).split(' · ')[1]}. "
            f"At {size_label(top)} keys, durable, point reads read {fmt_ratio(rd)} LMDB and scans {fmt_ratio(sc)}. "
            "Its rows are short and its host moves between rows, so it is the small-store companion to the matrix above, "
            "not a second headline."
            + (" Its buffered RocksDB arm is untuned and is left out." if crow["_untuned"] else "")
        )
        page.append(figure_html("fig0b-gate-summary", "The pull-request gate's view", ccap))
    for spec in FIGURES:
        page.append(figure_html(spec["name"], spec["title"], cap[spec["name"]]))
        page.append(table_html(cells, sizes, table_specs(spec)))
    if groups:
        page.append(figure_html("fig7-out-of-core", "Out of core: 5M keys under 640 MiB", args.ooc_note or "Out of core."))
        page.append(ooc_table_html(groups))
    page.append(
        "<p style=\"color:var(--muted);font-size:12px;margin-top:24px\">Raw per-rep samples: <a href=\"cells.csv\">cells.csv</a>. "
        f"Source row: <code>{html.escape(os.path.basename(args.row))}</code>.</p>\n</main>\n"
    )
    with open(os.path.join(args.out, "index.html"), "w") as f:
        f.write("".join(page))
    print(f"wrote {args.out}: {len(FIGURES) + 1 + (1 if groups else 0)} figures, {len(cells)} cells, sizes {sizes}")
    for g in groups:
        for i, s in enumerate(g["sittings"]):
            print(f"  {g['title']} sitting {i + 1}: {s['sup']} vs {s['comp']} at {s['sha']}, installs {['-' if x is None else round(x, 1) for x in s['inst']]}")


if __name__ == "__main__":
    main()
