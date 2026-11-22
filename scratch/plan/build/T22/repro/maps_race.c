/* No engine: threads that map and unmap small anonymous ranges of their
   own, beside one thread that reads /proc/self/maps, as a backtrace's
   symbolization does. Every call is on a range the calling thread mapped. */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>

static volatile int stop;
static void *mapper(void *a) {
  (void)a;
  while (!stop) {
    void *p = mmap(NULL, 1588, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) { perror("mmap"); exit(2); }
    ((char *)p)[0] = 1;
    if (munmap(p, 1588)) { perror("munmap"); exit(2); }
  }
  return NULL;
}
static void *reader(void *a) {
  (void)a;
  static char buf[1 << 16];
  for (int i = 0; i < 2000; i++) {
    int fd = open("/proc/self/maps", O_RDONLY | O_CLOEXEC);
    if (fd < 0) { perror("open"); exit(2); }
    while (read(fd, buf, sizeof buf) > 0) {}
    close(fd);
  }
  stop = 1;
  return NULL;
}
int main(int argc, char **argv) {
  int n = argc > 1 ? atoi(argv[1]) : 3;
  pthread_t t[16], r;
  for (int i = 0; i < n; i++) pthread_create(&t[i], NULL, mapper, NULL);
  pthread_create(&r, NULL, reader, NULL);
  pthread_join(r, NULL);
  for (int i = 0; i < n; i++) pthread_join(t[i], NULL);
  puts("done");
  return 0;
}
