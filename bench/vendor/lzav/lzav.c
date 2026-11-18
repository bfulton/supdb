/* The codec probe's door to LZAV (bench/examples/zcodec.rs): lzav.h's
 * inline functions given symbols the probe can call. Built only with the
 * bench crate's `codecs` feature. lzav.h is upstream's, version 7.1,
 * unmodified, under its own MIT licence. */
#include "lzav.h"

int supdb_lzav_bound(int n) { return lzav_compress_bound(n); }

int supdb_lzav_bound_hi(int n) { return lzav_compress_bound_hi(n); }

int supdb_lzav_compress(const void* src, void* dst, int n, int cap) {
	return lzav_compress_default(src, dst, n, cap);
}

int supdb_lzav_compress_hi(const void* src, void* dst, int n, int cap) {
	return lzav_compress_hi(src, dst, n, cap);
}

int supdb_lzav_decompress(const void* src, void* dst, int n, int len) {
	return lzav_decompress(src, dst, n, len);
}
