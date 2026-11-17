#!/bin/bash
# E2 driver: every timed run waits for a quiet machine first, and runs alone.
# usage: run.sh SET   (sets: main gbbusy split sparse tag steady cold base gb)
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e2-miss
cd "$W" || exit 1
E=./e2
D=data
mkdir -p raw

busy_pct() { # non-idle share of all CPUs over half a second, in percent
  read -r _ a1 b1 c1 d1 e1 f1 g1 h1 _ < /proc/stat
  sleep 0.5
  read -r _ a2 b2 c2 d2 e2 f2 g2 h2 _ < /proc/stat
  local idle=$(( (d2 + e2) - (d1 + e1) ))
  local tot=$(( (a2+b2+c2+d2+e2+f2+g2+h2) - (a1+b1+c1+d1+e1+f1+g1+h1) ))
  echo $(( 100 * (tot - idle) / (tot > 0 ? tot : 1) ))
}
quiet() {
  local n=0
  while :; do
    local b; b=$(busy_pct)
    if [ "$b" -lt 8 ] && ! pgrep -x 'cargo|rustc|bench|python3|cc1|ld' >/dev/null; then return 0; fi
    n=$((n + 1))
    if [ $n -eq 1 ] || [ $((n % 30)) -eq 0 ]; then
      echo "# waiting: busy=${b}% $(ps -eo pcpu,comm --sort=-pcpu | sed -n 2,4p | tr '\n' ' ')" >&2
    fi
    sleep 5
  done
}
jiff() { read -r _ u n s i w q sq st _ < /proc/stat; echo "$((u + n + s + q + sq)) $st"; }
r() { # label, then env assignments and args to e2; logs the CPU others took during the run
  local out=$1; shift
  quiet
  echo "# $(date +%T) $*" >> "raw/$out.txt"
  local a b TIMEFORMAT='%U %S %R' tm
  a=($(jiff))
  tm=$( { time env "$@" >> "raw/$out.txt" 2>&1 ; } 2>&1 )
  b=($(jiff))
  echo "$tm ${a[0]} ${b[0]} ${a[1]} ${b[1]}" | awk '{busy=($5-$4)/100; own=$1+$2; printf "CPUCHECK own_s=%.2f wall_s=%.2f others_s=%.2f steal_s=%.2f\n", own, $3, busy-own, ($7-$6)/100}' >> "raw/$out.txt"
}

set_=$1
case $set_ in
main|main2)
  arms=(i ii iii iv v vi)
  for rep in 1 2 3 4 5; do
    k=$(( rep % ${#arms[@]} ))
    order=("${arms[@]:$k}" "${arms[@]:0:$k}")
    [ $((rep % 2)) -eq 0 ] && order=($(printf '%s\n' "${order[@]}" | tac))
    for a in "${order[@]}"; do for t in 1 3; do r $set_ $E run $D "$a" $t 11; done; done
  done ;;
gbbusy)
  for rep in 1 2 3; do for a in i ii iii iv v; do for t in 1 3; do r gbbusy GBBUSY=1 $E run $D "$a" $t 11; done; done; done ;;
split)
  for rep in 1 2 3; do for a in i ii iii iv v vi; do for t in 1 3; do r split SPLIT=1 $E run $D "$a" $t 11; done; done; done ;;
sparse)
  for rep in 1 2 3 4 5; do for a in i ii iii iv; do for t in 1 3; do r sparse STRIDE=3 $E run $D "$a" $t 11; done; done
    for a in i ii; do r sparse STRIDE=3 SPLIT=1 $E run $D "$a" 1 11; done; done ;;
tag)
  for rep in 1 2 3; do
    for t in 1 3; do
      r tag GROUP=1 $E run $D ii $t 41
      for a in ii-t1 ii-t2 ii-t3; do r tag $E run $D "$a" $t 41; done
    done
    for a in ii ii-t1 ii-t2 ii-t3; do
      if [ "$a" = ii ]; then r tag GROUP=1 SPLIT=1 $E run $D ii 1 41; else r tag SPLIT=1 $E run $D "$a" 1 41; fi
    done
  done ;;
steady)
  for rep in 1 2 3 4 5; do for f in 0.25 0.5; do for a in i ii; do r steady $E steady $D "$a" $f; done; done; done ;;
cold)
  for rep in 1 2 3; do
    r cold SLEEPMS=4000 $E run $D ii 1 4
    r cold SLEEPMS=4000 STRIDE=3 $E run $D i 1 4
    r cold SLEEPMS=4000 $E run $D i 1 4
    r cold SLEEPMS=4000 SPLIT=1 $E run $D ii 1 4
  done ;;
drop)
  for rep in 1 2 3 4 5; do
    for d in 0 1 2; do
      r drop DROP=$d $E run $D ii 1 11
      r drop DROP=$d STRIDE=3 $E run $D ii 1 11
      r drop DROP=$d STRIDE=3 $E run $D i 1 11
      r drop DROP=$d $E run $D i 1 11
      r drop DROP=$d $E run $D v 1 11
    done
  done ;;
mems)
  for rep in 1 2 3 4; do for a in i ii iv v; do r mems MEMS=1 $E run $D "$a" 3 11; r mems $E run $D "$a" 3 11; done; done ;;
resplit)
  for rep in 1 2; do r split SPLIT=1 $E run $D i 3 11; r split SPLIT=1 $E run $D ii 1 11; done ;;
base)
  for rep in 1 2 3; do r base $E base $D; done ;;
gb)
  for rep in 1 2; do
    r gb REPS=3 $E gb $D
    r gb REPS=3 OTHER=1 $E gb $D
    r gb REPS=3 BUSY=3 $E gb $D
    r gb REPS=3 OTHER=1 BUSY=3 $E gb $D
  done ;;
*) echo "set?"; exit 2 ;;
esac
echo "# done $set_ $(date +%T)" >> "raw/$set_.txt"
