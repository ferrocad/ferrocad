//! The wasm shell: the exports the loader calls and the `ferrocad.call` import.
//!
//! **The memory contract**, both directions:
//!
//! * an incoming string is `(ptr, len)` into **this** module's memory; the loader wrote
//!   it there with [`ferrocad_alloc`];
//! * an outgoing string is a packed `len << 32 | ptr` buffer, also in this memory; the
//!   loader copies it out and calls [`ferrocad_dealloc`].
//!
//! The doorway is the same convention in reverse: we pass the request as `(ptr, len)`,
//! the loader calls the host with it, allocates the response with [`ferrocad_alloc`] in
//! our memory, writes it, and returns the packed buffer — which we free. Nothing but
//! text crosses, in either direction, and the document stays the host's.
//!
//! `www/addons/loader.js` implements the other side, and
//! `www/addons/loader.test.mjs` runs this module under Node.

use std::alloc::{Layout, alloc, dealloc};

#[link(wasm_import_module = "ferrocad")]
unsafe extern "C" {
    /// The host's document doorway: `(ptr, len)` into this module's memory; packed
    /// `(len << 32 | ptr)` back, allocated with [`ferrocad_alloc`].
    fn call(ptr: *const u8, len: usize) -> u64;
}

/// Allocate `len` bytes for the loader to write into.
///
/// One allocator serves both directions: every buffer is
/// `Layout::from_size_align(len, 1)`, so [`ferrocad_dealloc`] needs nothing but the
/// same length.
#[unsafe(no_mangle)]
pub extern "C" fn ferrocad_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    // SAFETY: the layout has a non-zero size and alignment 1.
    unsafe { alloc(layout(len)) }
}

/// Free a buffer [`ferrocad_alloc`] returned.
///
/// # Safety
///
/// `ptr` must come from [`ferrocad_alloc`] with the same `len`, and not be used after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferrocad_dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        // SAFETY: the caller guarantees the pointer and layout came from `alloc`.
        unsafe { dealloc(ptr, layout(len)) };
    }
}

/// The bundle's command list, as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn ferrocad_commands() -> u64 {
    respond(crate::toy::commands_json())
}

/// Run a command: the id as `(ptr, len)`; `1` when it ran, `0` otherwise.
///
/// # Safety
///
/// `ptr..ptr + len` must be a valid buffer in this module's memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferrocad_run(ptr: *const u8, len: usize) -> u32 {
    // SAFETY: the loader wrote `len` bytes at `ptr` for this call.
    let id = unsafe { read(ptr, len) };
    match crate::toy::run(&id, &doorway) {
        Ok(true) => 1,
        _ => 0,
    }
}

/// The display hook: `have` as JSON in, mesh JSON out.
///
/// # Safety
///
/// `ptr..ptr + len` must be a valid buffer in this module's memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferrocad_stage_meshes(ptr: *const u8, len: usize) -> u64 {
    // SAFETY: the loader wrote `len` bytes at `ptr` for this call.
    let have = unsafe { read(ptr, len) };
    let have: Vec<String> = serde_json::from_str(&have).unwrap_or_default();
    // A hook that cannot draw adds nothing. The host parses an array, so a failure is
    // the empty answer rather than a broken sync.
    respond(crate::toy::stage_meshes(&have, &doorway).unwrap_or_else(|_| "[]".to_string()))
}

/// The doorway over the import, for the pure logic.
fn doorway(request: &str) -> String {
    // SAFETY: the request outlives the call.
    let packed = unsafe { call(request.as_ptr(), request.len()) };
    let ptr = packed as u32 as *mut u8;
    let len = (packed >> 32) as usize;
    // SAFETY: the loader allocated this buffer with `ferrocad_alloc` and wrote `len`
    // bytes there.
    let answer = unsafe { read(ptr, len) };
    // SAFETY: the same buffer and length the loader allocated.
    unsafe { ferrocad_dealloc(ptr, len) };
    answer
}

fn layout(len: usize) -> Layout {
    // SAFETY: alignment 1, and every caller checks `len != 0` first.
    unsafe { Layout::from_size_align_unchecked(len, 1) }
}

/// Copy `text` into a buffer this module owns, packed for the loader.
fn respond(text: String) -> u64 {
    let bytes = text.into_bytes();
    let len = bytes.len();
    let ptr = ferrocad_alloc(len);
    if ptr.is_null() {
        // The loader reads a packed zero as an empty answer.
        return 0;
    }
    // SAFETY: `ptr` is `len` bytes we just allocated; the source is `len` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len) };
    ((len as u64) << 32) | (ptr as u32 as u64)
}

/// # Safety
///
/// `ptr..ptr + len` must be readable for this call.
unsafe fn read(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    // SAFETY: the caller guarantees the range.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}
