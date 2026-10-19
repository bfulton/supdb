/* A CPython extension over the C ABI in capi/: the smallest binding that
 * offers both reads, written the way a binding meant for a hot loop is --
 * a type holding the handle and single-argument methods (METH_O), so a
 * call parses no tuple. `get` returns bytes, one copy, as py-lmdb's default
 * does; `view` returns a memoryview over the store's own bytes, no copy,
 * as py-lmdb's buffers=True does. Built by run.sh. */
#define PY_SSIZE_T_CLEAN
#include <Python.h>
#include "capi/supdb_capi.h"

typedef struct { PyObject_HEAD supdb_capi_handle *h; } Store;

static void store_dealloc(Store *self) {
    if (self->h) supdb_capi_close(self->h);
    Py_TYPE(self)->tp_free((PyObject *)self);
}

static int store_init(Store *self, PyObject *args, PyObject *kw) {
    const char *dir;
    if (!PyArg_ParseTuple(args, "s", &dir)) return -1;
    self->h = supdb_capi_open(dir, 0);
    if (!self->h) { PyErr_SetString(PyExc_OSError, supdb_capi_error()); return -1; }
    return 0;
}

static inline int key_of(PyObject *key, const uint8_t **p, size_t *n) {
    if (!PyBytes_Check(key)) { PyErr_SetString(PyExc_TypeError, "key must be bytes"); return -1; }
    *p = (const uint8_t *)PyBytes_AS_STRING(key);
    *n = (size_t)PyBytes_GET_SIZE(key);
    return 0;
}

static PyObject *store_get(Store *self, PyObject *key) {
    const uint8_t *k; size_t klen;
    if (key_of(key, &k, &klen)) return NULL;
    const uint8_t *p; size_t n;
    int rc = supdb_capi_get_borrow(self->h, k, klen, &p, &n);
    if (rc < 0) { PyErr_SetString(PyExc_OSError, supdb_capi_error()); return NULL; }
    if (rc == 0) Py_RETURN_NONE;
    return PyBytes_FromStringAndSize((const char *)p, (Py_ssize_t)n);
}

static PyObject *store_view(Store *self, PyObject *key) {
    const uint8_t *k; size_t klen;
    if (key_of(key, &k, &klen)) return NULL;
    const uint8_t *p; size_t n;
    int rc = supdb_capi_get_borrow(self->h, k, klen, &p, &n);
    if (rc < 0) { PyErr_SetString(PyExc_OSError, supdb_capi_error()); return NULL; }
    if (rc == 0) Py_RETURN_NONE;
    return PyMemoryView_FromMemory((char *)p, (Py_ssize_t)n, PyBUF_READ);
}

/* The value's length alone: the call with no object made for the value,
 * which is the floor a binding can reach. */
static PyObject *store_len_of(Store *self, PyObject *key) {
    const uint8_t *k; size_t klen;
    if (key_of(key, &k, &klen)) return NULL;
    const uint8_t *p; size_t n;
    int rc = supdb_capi_get_borrow(self->h, k, klen, &p, &n);
    if (rc < 0) { PyErr_SetString(PyExc_OSError, supdb_capi_error()); return NULL; }
    return PyLong_FromSize_t(rc == 1 ? n : 0);
}

static PyMethodDef store_methods[] = {
    {"get", (PyCFunction)store_get, METH_O, "get(key) -> bytes | None (one copy)"},
    {"view", (PyCFunction)store_view, METH_O, "view(key) -> memoryview | None (no copy)"},
    {"len_of", (PyCFunction)store_len_of, METH_O, "len_of(key) -> int (no value object)"},
    {NULL, NULL, 0, NULL}
};

static PyTypeObject StoreType = {
    PyVarObject_HEAD_INIT(NULL, 0)
    .tp_name = "supdbpy.Store",
    .tp_basicsize = sizeof(Store),
    .tp_flags = Py_TPFLAGS_DEFAULT,
    .tp_doc = "A supdb store opened for reading",
    .tp_methods = store_methods,
    .tp_init = (initproc)store_init,
    .tp_new = PyType_GenericNew,
    .tp_dealloc = (destructor)store_dealloc,
};

static struct PyModuleDef mod = {PyModuleDef_HEAD_INIT, "supdbpy", NULL, -1, NULL};
PyMODINIT_FUNC PyInit_supdbpy(void) {
    if (PyType_Ready(&StoreType) < 0) return NULL;
    PyObject *m = PyModule_Create(&mod);
    if (!m) return NULL;
    Py_INCREF(&StoreType);
    PyModule_AddObject(m, "Store", (PyObject *)&StoreType);
    return m;
}
