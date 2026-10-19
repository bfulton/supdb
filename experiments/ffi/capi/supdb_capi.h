#ifndef SUPDB_CAPI_H
#define SUPDB_CAPI_H
#include <stddef.h>
#include <stdint.h>
typedef struct supdb_capi_handle supdb_capi_handle;
const char *supdb_capi_error(void);
supdb_capi_handle *supdb_capi_open(const char *dir, int32_t create);
int32_t supdb_capi_close(supdb_capi_handle *h);
void supdb_capi_append(supdb_capi_handle *h, const uint8_t *key, size_t klen, const uint8_t *val, size_t vlen);
int32_t supdb_capi_commit(supdb_capi_handle *h);
int32_t supdb_capi_flush(supdb_capi_handle *h);
int32_t supdb_capi_get_borrow(supdb_capi_handle *h, const uint8_t *key, size_t klen, const uint8_t **out, size_t *len);
int32_t supdb_capi_get_copy(supdb_capi_handle *h, const uint8_t *key, size_t klen, uint8_t *buf, size_t cap, size_t *len);
int64_t supdb_capi_read_all(supdb_capi_handle *h, const uint8_t *key, size_t klen, void (*cb)(void *, const uint8_t *, size_t), void *ctx);
uint64_t supdb_capi_bench_native(supdb_capi_handle *h, uint64_t n, uint64_t keyspace, uint64_t seed, int32_t touch, uint64_t *bytes);
uint64_t supdb_capi_bench_native_keys(supdb_capi_handle *h, const uint8_t *keys, uint64_t n, uint64_t *bytes);
#endif
