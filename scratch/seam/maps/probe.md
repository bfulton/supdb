## Can a decrypting pager sit behind userfaultfd here?

Kernel 6.18.44-fc-v470, Firecracker guest with 4 vCPUs, cpuidle driver "none", run as root. I did not touch `/home/user/supdb`. Everything is in `/tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/uffd.c`. The handler's fill is a 4 KiB memcpy (0.36 µs) plus one stamped word, and every touch checks that word: bad=0 in all runs. Each test uses 100k distinct pages touched in random order.

**What is available:**
- `userfaultfd(0)` works, and so does `UFFD_USER_MODE_ONLY`.
- `/dev/userfaultfd` exists (crw------- root) and `USERFAULTFD_IOC_NEW` works.
- `vm.unprivileged_userfaultfd` is 0.
- `UFFDIO_API` reports features 0x1ffff. That includes WP, MISSING_SHMEM, MINOR_SHMEM, SIGBUS, THREAD_ID, EXACT_ADDRESS, POISON, WP_ASYNC and MOVE.
- On anonymous memory, MISSING registers (COPY) and so does WP. MINOR fails with "Invalid argument".
- On a memfd, MISSING (COPY), MINOR (COPY and CONTINUE) and WP all register.

### Results (per fault, median / p99 in µs; "agg" is aggregate time per fault)

| probe | median | p99 | notes |
|---|---|---|---|
| memfd COPY, 1 faulting thread, 1 handler (blocking) | 33.7 | 67.8 | |
| anon COPY, 1/1 | 31.1 | 63.5 | |
| memfd COPY, 1/1, handler busy-polls | 19.2 | 37.5 | |
| memfd COPY, 1/1, both on the same CPU | **4.8** | 8.8 | anon: 3.6 |
| memfd COPY, 3 faulting threads / 1 handler | 41.0 | 87.3 | agg 14.2 |
| memfd COPY, 3/3 | 43.4 | 124.7 | agg 16.2; anon 40.9 |
| memfd COPY, 16 pages a fault | 58.9 | 150.6 | 4.0 a page; pinned: 28.3, 1.96 a page |
| memfd COPY, 256 pages a fault | 546 | 1050 | 2.3 a page; anon 1.9 a page |
| MISSING, filled through a second mapping, then CONTINUE | 32.0 | 70.0 | pinned: 4.6 |
| MINOR on a page already in the memfd, CONTINUE only | 28.6 | 60.3 | pinned: **3.5**; 16 pages: 1.8 a page |
| UFFD_FEATURE_SIGBUS, faulting thread runs COPY itself, memfd | **4.5** | 7.1 | 3 threads: 7.2 / 22.2, agg 2.7 |
| same, anon | 3.3 | 5.6 | 3 threads: 5.4 |
| PROT_NONE, SIGSEGV handler fills and mprotects, 1 thread | 7.1 | 27.5 | 3 threads: 11.2, agg 4.2 |
| Baseline: first-touch read of a fresh memfd page, no uffd | 3.1 | 8.4 | write: 1.9; anon write: 1.5 |
| Baseline: pread 4 KiB from a cached disk file / from a memfd | 0.75 / 1.06 | 1.2 / 1.5 | |
| Hit on an installed page (random, includes a TLB miss) | 0.26 | 0.57 | an ordinary load; the clock pair costs 0.02 |

**Eviction, µs per call, process idle → with 3 other threads of the process running:**
- anon `MADV_DONTNEED`, 1 MiB: 40 → 46
- anon `MADV_DONTNEED`, 4 KiB: 0.84 → **18.4**
- memfd `MADV_REMOVE`, 1 MiB: 57 → 88
- memfd `MADV_REMOVE`, 4 KiB: 1.27 → **19.2**
- either one over a page that is already empty: 0.2–0.4
- `mprotect(PROT_NONE)`, 4 KiB: 2.2 → 20

The jump with threads running is the TLB shootdown, an IPI to each other CPU. After eviction, every evicted page faulted through the handler again: 7022 of 7022 touched, all bytes correct.

**System calls whose buffer is in a missing page:**
- In the default mode, both `write()` from the page and `pread()` into it succeed. The kernel's own fault goes through the handler (handled=2, data correct).
- Under `UFFD_USER_MODE_ONLY`, both fail with "Bad address".
- Under `FEATURE_SIGBUS`, `write(pipe)` fails with "Bad address".
- On a PROT_NONE page, `write(pipe)` fails with "Bad address".

So only the handler-thread mode is safe for the engine handing mapped slices to `write()`.

**Skipped:** I did not measure eviction by `MADV_DONTNEED` of the reader mapping in MINOR mode, where the page stays in the memfd and the next touch takes the 3.5 µs pinned path.

### To reproduce
```
cd /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam
gcc -O2 -pthread -o uffd uffd.c
./uffd avail
./uffd fault <shm 0|1> <mode 0=COPY 1=MISSING+fill+CONTINUE 2=MINOR-prefilled> <pages a fault> <faulting threads> <handlers> <faults>
#  e.g. ./uffd fault 1 0 1 1 1 100000 ; ./uffd fault 1 0 1 3 3 100000 ; ./uffd fault 1 0 256 1 1 1000 ; ./uffd fault 1 2 1 1 1 100000
#  environment: PIN=1 (every thread on CPU 0), PIN=2 (separate CPUs), SPIN=1 (handler busy-polls), BUSY=1 (a spinning thread on each CPU)
./uffd evict 0|1 ; BUSY=1 ./uffd evict 0|1
./uffd sysbuf 0|1                 # 1 = UFFD_USER_MODE_ONLY
./uffd base
./uffd sigbus <shm> <threads> 100000
./uffd segv <threads> 30000
```

### What this means for a pager whose miss costs about 4 µs and whose hit costs a call and a load
1. A miss sent to a handler thread costs 30–45 µs here, eight to ten times today's 4 µs. Almost all of it is two cross-CPU wakeups at about 15 µs each, whether the CPUs are idle or busy (busy-polling the handler halves it at best). The same mechanism on one CPU costs 3.5–4.8 µs, and nothing lets a fault be routed to a handler on the faulting thread's CPU.
2. `UFFD_FEATURE_SIGBUS`, where the faulting thread runs `UFFDIO_COPY` itself, matches today's miss at 4.5 µs and costs nothing on a hit. But a kernel access to a missing page then fails with "Bad address", so every `write()` of a mapped slice would have to make the page present first. That puts back an "ask before you hand it to the kernel" rule on exactly those paths.
3. Eviction is not free either. A single-page `MADV_DONTNEED` or `MADV_REMOVE` costs about 19 µs once other threads are running, because of the TLB shootdown, and a 1 MiB batch costs 46–88 µs. Giving memory back has to stay batched in megabyte units, as the pager does today. With that, the hit becomes a bare load and the miss costs about the same as now, but only in SIGBUS mode.