import math
U='9'*20  # u64 at full width ({:08}/{:016} are minimum widths)
def spell(n):
    p=((n+15)//16)*16
    return 1+math.ceil(4*(16+12+p+17+16)/3)
for F in (16,32):
    h='f'*(2*F)
    names={'par merge/promo':f'par-{U}-{U}-{h}-{h}.sup','pcs':f'pcs-{U}-{U}-{h}-.sup',
      'first par':f'par-{U}-{U}--.sup','seg':f'seg-{U}-{U}.sup','ord.tmp':f'ord-{U}-{U}.oidx.tmp',
      'zpg.tmp':f'zpg-{U}-{U}.zpg.tmp','zpg-dead':f'zpg-dead-{U}.zpg','compact':f'compact-{U}.tmp',
      'direct':f'direct-{U}.tmp','wal':f'wal-{U}','manifest.prev':f'manifest.prev-{U}',
      'par+pfcopy':f'par-{U}-{U}-{h}-{h}.sup.pfcopy','spill-tmp presented':'spill-'+'0'*32+'-'+U+'.tmp'}
    for k,v in names.items(): print(F,k,len(v),spell(len(v)),'SPILL' if spell(len(v))>255 else '')
print('budget max inline bytes', max(n for n in range(300) if spell(n)<=255), 'spilled field 19->', spell(19))
