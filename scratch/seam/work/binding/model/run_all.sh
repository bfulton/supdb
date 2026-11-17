#!/bin/sh
# Every run under nice 19 on CPU 3, each well under 30 s of CPU.
cd "$(dirname "$0")"
OPS=RC,WC,SE,SY,SF,MAN,RT,RO,MG,CR
OPSA=RC,WC,SE,SF,MAN,RT,RO,MG,CR
M="nice -n 19 taskset -c 3 python3 model.py"
: > results.txt
for v in NONE LEAD LEADT L1 L1S L1P L1SK FULLN FULLV FULL FULLC LEADK M1 M1L M2 M3 M3L M4 M4L; do
  $M $v always 6 --ops $OPSA --pl-damage-depth 6 --examples 1 >> results.txt
  for p in bufWR bufRW; do
    $M $v $p 6 --ops $OPS --pl-damage-depth 6 --examples 1 >> results.txt
  done
done
for v in NONE FULL; do
  $M $v always 6 --ops $OPSA --pl-damage-depth 6 --run-sem plain --examples 1 >> results.txt
  $M $v bufWR 6 --ops $OPS --pl-damage-depth 6 --run-sem plain --examples 1 >> results.txt
done
for v in FULL FULLC; do
  $M $v always 8 --ops $OPSA --pl-damage-depth 8 --examples 1 >> results.txt
  for p in bufWR bufRW; do
    $M $v $p 8 --ops $OPS --pl-damage-depth 6 --examples 1 >> results.txt
  done
done
