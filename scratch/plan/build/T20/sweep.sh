#!/bin/sh
# Runs the four stopped-queue tests N times at each width of the scratch window; prints reds.
BIN=target/checked/deps/durable-72e75b2e3260fb23
for us in 0 50 100 150 200 300 400 600 800 1200 2000; do
  red=0
  for i in $(seq 1 25); do
    SUPDB_SCRATCH_WEDGE_US=$us $BIN a_seal_of_a_run_whose_close_cannot_be_made_leaves_the_run_open a_write_after_a_hand_off_over_a_stopped_queue a_write_that_ends_a_run_whose_close_cannot_be_made > /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T20/sweep.out 2>&1 || { red=$((red+1)); cp /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T20/sweep.out /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/plan/build/T20/sweep.red.$us; }
  done
  echo "window ${us}us: $red red of 25"
done
