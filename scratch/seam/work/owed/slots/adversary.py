#!/usr/bin/env python3
"""The adversary with the directory and not the key, against the two slots.

For every crash-free history over WUOBEF up to maxlen (slots.py's engine,
fixed protocol), the adversary has every record a publish wrote whole, in
the slot it was written to, and every pair of slot images the device held
together at some instant (each prefix's every device view).  It writes into
the two slot files any of: nothing (EMPTY), bytes that are no record
(GARBAGE), or any record it has, in either file (a put-back, a copy, a
swap).  Then the open's rule (slots.choose) runs.

Claimed (reported as violations):
  A_WRONG   the open takes a record no publish wrote whole
  A_RELOC   the open takes a record sealed for the other slot
  A_EMPTY   two files with bytes and no valid record read as "no manifest"
            (the store opened as one with no manifest: today's pre-manifest
            scan, which removes every par-/pcs- file)
Reported, not claimed (decision 1 / CLAUDE.md say the store taken back to
an earlier state of itself is not detected):
  ROLLBACK  the open takes a state older than the newest acknowledged durable
  MIXED     the open answers from a pair the device never held together
  REFUSED   the open refuses (a denial: deleting a file does as much)
"""
import itertools, sys, collections, time
import slots
from slots import World, Dev, Eng, Orc, Runner, choose, OPT

GARB = ('G', 9)


def histories(o, maxlen):
    """yield (W, pairs, ackimg, imgs_by_slot) per crash-free sequence"""
    for n in range(0, maxlen + 1):
        for t in itertools.product('WUOBEF', repeat=n):
            R = Runner(o, 'h')
            W = World()
            D = Dev(2, R.o['NSEC'], R.o['SU'])
            E = Eng(W, D, R.o, 0, 1, [None, None], 0)
            E.seq = -1
            to, img, ln = E.begin_publish()
            E.seq = 0
            E.write(to, img, ln)
            D.sync(to)
            orc = Orc(img)
            W.pub.add(img)
            E.durable, E.dimg[to], E.last_img = to, img, img
            pairs = set()
            T = slots.Tally('x')

            def snap():
                for v in D.views():
                    r = tuple(slots.read_slot(W, v, f, R.o, R.o['NSEC'], R.o['SU']) for f in (0, 1))
                    pairs.add(tuple(x[1] if x[0] == 'VALID' else None for x in r))
            snap()
            for op in t:
                if op in 'WUB':
                    to, img, ln = E.begin_publish()
                    E.write(to, img, ln)
                    orc.pub(img)
                    W.pub.add(img)
                    E.last_img = img
                    if op == 'W':
                        D.sync(to)
                        E.land(to, img, orc, T, t, [])
                    elif op == 'U':
                        E.entries.append(dict(slot=to, img=img, tok=None, held=True))
                    else:
                        E.entries.append(dict(slot=to, img=img, tok=D.sync_begin(to), held=True))
                elif op == 'O':
                    ow = [e for e in E.entries if e['tok'] is None]
                    if ow:
                        e = ow[-1]
                        E.entries.remove(e)
                        D.sync(e['slot'])
                        if e['held']:
                            E.land(e['slot'], e['img'], orc, T, t, [])
                elif op in 'EF':
                    fl = [e for e in E.entries if e['tok'] is not None]
                    if fl:
                        e = fl[0] if op == 'E' else fl[-1]
                        E.entries.remove(e)
                        D.sync_end(e['tok'])
                        if e['held']:
                            E.land(e['slot'], e['img'], orc, T, t, [])
                snap()
            yield t, W, pairs, orc


def place(W, img, f, o):
    """the record img's units written whole into file f: a view for that file"""
    su, nsec = o['SU'], o['NSEC']
    units = [None] * (su * nsec)
    if img == 'EMPTY':
        pass
    elif img == 'GARB':
        units[0] = GARB
    else:
        for k in range(W.imgs[img]['n']):
            units[k] = (img, k)
    return {(f, j): tuple(units[j * su:(j + 1) * su]) for j in range(nsec)}


def main():
    o = dict(OPT)
    for a in sys.argv[1:]:
        k, v = a.split('=', 1)
        try:
            v = int(v)
        except ValueError:
            pass
        o[k] = v
    maxlen = o.pop('maxlen', 4)
    name = 'adversary %s maxlen=%d' % (' '.join('%s=%s' % kv for kv in sorted(o.items()) if OPT.get(kv[0]) != kv[1]), maxlen)
    t0 = time.time()
    bad = collections.Counter()
    seen = collections.Counter()
    first = {}
    nh = 0
    for t, W, pairs, orc in histories(o, maxlen):
        nh += 1
        recs = sorted(W.pub)
        choices = ['EMPTY', 'GARB'] + recs
        ackimg = orc.hist[orc.ackpos]
        for a, b in itertools.product(choices, repeat=2):
            v = dict(place(W, a, 0, o))
            v.update(place(W, b, 1, o))
            verdict, taken, reads = choose(W, v, o, o['NSEC'], o['SU'])
            out = []
            if verdict == 'OK':
                f, x = taken
                img = x[1]
                if img not in W.pub:
                    out.append('A_WRONG')
                if W.imgs[img]['slot'] != f:
                    out.append('A_RELOC')
                if orc.hist.index(img) < orc.ackpos:
                    out.append('ROLLBACK')
                pr = tuple(x[1] if x[0] == 'VALID' else None for x in reads)
                if pr not in pairs:
                    out.append('MIXED')
            elif verdict == 'NO_MANIFEST':
                if a != 'EMPTY' and b != 'EMPTY':
                    out.append('A_EMPTY')
                else:
                    out.append('REFUSED')     # a store with a manifest and none readable
            else:
                out.append('REFUSED')
            seen['dirs'] += 1
            for k in out:
                bad[k] += 1
                if k not in first:
                    first[k] = (t, a, b, [W.imgs[x]['slot'] if x not in ('EMPTY', 'GARB') else x for x in (a, b)],
                                [W.imgs[x]['seq'] if x not in ('EMPTY', 'GARB') else x for x in (a, b)], reads, ackimg)
    claimed = sum(bad[k] for k in ('A_WRONG', 'A_RELOC', 'A_EMPTY'))
    print('==', name, 'histories', nh, 'directories', seen['dirs'], 'claimed violations', claimed,
          'secs %.1f' % (time.time() - t0))
    for k in sorted(bad):
        print('   ', k, bad[k])
    for k in sorted(first):
        t, a, b, sl, sq, reads, ack = first[k]
        print('  first', k, 'ops', t, 'file0', a, 'file1', b, 'sealed for', sl, 'seqs', sq, 'acked img', ack)
    sys.exit(1 if claimed else 0)


if __name__ == '__main__':
    main()
