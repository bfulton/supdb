#!/bin/sh
# The peak anonymous MiB of an oomprobe run between each phase mark and the next.
S=/tmp/claude-0/-home-user-supdb/95907082-4009-52f5-8d06-b36856ae75e3/scratchpad
a=$1
grep -h '^phasemark' $S/oom-$a.err | awk '{print $NF, $3"/"$4}' > $S/oom-$a.marks
awk -v marks=$S/oom-$a.marks 'BEGIN{n=0; while((getline l < marks)>0){split(l,f," "); t[n]=f[1]; lab[n]=f[2]; n++}}
  {u=$1; kb=$2; for(i=n-1;i>=0;i--) if(u>=t[i]){ if(kb>m[i])m[i]=kb; break}}
  END{for(i=0;i<n;i++) printf "%s %s %d MiB\n", i, lab[i], m[i]/1024}' $S/oom-$a.mem
