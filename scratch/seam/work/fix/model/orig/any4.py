import model as M, sys
cfg=dict(K=2, fix=True, ckpt_at_write=True) if sys.argv[1]=='fix' else dict(K=2)
runs,counts,first=M.drive(cfg,4,[1,5],['any'])
print(sys.argv[1],'runs',runs,counts,{k:v for k,v in first.items()})
