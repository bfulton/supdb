import re, collections, statistics as st, sys
R='/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e5-commit/raw/'
for f in sys.argv[1:]:
    iss={}; lat=collections.defaultdict(list)
    ev_list=[]
    for l in open(R+f):
        m=re.search(r'\s(\d+\.\d+): block_rq_(issue|complete): \S+ (\S+) (?:\d+ )?\(\) (\d+) \+ (\d+)',l)
        if not m: continue
        t,ev,rw,sec,n=m.groups(); t=float(t)
        kind='D' if 'D' in rw else ('F' if rw.startswith('F') else ('M' if 'M' in rw else 'W'))
        key=(kind,sec,n)
        ev_list.append((t,ev,kind,int(sec),int(n)))
        if ev=='issue': iss[key]=t
        elif key in iss: lat[kind].append((t-iss.pop(key))*1e6)
    # per flush cycle: time from the first write issue after a flush complete to next flush complete; does a discard sit inside it?
    cyc=[]; cur=None; hasD=False
    for t,ev,kind,sec,n in ev_list:
        if ev=='issue' and kind in 'WMD' and cur is None: cur=t; hasD=False
        if ev=='issue' and kind=='D' and n==8: hasD=True
        if ev=='complete' and kind=='F' and cur is not None:
            cyc.append(((t-cur)*1e6,hasD)); cur=None
    wD=[c for c,h in cyc if h]; nD=[c for c,h in cyc if not h]
    print(f, {k:(len(v), round(st.median(v),1)) for k,v in lat.items()},
          'cycles with 8-sect discard', len(wD), round(st.median(wD),1) if wD else '-', 'without', len(nD), round(st.median(nD),1) if nD else '-')
