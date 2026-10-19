# Pre-registration, written 01:55 UTC before lmdb-nordahead's first 10M line prints

CELL: point reads at 10M keys under a 1 GiB bound, supdb-nocache against LMDB with readahead off.
Printed so far (warmup rep): supdb-nocache 53,778/s at 0.413 faults and 1,694 B per read; LMDB with
default readahead 33,246/s at 0.267 faults and 19,809 B per read (74 KB per fault).

DECOMPOSITION: faults per read = 1 - cache/footprint when the cold pages touched per read is one.
Cache ~937 MB. supdb file ~1.52 GB (1.308 B/B at 3M applied), LMDB 1.33 GB (data.mdb 1.332 GB seen).
Predicted miss fractions 0.384 and 0.296; measured 0.413 and 0.267 (readahead helps LMDB slightly).

PREDICTION for lmdb-nordahead (the LMDB a user configures for this regime):
  faults per read 0.27 +- 0.03; device bytes per read 1.1 KB +- 0.15 KB (4096 x faults);
  point reads 65-85k/s single thread, i.e. 1.2-1.6x supdb-nocache's 53.8k/s;
  3-thread reads 150-220k/s (supdb-nocache 147k/s).
REFUTES the footprint story if: nordahead reads at or below 54k/s, or its faults per read exceed 0.38.
Scans: LMDB nordahead loses its readahead advantage; predict ~1 fault per leaf page, scan 1t
entries/s between supdb-nocache's 0.8M and LMDB-default's 1.83M.
