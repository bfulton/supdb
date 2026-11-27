/* A stand-in for the C library's header, for ring's build when the lint group type-checks the macOS target on Linux (scripts/check.sh lint); its objects are never linked. */
#pragma once
#include <stddef.h>
void abort(void);
void *malloc(size_t);
void free(void *);
