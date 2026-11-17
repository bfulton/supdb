# E2: the cost of a miss (design.md section 9), as a C probe

Machine: Firecracker guest, kernel 6.18.44, 4 vCPUs (Xeon 2.3 GHz, AES-NI/VAES/VPCLMULQDQ), 16 GB, shmem THP never,
virtio_balloon with free page reporting (order 9). Cipher: OpenSSL 3.0.13 EVP AES-256-GCM (warm 4 KiB decrypt 0.436 us).
The engine uses ring 0.17; ring was not timed here.

## Setup (e2.c)
- `env.bin`: today's envelope (crypt.rs: header page, then 4096-byte stored chunks = 4080 ct + 16 tag, nonce id8||c, AAD = 48-byte header).
- `pg.bin`: the design's layout (section 3): data page p at E(e)+2MiB+(p&0xFFFF)*4096, E(e)=1MiB+e*258MiB; tag entries 32 B,
  128 per tag page at E(e)+(g&511)*4096; tag pages sealed (AES-GCM), MAC kept apart. N = 102,400 stored pages (400 MiB each file).
- Both mapped read-only MAP_SHARED + MADV_RANDOM (as pager.rs Back::open), read in and mapped once (MADV_POPULATE_READ);
  mincore showed every page resident.
- Backing: memfd, MAP_SHARED, MADV_NOHUGEPAGE (pager.rs map_shared). Each op ends with the reader's load through the
  backing mapping and a check of the plaintext's first word: bad = 0 in every run that decrypts.
- Rounds: every page (or chunk) once per round in a fresh xoshiro permutation; 11 rounds, round 0 reported apart;
  between rounds the backing is given back by MADV_REMOVE in 1 MiB units on the main thread, timed apart, per page present.
- Faults: getrusage(RUSAGE_THREAD) per worker. Timing: clock_gettime pairs (21 ns).
- Driver run.sh waits for a quiet machine before every run and logs the CPU other processes took during it
  (CPUCHECK); runs with others > 25% of wall are left out by summarize.py (another session's python3 model ran at times).

## Results, 1 thread, ns per page: median across runs [range of run medians], mean
| arm | regime | median | mean | minflt/page | give-back/page |
|---|---|---|---|---|---|
| (i) today | dense (spec: every chunk once) | 2862 [2776-3399]; 3038 [2959-3277] (2nd sitting) | 3202; 3342 | 0.998 | 489; 517 |
| (i) today | isolated (every 3rd chunk) | 4441 [3865-5034]; 4226 [3736-4693] | 5022; 4897 | 1.994 | 461 |
| (i) today | steady, budget 25% / 50% | 4265 [3869-4393] / 3887 [3531-4255] | 4458 / 3923 | 1.596 / 1.333 per miss | 825 / 645 per miss |
| (ii) copy+decrypt | dense | 2828 [2385-3017]; 2876 [2590-3072] | 3263; 3217 | 1.002 | 464; 562 |
| (ii) | isolated | 3093 [2466-3382]; 2934 [2584-3085] | 3421; 3340 | 1.002 | 573; 522 |
| (ii) | steady 25% / 50% | 2812 [2564-2987] / 2559 [2494-3195] | 3074 / 2824 | 1.002 | 544 / 446 per miss |
| (iii) pread | dense | 3130 [2868-3406]; 3068 [2870-3360] | 3499; 3440 | 1.002 | 529 |
| (iii) | isolated | 3218 [3057-3465] | 3648 | 1.002 | 500 |
| (iv) decrypt+pwrite | dense | 4278 [4030-4758]; 4708 [4485-5034] | 4801; 5242 | 1.002 | 662; 744 |
| (iv) | isolated | 3990 [3645-4245] | 4529 | 1.002 | 633 |
| (v) 64-page populate | dense | 2120 [1951-2383]; 2247 [2079-2578] | 2356; 2520 | 1.000 | 472; 513 |
| (vi) 64 pwrite+POPULATE_READ (extra) | dense | 2134 [1874-2257]; 2312 [2200-2563] | 2391; 2568 | 0.078 | 441 |

3 threads (one memfd shared, as one file's backing): (i) 3666/4201, (ii) 3489/3945, (iii) 4164/4451, (iv) 6599/6676,
(v) 3153/3144, (vi) 3983/4093 median (two sittings); isolated (i) 6111, (ii) 3633. With one memfd per thread (mems set):
(ii) 3082 [2972-3302] against 3912 shared in the same sitting, (iv) 4398 vs 6048, (v) 2424 vs 3220: the 3-thread
penalty is mostly contention on one shmem inode. Aggregate Mpages/s: (ii) 0.30 (1t), 0.62-0.72 (3t shared), 0.77 (3t per-thread memfd).

## Breakdown of (ii), 1 thread
- Subtractive (drop set, same sitting, dense): full 2757, copy without decrypt 2192, tag lookup + 1-byte fault 2032 ->
  fault+tag ~2.0 us, copy+cipher ~0.7 (isolated: 2934 / 2143 / 2042 -> 0.9). Margins are not additive (out-of-order overlap).
- Stamped split (inflates the total by 0.4-0.6 us): tag 194-216, fault 1960-2124, copy 675-746, cipher 472-491, load 22.
- Alone (base set): write fault on a fresh memfd page 1713-2117, memcpy 4 KiB cold->warm 493-805, EVP decrypt 4 KiB warm 436,
  MADV_REMOVE 1 MiB of 256 sequentially-filled pages 60-72 us (0.23-0.28/page), MADV_POPULATE_WRITE 64 pages 76-95 us (1.2-1.5/page).
- Today's (i) isolated split: fault 3839 (2 faults, 75%), copy 766 (15%), cipher 470 (9%) — the shape perf gave (CLAUDE.md:
  fault over half, copy a fifth, cipher a tenth).

## Give-back per page
- gb set (400 MiB filled then removed per MiB): sequential fill same thread 260 [233-464], random fill same thread 288,
  random fill by another thread 451 [386-734], plus 3 threads spinning 596 [484-792]. probe.md's 0.22-0.34 was the sequential case.
- In the arms: 0.46-0.56 idle, 0.62 (1t) / 0.78 (3t) with the readers spinning (gbbusy set), 0.45-0.54 per miss in the steady model
  (units 1/4-1/2 full when given back).

## Tag
Group-sampled (one page per tag group per round, 800 ops/round, 41 rounds); baseline (ii) 3248 [2985-3434].
- t1 tier entry flushed from cache: 2728 [2692-3229] — not distinguishable.
- t2 tier miss, tag page in the kernel's cache (copy + AES-GCM verify 4 KiB): 4159 [4043-4335]; split tag 1028 vs 208 -> +0.8-0.9 us
  (3t: 4814 vs 3867).
- t3 tag page not resident (major fault, MADV_RANDOM, one 4 KiB device read): 44821 [35147-50234] 1t, majflt 1.0/op;
  ~56.7k [55.5k-61.7k] at 3t (those runs flagged only by the I/O kworkers' CPU; taken unfiltered).

## Other checks
- Full-page pwrite into a shmem hole skips the zeroing on 6.18 (perf cpu-clock over zloop: clear_page_erms 0.00% for pwrite 4096,
  3.06% for the write-fault loop, __pi_memset 2.58% for pwrite 2048). Zeroing is ~0.1 us of a ~2 us fault, so it buys little;
  (iv) pays a pwrite (1.2-1.7 us) and then a read fault to map the page (1.55-1.88 us, split).
- Cold host (4 s pause between rounds so freed memory is reported): (ii) 2877 vs 2828-2876; (i) isolated 4789 vs 4441 (+8%).

## Reproduce
```
cd /tmp/claude-0/-home-user-supdb/2fecfb3d-3630-50b2-b9c5-2a19705f39b6/scratchpad/seam/work/e2-miss
gcc -O2 -Wall -Wextra -pthread -o e2 e2.c -lcrypto
./e2 setup data            # 804 MB, 1 s; deleted after the runs
./run.sh main; ./run.sh main2; ./run.sh sparse; ./run.sh steady; ./run.sh split; ./run.sh drop; ./run.sh tag
./run.sh gbbusy; ./run.sh cold; ./run.sh mems; ./run.sh base; ./run.sh gb
python3 -I summarize.py raw/*.txt
P=/usr/lib/linux-tools-6.8.0-31/perf; $P record -e cpu-clock -F 20000 -o perf/zloop0.data ./e2 zloop data 0   # 1, 2 likewise
```
