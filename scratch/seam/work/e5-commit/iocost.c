// The device's price of a commit by its parts, for reading E5: k separate
// 4 KiB writes into allocated blocks inside i_size (no metadata), k pages
// in one contiguous write, and one page appended (allocation and i_size),
// each followed by fdatasync.
//
//   iocost <mode> <k> <commits> <dir>
//   mode: sep  k pwrites of one page, 1 MiB apart, over a prefilled file
//         con  one pwrite of k contiguous pages over a prefilled file
//         app  k pages appended in one write to a fresh file (i_size grows)
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static uint64_t now_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec;
}
static void rd(unsigned long long v[17]) {
  FILE *f = fopen("/sys/block/vda/stat", "r");
  for (int i = 0; i < 17; i++)
    if (fscanf(f, "%llu", &v[i]) != 1) break;
  fclose(f);
}
static int cmp(const void *a, const void *b) {
  uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
  return x < y ? -1 : x > y;
}
int main(int argc, char **argv) {
  const char *mode = argv[1];
  int k = atoi(argv[2]);
  int commits = atoi(argv[3]);
  char path[4096];
  snprintf(path, sizeof path, "%s/iocost-%d.f", argv[4], getpid());
  static uint8_t buf[64 * 4096];
  for (size_t i = 0; i < sizeof buf; i++) buf[i] = (uint8_t)(i * 131 + 7);
  int fd = open(path, O_CREAT | O_RDWR | O_TRUNC, 0644);
  if (fd < 0) return 1;
  if (strcmp(mode, "app")) {
    /* prefilled: 16 MiB of zeros in 4 KiB writes, fsynced */
    static uint8_t z[4096];
    for (int i = 0; i < 4096; i++)
      if (write(fd, z, 4096) != 4096) return 1;
    fsync(fd);
  }
  sync();
  usleep(300000);
  unsigned long long a[17], b[17];
  rd(a);
  uint64_t *lat = malloc(commits * sizeof *lat);
  for (int i = 0; i < commits; i++) {
    uint64_t t0 = now_ns();
    if (!strcmp(mode, "sep")) {
      for (int j = 0; j < k; j++)
        if (pwrite(fd, buf, 4096, (off_t)j * (1 << 20) + (off_t)(i % 256) * 4096) != 4096) return 1;
    } else if (!strcmp(mode, "con")) {
      if (pwrite(fd, buf, 4096 * k, (off_t)(i % (4096 / 64)) * 64 * 4096) != 4096 * k) return 1;
    } else {
      if (write(fd, buf, 4096 * k) != 4096 * k) return 1;
    }
    fdatasync(fd);
    lat[i] = now_ns() - t0;
  }
  rd(b);
  qsort(lat, commits, sizeof *lat, cmp);
  printf("mode=%s k=%d commits=%d med_us=%.1f p90_us=%.1f ios_per=%.2f sect_per=%.2f flush_per=%.2f\n", mode, k,
         commits, lat[commits / 2] / 1e3, lat[commits * 9 / 10] / 1e3, (double)(b[4] - a[4]) / commits,
         (double)(b[6] - a[6]) / commits, (double)(b[15] - a[15]) / commits);
  close(fd);
  unlink(path);
  return 0;
}
