import re,sys
for l in open(sys.argv[1]).read().splitlines():
  if l.startswith('STORE') or l.startswith('BEFORE'): 
    if l.startswith('STORE-START'): acc={}
    if l.startswith('STORE-END') or l.startswith('BEFORE'): print('  ',acc)
    print(l); continue
  m=re.search(r'thread=Some\("([^"]+)"\) seg=(\d+) len=(\d+) placed=(\d+) records=(\d+) via \["1: ([^"]+)", "2: ([^"]+)", "3: ([^"]+)"',l)
  if m:
    g=m.groups(); k=(g[0][:12],g[5][-20:],g[7][-20:])
    a=acc.setdefault(k,[0,0,0]); a[0]+=1;a[1]+=int(g[3]);a[2]+=int(g[4])
