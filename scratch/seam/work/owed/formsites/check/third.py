#!/usr/bin/env python3
"""Third derivation (skeptic's): brace-matched function bodies over every
non-test .rs under src, impl-block attribution, and a fixed point over the
call graph seeded from the form file's mapping and the cursor's held borrow.
Different from sites.py: functions are delimited by brace matching (not
"last fn above"), every file of src is searched, impl owners are known, and
the closure runs over ALL src functions by method name (collisions printed,
not filtered by receiver).
Usage: third.py SRC [--mutant N]"""
import re, sys, os, shutil, tempfile

def strip(src):
    # blank out comments and string/char literals, keep newlines/offsets
    out=[];i=0;n=len(src)
    while i<n:
        c=src[i]
        if src.startswith('//',i):
            j=src.find('\n',i); j=n if j<0 else j
            out.append(' '*(j-i)); i=j; continue
        if src.startswith('/*',i):
            d=1;j=i+2
            while j<n and d:
                if src.startswith('/*',j): d+=1;j+=2
                elif src.startswith('*/',j): d-=1;j+=2
                else: j+=1
            out.append(re.sub(r'[^\n]',' ',src[i:j])); i=j; continue
        m=re.match(r'b?r(#*)"',src[i:i+8])
        if m:
            h=m.group(1); end='"'+h; j=src.find(end,i+len(m.group(0)))
            j=n if j<0 else j+len(end)
            out.append(re.sub(r'[^\n]',' ',src[i:j])); i=j; continue
        if c=='"' or (c=='b' and i+1<n and src[i+1]=='"'):
            j=i+(2 if c=='b' else 1)
            while j<n and src[j]!='"':
                j+=2 if src[j]=='\\' else 1
            j+=1; out.append(re.sub(r'[^\n]',' ',src[i:j])); i=j; continue
        if c=="'":
            m=re.match(r"'(\\.[^']*|[^\\'])'",src[i:i+12])
            if m: out.append(' '*len(m.group(0))); i+=len(m.group(0)); continue
        out.append(c); i+=1
    return ''.join(out)

def match_brace(s,i):
    d=0
    for j in range(i,len(s)):
        if s[j]=='{': d+=1
        elif s[j]=='}':
            d-=1
            if d==0: return j
    return len(s)-1

def functions(path):
    raw=open(path).read(); s=strip(raw)
    # cut test modules
    cuts=[]
    for m in re.finditer(r'#\[cfg\(test\)\]\s*(pub(\([a-z]+\))?\s+)?mod\s+\w+\s*\{',s):
        b=s.index('{',m.start()); cuts.append((m.start(),match_brace(s,b)))
    impls=[]
    for m in re.finditer(r'\bimpl\b(?:<[^>{]*>)?\s+(?:[\w:<>&\' ,]+\s+for\s+)?([\w:]+)(?:<[^{]*>)?\s*(?:where[^{]*)?\{',s):
        b=m.end()-1; impls.append((b,match_brace(s,b),m.group(1).split('::')[-1]))
    fns=[]
    for m in re.finditer(r'\bfn\s+(\w+)\s*(<[^{;]*?>)?\s*\(',s):
        st=m.start()
        if any(a<=st<=b for a,b in cuts): continue
        # find body brace or ; (decl)
        j=m.end(); d=1
        while j<len(s) and d: 
            d+= s[j]=='('; d-= s[j]==')'; j+=1
        k=j
        while k<len(s) and s[k] not in '{;': k+=1
        if k>=len(s) or s[k]==';': continue
        e=match_brace(s,k)
        owner=None
        for a,b,o in impls:
            if a<st<b: owner=o
        line=s.count('\n',0,st)+1
        fns.append(dict(file=path,name=m.group(1),owner=owner,line=line,body=s[k:e+1],sig=s[st:k]))
    return fns

def run(src):
    files=[]
    for d,_,fs in os.walk(src):
        for f in fs:
            if f.endswith('.rs'): files.append(os.path.join(d,f))
    F=[]
    for p in sorted(files): F+=functions(p)
    rel=lambda f: os.path.relpath(f['file'],src)
    key=lambda f:(rel(f),f['owner'] or '-',f['name'],f['line'])
    # Seeds: bytes of a form file's mapping or a held borrow of it.
    seedpat=[
      (r'self\s*\.\s*map\s*\.\s*(slice_at|as_slice|read_at|lend_at|piece_at|piece_from)', 'ZFile.map read'),
      (r'\bmap\s*\.\s*(slice_at|as_slice\(\)\s*\[|read_at|piece_at)', 'local map read'),
      (r'Group::(Page|Coded)\s*\(', 'cursor held borrow'),
      (r'Opened::(Borrowed|Coded)\s*\(', 'opened borrow'),
      (r'zpg_name', 'names form file'),
      (r'\bskewed\b', 'test seam'),
    ]
    zimpl={f['name'] for f in F if f['owner'] in ('ZFile','ZCursor') and rel(f)=='db/zfile.rs'}
    reads=set(); why={}
    for f in F:
        for p,w in seedpat:
            if re.search(p,f['body']):
                if w=='names form file' and not re.search(r'vault::(map|read|open)\b|attach|warm_file',f['body']): continue
                if w=='local map read' and rel(f)!='db/zfile.rs' : 
                    pass
                reads.add(key(f)); why.setdefault(key(f),[]).append(w)
    # closure: a function calls (by .name( or ::name( ) a ZFile/ZCursor method in set
    changed=True
    byname={}
    for f in F:
        if key(f) in reads and f['owner'] in ('ZFile','ZCursor'):
            byname.setdefault(f['name'],[]).append(key(f))
    rounds=0
    while changed and rounds<50:
        changed=False; rounds+=1
        names={k[2] for k in reads if k[1] in ('ZFile','ZCursor')}
        pat=re.compile(r'(?:\.|::)\s*('+'|'.join(sorted(names))+r')\s*(?:::<[^>]*>)?\s*\(') if names else None
        for f in F:
            k=key(f)
            if k in reads: continue
            if pat and pat.search(f['body']):
                hit=pat.findall(f['body'])
                # only follow names that belong to ZFile/ZCursor, but in non-zfile files require a form-ish token
                if rel(f)=='db/zfile.rs' or re.search(r'\bz\b|zfile|ZCursor|ZFile|\bzc\(|cursor|\.z\.get|z_spare|zf\b',f['body']):
                    reads.add(k); why.setdefault(k,[]).append('calls '+','.join(sorted(set(hit))))
                    changed=True
    return F,reads,why,rel

if __name__=='__main__':
    src=sys.argv[1]
    F,reads,why,rel=run(src)
    for k in sorted(reads):
        print('%-14s %-12s %-26s %6d  %s'%(k[0],k[1],k[2],k[3],'; '.join(why[k])[:120]))
    print('TOTAL',len(reads), 'functions parsed',len(F))
