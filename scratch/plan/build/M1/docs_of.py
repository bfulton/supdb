# For every fn in a file, the doc block right above it (after attributes). Compare tree with HEAD and MERGE_HEAD:
# a fn whose doc in either side is not the tree's doc (and the tree's doc is not the other side's) is printed.
import re,subprocess,sys
def docs(text):
    out={}
    lines=text.split('\n')
    for i,l in enumerate(lines):
        m=re.match(r'\s*(pub(\([^)]*\))?\s+)?(async\s+)?(unsafe\s+)?(const\s+)?fn\s+(\w+)',l)
        if not m: continue
        j=i-1
        while j>=0 and lines[j].strip().startswith('#['): j-=1
        d=[]
        while j>=0 and lines[j].strip().startswith('///'): d.append(lines[j].strip()); j-=1
        out.setdefault(m.group(6),[]).append('\n'.join(reversed(d)))
    return out
def show(rev,f):
    r=subprocess.run(['git','show',f'{rev}:{f}'],capture_output=True,text=True)
    return r.stdout if r.returncode==0 else ''
bad=0
for f in sys.argv[1:]:
    t=docs(open(f).read()); h=docs(show('HEAD',f)); u=docs(show('MERGE_HEAD',f)); b=docs(show('9bfc4362',f))
    for name,tl in t.items():
        for side,sd in (('HEAD',h),('MERGE_HEAD',u)):
            if name in sd and len(sd[name])==len(tl):
                for k,(a,c) in enumerate(zip(sd[name],tl)):
                    if a!=c:
                        other=(u if side=='HEAD' else h).get(name)
                        if other and len(other)==len(tl) and other[k]==c: continue
                        # changed by the other side relative to base is fine
                        print(f,name,k,'differs from',side); bad+=1
print('BAD',bad)
