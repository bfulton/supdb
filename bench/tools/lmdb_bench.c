/* An independent reading of LMDB on the suite's shapes, against the LMDB
 * the suite links -- the source `lmdb-master-sys` vendors, which
 * `bench/scripts/comparators.sh` compiles this against. LMDB ships no
 * benchmark tool, so this stands where RocksDB's db_bench stands.
 *
 *   lmdb_bench DIR N MAP_GB [BATCH] [SCAN]
 *
 * N keys of sixteen decimal digits (`db_key_into`'s shape) with values of
 * a hundred bytes, loaded in order in write transactions of BATCH keys
 * (1000, the suite's batch), each committed with LMDB's default sync; then
 * N point reads of keys drawn uniformly over the loaded set, on one thread,
 * through one read transaction held across them as the suite's arm holds
 * its own; then N/SCAN ordered scans of SCAN entries (100, the suite's
 * scan length) from a uniform key below the top, through a cursor. Three
 * rates, as the suite's row has them: keys a second loaded, reads a second,
 * entries a second scanned. The environment's map is MAP_GB gibibytes, the
 * suite's `lmdb_map_gb`, and readahead is left on, as the `lmdb` arm's is.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "lmdb.h"

static double now_s(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec * 1e-9;
}

static uint64_t rng = 0x9E3779B97F4A7C15ull;
static uint64_t next_u64(void) {
    rng ^= rng << 13;
    rng ^= rng >> 7;
    rng ^= rng << 17;
    return rng;
}

static void key_into(uint64_t n, unsigned char *buf) {
    for (int i = 15; i >= 0; i--) {
        buf[i] = (unsigned char)('0' + n % 10);
        n /= 10;
    }
}

static int fail(const char *what, int rc) {
    fprintf(stderr, "lmdb_bench: %s: %s\n", what, mdb_strerror(rc));
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 4) {
        fprintf(stderr, "usage: lmdb_bench DIR N MAP_GB [BATCH] [SCAN]\n");
        return 2;
    }
    const char *dir = argv[1];
    uint64_t n = strtoull(argv[2], NULL, 10);
    size_t map_gb = (size_t)strtoull(argv[3], NULL, 10);
    uint64_t batch = argc > 4 ? strtoull(argv[4], NULL, 10) : 1000;
    uint64_t scan = argc > 5 ? strtoull(argv[5], NULL, 10) : 100;
    if (n == 0 || batch == 0 || scan == 0 || scan > n) {
        fprintf(stderr, "lmdb_bench: N, BATCH and SCAN must be positive, SCAN at most N\n");
        return 2;
    }

    MDB_env *env;
    int rc;
    if ((rc = mdb_env_create(&env))) return fail("env create", rc);
    if ((rc = mdb_env_set_mapsize(env, map_gb * 1024ull * 1024ull * 1024ull))) return fail("mapsize", rc);
    if ((rc = mdb_env_set_maxdbs(env, 4))) return fail("maxdbs", rc);
    if ((rc = mdb_env_open(env, dir, 0, 0664))) return fail("env open", rc);
    MDB_txn *txn;
    MDB_dbi dbi;
    if ((rc = mdb_txn_begin(env, NULL, 0, &txn))) return fail("txn begin", rc);
    if ((rc = mdb_dbi_open(txn, NULL, 0, &dbi))) return fail("dbi open", rc);
    if ((rc = mdb_txn_commit(txn))) return fail("txn commit", rc);

    unsigned char kb[16];
    unsigned char vb[100];
    MDB_val k = {16, kb}, v = {100, vb};

    /* The load: in order, BATCH keys a transaction, each commit synced. */
    double t0 = now_s();
    for (uint64_t i = 0; i < n;) {
        if ((rc = mdb_txn_begin(env, NULL, 0, &txn))) return fail("load txn", rc);
        uint64_t end = i + batch < n ? i + batch : n;
        for (; i < end; i++) {
            key_into(i, kb);
            for (int j = 0; j < 100; j++) vb[j] = (unsigned char)(i * 31 + j);
            if ((rc = mdb_put(txn, dbi, &k, &v, 0))) return fail("put", rc);
        }
        if ((rc = mdb_txn_commit(txn))) return fail("load commit", rc);
    }
    double load_s = now_s() - t0;

    /* The point reads: N of them, uniform, one read transaction. */
    MDB_txn *rt;
    if ((rc = mdb_txn_begin(env, NULL, MDB_RDONLY, &rt))) return fail("read txn", rc);
    uint64_t sink = 0;
    t0 = now_s();
    for (uint64_t i = 0; i < n; i++) {
        key_into(next_u64() % n, kb);
        MDB_val got;
        if ((rc = mdb_get(rt, dbi, &k, &got))) return fail("get", rc);
        sink += got.mv_size + ((unsigned char *)got.mv_data)[0];
    }
    double read_s = now_s() - t0;

    /* The scans: N/SCAN of them, SCAN entries from a uniform key below the
     * top, so every scan walks SCAN loaded entries. */
    uint64_t scans = n / scan ? n / scan : 1;
    uint64_t below = n - scan ? n - scan : 1;
    MDB_cursor *cur;
    if ((rc = mdb_cursor_open(rt, dbi, &cur))) return fail("cursor", rc);
    uint64_t entries = 0;
    t0 = now_s();
    for (uint64_t s = 0; s < scans; s++) {
        key_into(next_u64() % below, kb);
        MDB_val ck = {16, kb}, cv;
        rc = mdb_cursor_get(cur, &ck, &cv, MDB_SET_RANGE);
        for (uint64_t e = 0; e < scan && rc == 0; e++) {
            sink += cv.mv_size + ((unsigned char *)cv.mv_data)[0];
            entries++;
            rc = mdb_cursor_get(cur, &ck, &cv, MDB_NEXT);
        }
        if (rc && rc != MDB_NOTFOUND) return fail("cursor get", rc);
    }
    double scan_s = now_s() - t0;
    mdb_cursor_close(cur);
    mdb_txn_abort(rt);
    mdb_env_close(env);

    printf("lmdb n %llu load_ops_per_s %.0f read_ops_per_s %.0f scan_entries_per_s %.0f sink %llu\n",
           (unsigned long long)n, (double)n / load_s, (double)n / read_s,
           (double)entries / scan_s, (unsigned long long)sink);
    return 0;
}
