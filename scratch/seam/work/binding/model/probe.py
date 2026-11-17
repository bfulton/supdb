#!/usr/bin/env python3
"""One history, one end state: probe.py VARIANT POLICY OPS [DAMAGE] [--land a/b/..]

OPS is a comma list of macro operations; forced micro-steps (SY2, CR2, CSY,
WCW) run after each.  DAMAGE is a Python tuple as model.damages() makes them,
e.g. "('rbyte',1,2)".  --land picks a power-loss landing: per temp file (in id
order) a count or A, then per WAL file a count, separated by '/', the temps
and the WAL parted by '|', e.g. "0|1".  --stop-before-forced leaves the last
operation's micro-steps unrun (the window between them)."""
import sys
import model as m


def main():
    args = sys.argv[1:]
    land = None
    stop = False
    if '--land' in args:
        i = args.index('--land')
        land = args[i + 1]
        del args[i:i + 2]
    if '--stop-before-forced' in args:
        args.remove('--stop-before-forced')
        stop = True
    v, pol, ops = args[0], args[1], args[2].split(',')
    dmg = eval(args[3]) if len(args) > 3 else None
    var = m.VARIANTS[v]
    s = m.init()
    for n, op in enumerate(ops):
        while s['forced']:
            m.run_forced(s, var, pol)
        assert m.enabled(s, op, pol, m.OPF.keys() | {'CR'}), (op, s['ops'])
        s['ops'].append(op)
        if op == 'CR':
            assert m.op_CR(s, var, pol, lambda *a: None)
        else:
            m.OPF[op](s, var, pol)
        s['lastop'] = op
    while s['forced'] and not stop:
        m.run_forced(s, var, pol)
    if land is None:
        base = m.crash_view(s)
    else:
        t, w = land.split('|')
        tv = [x for x in t.split('/') if x]
        wv = [int(x) for x in w.split('/') if x]
        base = None
        for key, vv in m.pl_views(s):
            if [str(x) for x in key[0]] == tv and list(key[1]) == wv:
                base = vv
        assert base is not None, 'no such landing'
    views = m.apply_damage(s, base, dmg, 'design') if dmg else [base]
    H = ' '.join('%s%s' % (h[0], h[1]) for h in s['H'])
    for vw in views:
        out = m.open_store(s, vw, var)
        o = (out[0], sorted(out[1])) if out[0] == 'OPEN' else out
        print('%-6s %-6s ops=[%s] land=%s dmg=%s | H=%s | %s -> %s' % (
            v, pol, ' '.join(s['ops']), land, dmg, H, o, m.judge(s, out)))


if __name__ == '__main__':
    main()
