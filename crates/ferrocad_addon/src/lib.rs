//! The ABI a wasm addon bundle shares: the memory contract, the packed strings, and the
//! `ferrocad.call` import.
//!
//! **The memory contract**, both directions:
//!
//! * an incoming string is `(ptr, len)` into the **bundle's** memory; the loader wrote it
//!   there with [`alloc`];
//! * an outgoing string is a packed `len << 32 | ptr` buffer, also in the bundle's memory;
//!   the loader copies it out and calls [`dealloc`].
//!
//! The document API is the same convention in reverse: the bundle passes the request as
//! `(ptr, len)`, the loader calls the host with it, allocates the response with [`alloc`]
//! in the bundle's memory, writes it, and returns the packed buffer, which the bundle
//! frees. Nothing but text crosses, in either direction, and the document stays the
//! host's.
//!
//! A bundle's `abi.rs` is then only the exported entry points, so the unsafe half exists
//! once. `www/addons/loader.js` implements the other side, and `www/addons/loader.test.mjs`
//! runs a bundle under Node.

use std::alloc::{self, Layout};

#[cfg(target_family = "wasm")]
#[link(wasm_import_module = "ferrocad")]
unsafe extern "C" {
    /// The host's document API: `(ptr, len)` into the bundle's memory; packed
    /// `(len << 32 | ptr)` back, allocated with [`alloc`].
    fn call(ptr: *const u8, len: usize) -> u64;
}

/// Allocate `len` bytes for the loader to write into.
///
/// One allocator serves both directions: every buffer is `Layout::from_size_align(len, 1)`,
/// so [`dealloc`] needs nothing but the same length.
pub fn alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the layout has a non-zero size and alignment 1.
    unsafe { alloc::alloc(layout(len)) }
}

/// Free a buffer [`alloc`] returned.
///
/// # Safety
///
/// `ptr` must come from [`alloc`] with the same `len`, and not be used after.
pub unsafe fn dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        // SAFETY: the caller guarantees the pointer and layout came from `alloc`.
        unsafe { alloc::dealloc(ptr, layout(len)) };
    }
}

/// Copy `text` into a buffer the bundle owns, packed for the loader.
pub fn respond(text: String) -> u64 {
    let bytes = text.into_bytes();
    let len = bytes.len();
    let ptr = alloc(len);
    if ptr.is_null() {
        // The loader reads a packed zero as an empty answer.
        return 0;
    }
    // SAFETY: `ptr` is `len` bytes we just allocated; the source is `len` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len) };
    ((len as u64) << 32) | (ptr as u32 as u64)
}

/// Read a string the loader wrote into the bundle's memory.
///
/// # Safety
///
/// `ptr..ptr + len` must be readable for this call.
pub unsafe fn read(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    // SAFETY: the caller guarantees the range.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// The document API, over the import.
///
/// wasm only: on the host a bundle's logic is handed a closure instead, which is what
/// makes it a normal `cargo test`.
#[cfg(target_family = "wasm")]
pub fn document_api(request: &str) -> String {
    // SAFETY: the request outlives the call.
    let packed = unsafe { call(request.as_ptr(), request.len()) };
    let ptr = packed as u32 as *mut u8;
    let len = (packed >> 32) as usize;
    // SAFETY: the loader allocated this buffer with `alloc` and wrote `len` bytes there.
    let answer = unsafe { read(ptr, len) };
    // SAFETY: the same buffer and length the loader allocated.
    unsafe { dealloc(ptr, len) };
    answer
}

/// The display hook's `have` argument: the objects the backend already meshed.
pub fn have(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

fn layout(len: usize) -> Layout {
    // SAFETY: alignment 1, and every caller checks `len != 0` first.
    unsafe { Layout::from_size_align_unchecked(len, 1) }
}
