#!/bin/sh
# Depth 9 for FULL and FULLC, split by the first two operations so that each
# run stays under 30 s of CPU; nice 19 on CPU 3.
cd "$(dirname "$0")"
OPS=RC,WC,SE,SY,SF,MAN,RT,RO,MG,CR
OPSA=RC,WC,SE,SF,MAN,RT,RO,MG,CR
M="nice -n 19 taskset -c 3 python3 model.py"
: > results_deep.txt
for v in FULL FULLC; do
  for pre in RC,RC RC,WC RC,SE RC,CR WC,WC WC,SE WC,RO WC,CR CR,RC CR,WC CR,CR; do
    $M $v always 9 --ops $OPSA --first $pre --pl-damage-depth 7 --examples 1 >> results_deep.txt
  done
  for p in bufWR bufRW; do
    for pre in RC,RC RC,WC RC,SE RC,SY RC,CR WC,WC WC,SE WC,SY WC,RO WC,CR CR,RC CR,WC CR,CR; do
      $M $v $p 9 --ops $OPS --first $pre --pl-damage-depth 6 --examples 1 >> results_deep.txt
    done
  done
done
for v in LEAD L1 L1S L1P L1SK LEADK; do
  $M $v always 8 --ops $OPSA --pl-damage-depth 6 --examples 1 >> results_deep.txt
  $M $v bufWR 8 --ops $OPS --pl-damage-depth 5 --examples 1 >> results_deep.txt
done
