/* A stand-in for the C library's header, for ring's build when the lint group type-checks the macOS target on Linux (scripts/check.sh lint); its objects are never linked. */
#pragma once
#include <stddef.h>
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
void *memset(void *, int, size_t);
int memcmp(const void *, const void *, size_t);
size_t strlen(const char *);
