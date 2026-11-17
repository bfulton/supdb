#!/usr/bin/env python3
"""Summaries of e2's raw outputs: per configuration, the median across runs of each
run's figure, with the range across runs. Usage: summarize.py raw/main.txt [...]"""
import re, sys, statistics as st
from collections import defaultdict

def kv(line):
    return dict(re.findall(r'(\w+)=([^\s]+)', line))

def num(x):
    try:
        return float(x)
    except ValueError:
        return x

for path in sys.argv[1:]:
    res = defaultdict(list); spl = defaultdict(list); stdy = defaultdict(list); base = defaultdict(list); gb = defaultdict(list)
    # runs another process disturbed (CPUCHECK: others' CPU over a quarter of the wall) are left out
    blocks, cur = [], []
    for line in open(path):
        if line.startswith('# ') and line[2:3].isdigit():
            blocks.append(cur); cur = []
        cur.append(line)
    blocks.append(cur)
    kept, dropped = [], 0
    for b in blocks:
        cc = [kv(l) for l in b if l.startswith('CPUCHECK')]
        if cc and float(cc[0]['others_s']) > max(0.2, 0.25 * float(cc[0]['wall_s'])):
            dropped += 1; continue
        kept.extend(b)
    if dropped: print(f'# {path}: {dropped} disturbed run(s) left out')
    for line in kept:
        if line.startswith('RESULT'):
            d = {k: num(v) for k, v in kv(line).items()}
            key = (d['arm'], int(d['threads']), int(d['stride']), int(d['group']), int(d['gbbusy']), int(d['split']), int(d.get('drop', 0)), int(d.get('mems', 0)))
            res[key].append(d)
        elif line.startswith('SPLIT'):
            d = kv(line[5:]); key = (d.pop('arm'), int(d.pop('threads')))
            spl[key].append({k: float(v) for k, v in d.items()})
        elif line.startswith('STEADY'):
            d = {k: num(v) for k, v in kv(line).items()}
            stdy[(d['arm'], d['frac'])].append(d)
        elif line.startswith('BASE'):
            m = re.match(r'BASE (.*?)\s+median_ns=(\d+) p10=(\d+) p90=(\d+) mean=(\d+)', line)
            base[m.group(1)].append([float(m.group(i)) for i in range(2, 6)])
        elif line.startswith('GB '):
            d = kv(line); gb[(d['order'], d['busy'], d['other_thread_filled'])].append(float(d['ns_per_page']))
    print(f'== {path}')
    def rng(v):
        return f'{st.median(v):.0f} [{min(v):.0f}-{max(v):.0f}]'
    if res:
        print('arm threads stride group gbbusy split drop mems | runs | median_ns p10 p90 mean (median across runs [min-max]) | minflt/pg majflt/pg | giveback ns/pg | agg Mpages/s | r0 median | bad')
        for key in sorted(res, key=lambda k: (k[6], k[5], k[3], k[2], k[4], k[0], k[1])):
            v = res[key]
            f = lambda k: [x[k] for x in v]
            print(' '.join(map(str, key)), '|', len(v), '|', rng(f('median_ns')), '|', rng(f('p10_ns')), '|', rng(f('p90_ns')), '|', rng(f('mean_ns')),
                  '|', f"{st.median(f('minflt_pp')):.3f} {st.median(f('majflt_pp')):.4f}", '|', rng(f('gb_ns_pp_med')),
                  '|', f"{st.median(f('agg_pages_per_s'))/1e6:.3f}", '|', rng(f('r0_median_ns')), '|', int(sum(f('bad'))))
    if spl:
        print('-- split components: median across runs of each run\'s median (mean in parentheses)')
        for key in sorted(spl):
            v = spl[key]; comps = [k[:-4] for k in v[0] if k.endswith('_med')]
            s = ', '.join(f"{c} {st.median([x[c + '_med'] for x in v]):.0f} ({st.median([x[c + '_mean'] for x in v]):.0f})" for c in comps)
            tot_med = sum(st.median([x[c + '_med'] for x in v]) for c in comps)
            tot_mean = sum(st.median([x[c + '_mean'] for x in v]) for c in comps)
            print(f'{key[0]} t={key[1]} runs={len(v)}: {s}; sum of medians {tot_med:.0f}, sum of means {tot_mean:.0f}')
    if stdy:
        print('-- steady state: arm frac | runs | median_ns | mean_ns | minflt/miss | giveback ns/miss | hit rate')
        for key in sorted(stdy):
            v = stdy[key]; f = lambda k: [x[k] for x in v]
            print(key[0], key[1], '|', len(v), '|', rng(f('median_ns')), '|', rng(f('mean_ns')), '|', f"{st.median(f('minflt_per_miss')):.3f} [{min(f('minflt_per_miss')):.3f}-{max(f('minflt_per_miss')):.3f}]",
                  '|', rng(f('gb_ns_per_miss')), '|', f"{st.median(f('hit_rate')):.3f}", '| bad', int(sum(f('bad'))))
    if base:
        print('-- components alone: median across runs of median [min-max of medians]; mean')
        for k, v in base.items():
            print(f'{k:62s} {rng([x[0] for x in v])}  mean {st.median([x[3] for x in v]):.0f}')
    if gb:
        print('-- give-back ns per page after a fill: order busy other_thread_filled | n | median [min-max]')
        for k in sorted(gb):
            print(' '.join(k), '|', len(gb[k]), '|', rng(gb[k]))
