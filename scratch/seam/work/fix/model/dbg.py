import sys, pfm, logrun
from pfm import *
def show(W, v, ino=1):
    for k in sorted([k for k in v if k[0]==ino], key=repr):
        x = v[k]
        if x == TORN: print('  ', k[1], 'TORN'); continue
        m = W.imgs[x]
        if m['kind']=='frame':
            b=dict(m['body']); 
            print('  ', k[1], x, 'frame sidx',m['sidx'],'seq',m['seq'], {kk:vv for kk,vv in b.items() if kk in ('k','len','home','homes','groups','sh','open','gmac','off','b')})
        elif m['kind']=='hdr': print('  ', k[1], x, 'hdr seq', m['hdr_seq'], 'table', [(t[1],t[2],t[3]) for t in m['table']])
        else: print('  ', k[1], x, m['kind'], {kk:vv for kk,vv in m.items() if kk in ('p','g','content')}, 'key', m['key'])
