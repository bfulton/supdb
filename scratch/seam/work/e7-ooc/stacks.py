#!/usr/bin/env python3
# Buckets `perf script -F tid,ip,sym` call chains of one thread by the
# mechanism each sample is in, first match in the order below, and prints
# each bucket's share of the thread's samples. Usage: stacks.py SCRIPT.txt
import sys
from collections import Counter

RULES = [
    ("memcg/direct reclaim", ("try_to_free_mem_cgroup_pages", "shrink_lruvec", "shrink_node", "shrink_folio_list", "do_try_to_free_pages", "mem_cgroup_handle_over_high", "try_charge_memcg", "reclaim_high")),
    ("tlb shootdown (ipi)", ("smp_call_function_many_cond", "flush_tlb_mm_range", "native_flush_tlb_multi")),
    ("memfd page fault (shmem)", ("shmem_fault", "shmem_get_folio_gfp", "shmem_alloc_and_add_folio")),
    ("madvise/fallocate", ("do_madvise", "__x64_sys_madvise", "shmem_fallocate", "vfs_fallocate")),
    ("file fault: device submit", ("iowrite16", "virtqueue_notify", "blk_mq_", "submit_bio", "ext4_mpage_readpages", "bio_")),
    ("file fault: wait/switch", ("io_schedule", "folio_wait_bit", "finish_task_switch", "__schedule", "schedule")),
    ("file fault: map/lookup", ("filemap_fault", "filemap_map_pages", "next_uptodate_folio", "do_read_fault", "__do_fault", "filemap_get_entry", "xas_load")),
    ("other page fault", ("exc_page_fault", "handle_mm_fault", "asm_exc_page_fault")),
    ("other syscall", ("entry_SYSCALL", "do_syscall_64")),
    ("aes-gcm (ring)", ("aes_gcm", "ring_core", "aes_hw", "gcm")),
    ("memmove/copy", ("__memmove", "memcpy")),
    ("pager (user)", ("pager",)),
    ("flatindex/blob/read path (user)", ("flatindex", "blob", "Reader", "read_all", "supdb_get", "parse_record")),
]


def classify(frames):
    for name, keys in RULES:
        for f in frames:
            if any(k in f for k in keys):
                return name
    if frames and frames[0].startswith("[k]"):
        return "other kernel"
    return "other user"


samples = Counter()
leafk = Counter()
block = []
total = 0
for line in open(sys.argv[1]):
    s = line.rstrip("\n")
    if not s.strip():
        if block:
            frames = block
            total += 1
            samples[classify(frames)] += 1
            block = []
        continue
    if s[0] != "\t" and not s.startswith(" "):
        continue
    parts = s.split(None, 1)
    if len(parts) == 2 and all(c in "0123456789abcdef" for c in parts[0]):
        addr, sym = parts
        k = int(addr, 16) >= 0xffff800000000000
        block.append(("[k]" if k else "[u]") + sym)
    elif parts and parts[0].isdigit():
        continue
if block:
    total += 1
    samples[classify(block)] += 1
print(f"samples {total}")
for name, n in samples.most_common():
    print(f"{100.0*n/total:6.2f}%  {name}")
