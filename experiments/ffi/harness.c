/* The boundary experiment, native half: one process opens a supdb store
 * through the C ABI in capi/, an LMDB store through liblmdb and a RocksDB
 * store through librocksdb's C API, loads each with the same keys and
 * values, and then runs the suite's read pass -- n uniform keys, sixteen
 * decimal digits -- against each, borrowed and copied, interleaved and
 * repeated. The supdb store is also read by a loop inside the library
 * (supdb_capi_bench_native), which crosses no boundary per read: that is
 * the reference the per-call ABI cost is measured against.
 *
 * Usage: harness <root> <keys> <value_bytes> <reps> [load]
 * With `load` the stores are created; without, they are reopened. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <time.h>
#include <lmdb.h>
#include <rocksdb/c.h>
#include "capi/supdb_capi.h"

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

/* The suite's generator: xorshift64*, seeded explicitly. */
static uint64_t xs(uint64_t *s) {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    return *s;
}
static uint64_t seed_of(uint64_t seed) { return seed ? seed : 0x9E3779B97F4A7C15ull; }

/* Sixteen decimal digits, db_bench's key shape. */
static void key_into(uint64_t n, uint8_t *buf) {
    for (int i = 15; i >= 0; i--) { buf[i] = (uint8_t)('0' + n % 10); n /= 10; }
}

/* A value that can be checked: byte i of key n's value is (n*31 + i) & 0xff. */
static void value_of(uint64_t n, uint8_t *v, size_t len) {
    for (size_t i = 0; i < len; i++) v[i] = (uint8_t)(n * 31 + i);
}

static void die(const char *what, const char *err) {
    fprintf(stderr, "%s: %s\n", what, err ? err : "?");
    exit(1);
}

/* ---------------------------------------------------------------- stores */

static supdb_capi_handle *sup;
static MDB_env *lenv; static MDB_dbi ldbi; static MDB_txn *ltxn;
static rocksdb_t *rdb; static rocksdb_readoptions_t *rro;

static void open_supdb(const char *root, int create) {
    char p[4096]; snprintf(p, sizeof p, "%s/supdb", root);
    sup = supdb_capi_open(p, create);
    if (!sup) die("supdb open", supdb_capi_error());
}

static void open_lmdb(const char *root, int create, size_t map_bytes) {
    char p[4096]; snprintf(p, sizeof p, "%s/lmdb", root);
    char cmd[4200]; snprintf(cmd, sizeof cmd, "mkdir -p %s", p); if (system(cmd)) die("mkdir", p);
    int rc;
    if ((rc = mdb_env_create(&lenv))) die("mdb_env_create", mdb_strerror(rc));
    if ((rc = mdb_env_set_mapsize(lenv, map_bytes))) die("mapsize", mdb_strerror(rc));
    if ((rc = mdb_env_set_maxdbs(lenv, 4))) die("maxdbs", mdb_strerror(rc));
    /* The arm's flags: none. Durable commits, readahead as the kernel has it. */
    if ((rc = mdb_env_open(lenv, p, 0, 0664))) die("mdb_env_open", mdb_strerror(rc));
    MDB_txn *t;
    if ((rc = mdb_txn_begin(lenv, NULL, 0, &t))) die("txn", mdb_strerror(rc));
    if ((rc = mdb_dbi_open(t, NULL, create ? MDB_CREATE : 0, &ldbi))) die("dbi", mdb_strerror(rc));
    if ((rc = mdb_txn_commit(t))) die("commit", mdb_strerror(rc));
}

static void open_rocks(const char *root, int create) {
    char p[4096]; snprintf(p, sizeof p, "%s/rocksdb", root);
    /* rocksdb-tuned, as the arm states it: no compression, a 256 MB LRU
     * block cache, a 10-bit full Bloom filter with index and filter blocks
     * cached, four background threads, and the bulk-load write side. */
    rocksdb_options_t *o = rocksdb_options_create();
    rocksdb_options_set_create_if_missing(o, create);
    rocksdb_options_set_compression(o, rocksdb_no_compression);
    rocksdb_block_based_table_options_t *bbo = rocksdb_block_based_options_create();
    rocksdb_cache_t *cache = rocksdb_cache_create_lru(256u << 20);
    rocksdb_block_based_options_set_block_cache(bbo, cache);
    rocksdb_block_based_options_set_filter_policy(bbo, rocksdb_filterpolicy_create_bloom_full(10.0));
    rocksdb_block_based_options_set_cache_index_and_filter_blocks(bbo, 1);
    rocksdb_options_set_block_based_table_factory(o, bbo);
    rocksdb_options_increase_parallelism(o, 4);
    rocksdb_options_set_max_background_jobs(o, 4);
    rocksdb_options_set_write_buffer_size(o, 128u << 20);
    rocksdb_options_set_max_write_buffer_number(o, 4);
    rocksdb_options_set_min_write_buffer_number_to_merge(o, 2);
    rocksdb_options_set_level0_file_num_compaction_trigger(o, 8);
    char *err = NULL;
    rdb = rocksdb_open(o, p, &err);
    if (err) die("rocksdb_open", err);
    rro = rocksdb_readoptions_create();
    rocksdb_readoptions_set_verify_checksums(rro, 0);
}

/* ------------------------------------------------------------------ load */

#define BATCH 1000

static void load_all(uint64_t keys, size_t vlen) {
    uint8_t k[16]; uint8_t *v = malloc(vlen);
    char *err = NULL; int rc;
    uint64_t t0;

    t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(i, k); value_of(i, v, vlen);
        supdb_capi_append(sup, k, 16, v, vlen);
        if ((i + 1) % BATCH == 0 || i + 1 == keys)
            if (supdb_capi_commit(sup)) die("supdb commit", supdb_capi_error());
    }
    if (supdb_capi_flush(sup)) die("supdb flush", supdb_capi_error());
    printf("load supdb   %.2f s (drained)\n", (now_ns() - t0) / 1e9);

    t0 = now_ns();
    MDB_txn *t = NULL;
    for (uint64_t i = 0; i < keys; i++) {
        if (!t && (rc = mdb_txn_begin(lenv, NULL, 0, &t))) die("txn", mdb_strerror(rc));
        key_into(i, k); value_of(i, v, vlen);
        MDB_val mk = {16, k}, mv = {vlen, v};
        if ((rc = mdb_put(t, ldbi, &mk, &mv, 0))) die("mdb_put", mdb_strerror(rc));
        if ((i + 1) % BATCH == 0 || i + 1 == keys) {
            if ((rc = mdb_txn_commit(t))) die("mdb commit", mdb_strerror(rc));
            t = NULL;
        }
    }
    printf("load lmdb    %.2f s\n", (now_ns() - t0) / 1e9);

    t0 = now_ns();
    rocksdb_writeoptions_t *wo = rocksdb_writeoptions_create();
    rocksdb_writeoptions_set_sync(wo, 1);
    rocksdb_writebatch_t *wb = rocksdb_writebatch_create();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(i, k); value_of(i, v, vlen);
        rocksdb_writebatch_put(wb, (const char *)k, 16, (const char *)v, vlen);
        if ((i + 1) % BATCH == 0 || i + 1 == keys) {
            rocksdb_write(rdb, wo, wb, &err);
            if (err) die("rocksdb_write", err);
            rocksdb_writebatch_clear(wb);
        }
    }
    /* Not flushed: the arm's sync is a WAL fsync and its reads go through
     * the memtable, and that is the shape measured. */
    printf("load rocksdb %.2f s (memtable held)\n", (now_ns() - t0) / 1e9);
    free(v);
}

/* ------------------------------------------------------------------ read */

typedef struct { const char *name; uint64_t ns; uint64_t bytes; } Result;
static volatile uint64_t sink;

/* Each pass draws the suite's keys: uniform below `keys`, seed 7. */
#define READ_SEED 7

static Result supdb_native(uint64_t keys) {
    uint64_t bytes = 0;
    uint64_t ns = supdb_capi_bench_native(sup, keys, keys, READ_SEED, 0, &bytes);
    return (Result){"supdb native (loop in Rust)", ns, bytes};
}

static Result supdb_native_touch(uint64_t keys) {
    uint64_t bytes = 0;
    uint64_t ns = supdb_capi_bench_native(sup, keys, keys, READ_SEED, 1, &bytes);
    return (Result){"supdb native + touch 1 byte", ns, bytes};
}

/* The suite's keys, drawn once into a table, for the loops that draw none. */
static uint8_t *key_table;
static void make_key_table(uint64_t keys) {
    key_table = malloc(keys * 16);
    uint64_t s = seed_of(READ_SEED);
    for (uint64_t i = 0; i < keys; i++) key_into(xs(&s) % keys, key_table + i * 16);
}

static Result supdb_native_keys(uint64_t keys) {
    uint64_t bytes = 0;
    uint64_t ns = supdb_capi_bench_native_keys(sup, key_table, keys, &bytes);
    return (Result){"supdb native, keys from table", ns, bytes};
}

static Result supdb_borrow_keys(uint64_t keys) {
    uint64_t bytes = 0; const uint8_t *p; size_t n;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++)
        if (supdb_capi_get_borrow(sup, key_table + i * 16, 16, &p, &n) == 1) bytes += n;
    return (Result){"supdb C ABI borrowed, table", now_ns() - t0, bytes};
}

static Result supdb_borrow_touch(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0, sum = 0; uint8_t k[16];
    const uint8_t *p; size_t n;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (supdb_capi_get_borrow(sup, k, 16, &p, &n) == 1) { bytes += n; sum += p[0]; }
    }
    uint64_t ns = now_ns() - t0; sink += sum;
    return (Result){"supdb C ABI borrowed + touch", ns, bytes};
}

static Result supdb_borrow_touch_last(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0, sum = 0; uint8_t k[16];
    const uint8_t *p; size_t n;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (supdb_capi_get_borrow(sup, k, 16, &p, &n) == 1) { bytes += n; sum += p[n - 1]; }
    }
    uint64_t ns = now_ns() - t0; sink += sum;
    return (Result){"supdb C ABI borrowed + touch last", ns, bytes};
}

/* Borrow across the ABI, then memcpy in C: the copy without the copying
 * entry point, so the two can be told apart. */
static Result supdb_borrow_memcpy(uint64_t keys, size_t vlen) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16];
    uint8_t *buf = malloc(vlen); const uint8_t *p; size_t n;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (supdb_capi_get_borrow(sup, k, 16, &p, &n) == 1) { memcpy(buf, p, n < vlen ? n : vlen); bytes += n; }
    }
    uint64_t ns = now_ns() - t0; sink += buf[0]; free(buf);
    return (Result){"supdb C ABI borrowed + memcpy", ns, bytes};
}

static Result lmdb_borrow_touch_last(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0, sum = 0; uint8_t k[16];
    MDB_val mk = {16, k}, mv;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (mdb_get(ltxn, ldbi, &mk, &mv) == 0) { bytes += mv.mv_size; sum += ((uint8_t *)mv.mv_data)[mv.mv_size - 1]; }
    }
    uint64_t ns = now_ns() - t0; sink += sum;
    return (Result){"lmdb C borrowed + touch last", ns, bytes};
}

static Result lmdb_borrow_touch(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0, sum = 0; uint8_t k[16];
    MDB_val mk = {16, k}, mv;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (mdb_get(ltxn, ldbi, &mk, &mv) == 0) { bytes += mv.mv_size; sum += ((uint8_t *)mv.mv_data)[0]; }
    }
    uint64_t ns = now_ns() - t0; sink += sum;
    return (Result){"lmdb C borrowed + touch", ns, bytes};
}

static Result supdb_borrow(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16];
    const uint8_t *p; size_t n;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (supdb_capi_get_borrow(sup, k, 16, &p, &n) == 1) bytes += n;
    }
    return (Result){"supdb C ABI borrowed", now_ns() - t0, bytes};
}

static Result supdb_copy(uint64_t keys, size_t vlen) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16];
    uint8_t *buf = malloc(vlen); size_t n;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (supdb_capi_get_copy(sup, k, 16, buf, vlen, &n) == 1) bytes += n;
    }
    uint64_t ns = now_ns() - t0; free(buf);
    return (Result){"supdb C ABI copied", ns, bytes};
}

static Result lmdb_borrow(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16];
    MDB_val mk = {16, k}, mv;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (mdb_get(ltxn, ldbi, &mk, &mv) == 0) bytes += mv.mv_size;
    }
    return (Result){"lmdb C borrowed", now_ns() - t0, bytes};
}

static Result lmdb_copy(uint64_t keys, size_t vlen) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16];
    uint8_t *buf = malloc(vlen);
    MDB_val mk = {16, k}, mv;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        if (mdb_get(ltxn, ldbi, &mk, &mv) == 0) {
            memcpy(buf, mv.mv_data, mv.mv_size < vlen ? mv.mv_size : vlen);
            bytes += mv.mv_size;
        }
    }
    uint64_t ns = now_ns() - t0; free(buf);
    return (Result){"lmdb C copied", ns, bytes};
}

static Result rocks_pinned(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16]; char *err = NULL;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        rocksdb_pinnableslice_t *p = rocksdb_get_pinned(rdb, rro, (const char *)k, 16, &err);
        if (err) die("get_pinned", err);
        if (p) { size_t n; rocksdb_pinnableslice_value(p, &n); bytes += n; rocksdb_pinnableslice_destroy(p); }
    }
    return (Result){"rocksdb C pinned", now_ns() - t0, bytes};
}

static Result rocks_copy(uint64_t keys) {
    uint64_t s = seed_of(READ_SEED), bytes = 0; uint8_t k[16]; char *err = NULL;
    uint64_t t0 = now_ns();
    for (uint64_t i = 0; i < keys; i++) {
        key_into(xs(&s) % keys, k);
        size_t n; char *v = rocksdb_get(rdb, rro, (const char *)k, 16, &n, &err);
        if (err) die("get", err);
        if (v) { bytes += n; rocksdb_free(v); }
    }
    return (Result){"rocksdb C copied (malloc)", now_ns() - t0, bytes};
}

/* Every store answers every key with the value loaded, and a borrowed
 * supdb pointer still reads right after a thousand more reads. */
static void verify(uint64_t keys, size_t vlen) {
    uint8_t k[16]; uint8_t *want = malloc(vlen); uint8_t *buf = malloc(vlen);
    const uint8_t *held[1024]; uint64_t held_n[1024]; int nh = 0;
    uint64_t s = seed_of(READ_SEED);
    for (uint64_t i = 0; i < keys; i++) {
        uint64_t n = xs(&s) % keys; key_into(n, k); value_of(n, want, vlen);
        const uint8_t *p; size_t len;
        if (supdb_capi_get_borrow(sup, k, 16, &p, &len) != 1 || len != vlen || memcmp(p, want, vlen))
            die("verify", "supdb borrowed value differs");
        if (nh < 1024 && (i % 293) == 0) { held[nh] = p; held_n[nh] = n; nh++; }
        if (supdb_capi_get_copy(sup, k, 16, buf, vlen, &len) != 1 || len != vlen || memcmp(buf, want, vlen))
            die("verify", "supdb copied value differs");
        MDB_val mk = {16, k}, mv;
        if (mdb_get(ltxn, ldbi, &mk, &mv) || mv.mv_size != vlen || memcmp(mv.mv_data, want, vlen))
            die("verify", "lmdb value differs");
        char *err = NULL; size_t rn;
        char *rv = rocksdb_get(rdb, rro, (const char *)k, 16, &rn, &err);
        if (err || !rv || rn != vlen || memcmp(rv, want, vlen)) die("verify", "rocksdb value differs");
        rocksdb_free(rv);
    }
    for (int i = 0; i < nh; i++) {
        value_of(held_n[i], want, vlen);
        if (memcmp(held[i], want, vlen)) die("verify", "a borrowed supdb pointer went stale");
    }
    printf("verified %llu keys on every store; %d borrowed pointers held across the pass still read right\n",
           (unsigned long long)keys, nh);
    free(want); free(buf);
}

static int cmp_u64(const void *a, const void *b) {
    uint64_t x = *(const uint64_t *)a, y = *(const uint64_t *)b;
    return x < y ? -1 : x > y;
}

int main(int argc, char **argv) {
    if (argc < 5) { fprintf(stderr, "usage: harness <root> <keys> <value_bytes> <reps> [load]\n"); return 2; }
    const char *root = argv[1];
    uint64_t keys = strtoull(argv[2], NULL, 10);
    size_t vlen = strtoull(argv[3], NULL, 10);
    int reps = atoi(argv[4]);
    int create = argc > 5 && !strcmp(argv[5], "load");
    size_t map = (size_t)keys * (16 + vlen) * 4 + (256u << 20);

    open_supdb(root, create);
    open_lmdb(root, create, map);
    open_rocks(root, create);
    if (create) load_all(keys, vlen);
    int rc;
    if ((rc = mdb_txn_begin(lenv, NULL, MDB_RDONLY, &ltxn))) die("rtxn", mdb_strerror(rc));
    verify(keys, vlen);

    make_key_table(keys);
    enum { V = 15 };
    uint64_t ns[V][64]; const char *names[V] = {0}; uint64_t want_bytes = keys * vlen;
    if (reps > 64) reps = 64;
    for (int r = 0; r < reps; r++) {
        /* Rotated, so no variant always runs first or last. */
        for (int j = 0; j < V; j++) {
            int v = (j + r) % V;
            Result x;
            switch (v) {
            case 0: x = supdb_native(keys); break;
            case 1: x = supdb_native_keys(keys); break;
            case 2: x = supdb_native_touch(keys); break;
            case 3: x = supdb_borrow(keys); break;
            case 4: x = supdb_borrow_keys(keys); break;
            case 5: x = supdb_borrow_touch(keys); break;
            case 6: x = supdb_borrow_touch_last(keys); break;
            case 7: x = supdb_borrow_memcpy(keys, vlen); break;
            case 8: x = supdb_copy(keys, vlen); break;
            case 9: x = lmdb_borrow(keys); break;
            case 10: x = lmdb_borrow_touch(keys); break;
            case 11: x = lmdb_borrow_touch_last(keys); break;
            case 12: x = lmdb_copy(keys, vlen); break;
            case 13: x = rocks_pinned(keys); break;
            default: x = rocks_copy(keys); break;
            }
            if (x.bytes != want_bytes) { fprintf(stderr, "%s read %llu bytes, want %llu\n", x.name,
                (unsigned long long)x.bytes, (unsigned long long)want_bytes); return 1; }
            ns[v][r] = x.ns; names[v] = x.name;
        }
    }
    printf("\n%-36s %10s %10s %10s   reps=%d keys=%llu value=%zu\n", "variant", "min ns", "med ns", "max ns",
           reps, (unsigned long long)keys, vlen);
    for (int v = 0; v < V; v++) {
        uint64_t sorted[64]; memcpy(sorted, ns[v], sizeof(uint64_t) * reps);
        qsort(sorted, reps, sizeof(uint64_t), cmp_u64);
        double lo = (double)sorted[0] / keys, md = (double)sorted[reps / 2] / keys, hi = (double)sorted[reps - 1] / keys;
        printf("%-36s %10.1f %10.1f %10.1f\n", names[v], lo, md, hi);
    }
    /* Rep by rep, which of a pair was faster: the interleaving makes the
     * reps comparable and the count is the sign test the suite uses. */
    int pairs[][2] = {{0, 3}, {1, 4}, {3, 5}, {3, 6}, {3, 7}, {7, 8}, {9, 12}, {13, 14}};
    printf("\npairs, reps in which the second was faster than the first:\n");
    for (size_t i = 0; i < sizeof pairs / sizeof pairs[0]; i++) {
        int a = pairs[i][0], b = pairs[i][1], wins = 0; double ratio = 0;
        for (int r = 0; r < reps; r++) { if (ns[b][r] < ns[a][r]) wins++; ratio += (double)ns[b][r] / ns[a][r]; }
        printf("  %-34s vs %-34s %2d/%d, mean ratio %.3f\n", names[a], names[b], wins, reps, ratio / reps);
    }
    supdb_capi_close(sup);
    mdb_txn_abort(ltxn); mdb_env_close(lenv);
    rocksdb_close(rdb);
    return 0;
}
