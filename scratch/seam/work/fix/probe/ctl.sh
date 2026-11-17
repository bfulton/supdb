#!/bin/bash
# Controls on other filesystems, as E5's jext4: an ext4 made in an image file
# behind a direct-io loop device, every shape at every size, COMMITS commits,
# REPS reps, shapes rotated. Counts are the loop device's (E5_DEV); timings
# are indicative only (the loop's writes land in a file on vda).
#   ctl.sh NAME "MKFS_OPTS" "MOUNT_OPTS" COMMITS REPS
W=/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/fix/probe
NAME=$1; MKFS=$2; MNT=$3; C=${4:-300}; REPS=${5:-3}
IMG=$W/tmp/$NAME.img
M=$W/tmp/mnt-$NAME
mkdir -p $M
dd if=/dev/zero of=$IMG bs=1M count=768 conv=fsync status=none
LOOP=$(losetup -f --show --direct-io=on $IMG) || exit 1
mkfs.ext4 -q $MKFS $LOOP || exit 1
mount -o "$MNT" $LOOP $M || exit 1
echo "# $NAME: mkfs.ext4 $MKFS; mount -o $MNT; $(grep " $M " /proc/mounts)" >> $W/raw/ctl-$NAME.txt
D=$(basename $LOOP)
SHAPES=(A Aw A0 Ap0 B D Ow O0 Tw T0 Rw R0 Cf Cp Ds Dsp J Jp S Sp)
NS=${#SHAPES[@]}
for rep in $(seq 1 $REPS); do
  for n in 100 4096 13300 133000; do
    k=$(( (rep - 1) * 7 % NS ))
    for j in $(seq 0 $(( NS - 1 ))); do
      s=${SHAPES[$(( (j + k) % NS ))]}
      E5_DEV=$D $W/e5 $s $n $C $M ${NAME}r$rep >> $W/raw/ctl-$NAME.txt || echo "FAIL $s $n" >> $W/raw/ctl-$NAME.txt
    done
  done
done
umount $M
losetup -d $LOOP
rm -f $IMG
rmdir $M
