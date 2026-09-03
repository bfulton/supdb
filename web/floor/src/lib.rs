//! The empty wasm module, with the same standard-library surface the reader
//! uses and nothing else. See `Cargo.toml` for why it exists.
//!
//! Every item here is deliberately reachable from an export, so the linker
//! cannot delete the thing being measured.

use std::cell::RefCell;
use std::io::{Error, ErrorKind, Result};

thread_local! {
    static STATE: RefCell<(Vec<Vec<u8>>, String)> =
        const { RefCell::new((Vec::new(), String::new())) };
}

fn work(n: u32) -> Result<u64> {
    if n == 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("nothing to do with {n} items"),
        ));
    }
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.0.push(vec![0u8; n as usize]);
        st.1 = format!("did {n}");
        Ok(st.0.iter().map(|v| v.len() as u64).sum())
    })
}

/// The same allocation shape `supdb_alloc` has, deliberately down to the
/// detail that makes it correct: `vec![0u8; n]` allocates exactly `n` where
/// `with_capacity` may allocate more, and handing the allocator back a
/// capacity it did not give out is undefined behaviour rather than a leak.
/// A control that allocates differently from the thing it controls for is
/// not measuring the same std surface -- and the first version of this file
/// used `with_capacity` and had no `floor_free` at all, so it linked neither
/// the exact-size path nor the deallocation one.
#[no_mangle]
pub extern "C" fn floor_alloc(len: u32) -> *mut u8 {
    let mut v = vec![0u8; len as usize];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// Release memory from `floor_alloc`, as `supdb_free` does.
///
/// # Safety
/// `ptr` and `len` must be exactly what `floor_alloc` returned and was given.
#[no_mangle]
pub unsafe extern "C" fn floor_free(ptr: *mut u8, len: u32) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, 0, len as usize));
    }
}

#[no_mangle]
pub extern "C" fn floor_work(n: u32) -> u64 {
    match work(n) {
        Ok(v) => v,
        Err(e) => {
            STATE.with(|s| s.borrow_mut().1 = e.to_string());
            u64::MAX
        }
    }
}

#[no_mangle]
pub extern "C" fn floor_error_len() -> u32 {
    STATE.with(|s| s.borrow().1.len() as u32)
}

#[no_mangle]
pub extern "C" fn floor_error_ptr() -> *const u8 {
    STATE.with(|s| s.borrow().1.as_ptr())
}
