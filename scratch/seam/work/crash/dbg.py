import model as M
r=M.Run(K=2,fix=True)
r.write(5); r.sync(); r.write(3); r.sync()
for e in r.events: print(e)
for fid,f in r.frames.items(): print(fid,f)
n=0
for d,ack in M.disks(r,'any'):
    b=M.check(r,d,ack)
    if b and b[0]=='UNREADABLE':
        print(b); print(sorted(d.items())); print(M.recover(r,d)); break
