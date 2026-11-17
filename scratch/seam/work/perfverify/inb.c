#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/resource.h>
#include <sys/uio.h>
#include <errno.h>
int main(){
  const char*p="inb.dat"; int fd=open(p,O_RDWR|O_CREAT|O_TRUNC,0600);
  char*buf=aligned_alloc(4096,1<<20); for(int i=0;i<(1<<20);i++)buf[i]=i;
  for(int i=0;i<16;i++) write(fd,buf,1<<20);
  fsync(fd); posix_fadvise(fd,0,0,POSIX_FADV_DONTNEED); posix_fadvise(fd,0,0,POSIX_FADV_RANDOM);
  struct rusage a,b; getrusage(1,&a);
  int nowait_again=0, n=200;
  for(int i=0;i<n;i++){ off_t o=((off_t)(rand()%4096))*4096;
    struct iovec v={buf,4096};
    ssize_t r=preadv2(fd,&v,1,o,RWF_NOWAIT); if(r<0&&errno==EAGAIN){nowait_again++; pread(fd,buf,4096,o);} }
  getrusage(1,&b);
  printf("preads %d nowait_eagain %d majflt %ld inblock(512B) %ld => pages %ld\n",n,nowait_again,b.ru_majflt-a.ru_majflt,b.ru_inblock-a.ru_inblock,(b.ru_inblock-a.ru_inblock)/8);
  unlink(p); return 0;}
