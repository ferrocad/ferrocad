//! Link-time stubs for the WASI imports OCCT's wasi-sdk build leaves behind.
//!
//! The prebuilt wasm OCCT is compiled as `wasm32-wasip1`, so the linker emits
//! undefined `__imported_wasi_snapshot_preview1_*` symbols for the libc wrappers
//! it kept. Defining those symbols in the same module makes the linker resolve
//! them, and the shipped module then has **no imports at all** — which is what
//! lets `wasm-bindgen`'s `--target web` glue feed it without a WASI polyfill.
//!
//! The set is per linked subset: `--gc-sections` keeps only the wrappers this
//! code reaches. A new OCCT entry point can pull in more (for example
//! `fd_filestat_get`); the CI smoke test asserts the final module has no
//! `wasi_snapshot_preview1.*` imports, so a missing stub fails there rather than
//! in a browser.
//!
//! Behaviour: file APIs report `EBADF`/`ENOENT` (nothing is opened), the
//! environment is empty, and `proc_exit` traps so a kernel abort is visible.
//!
//! # Using it
//!
//! The stubs are referenced by name from OCCT's objects, never from Rust, so a
//! host must call [`install`]: without it the linker can drop this crate's
//! objects and the imports come back.

const EBADF: i32 = 8;
const ENOENT: i32 = 44;

/// Keeps the stubs in the final link.
///
/// Taking each stub's address is what retains its section; the call is otherwise
/// a no-op, and it is cheap enough to keep in a release host.
pub fn install() {
    core::hint::black_box((
        environ_get as *const () as usize,
        environ_sizes_get as *const () as usize,
        clock_time_get as *const () as usize,
        fd_close as *const () as usize,
        fd_fdstat_get as *const () as usize,
        fd_fdstat_set_flags as *const () as usize,
        fd_prestat_get as *const () as usize,
        fd_prestat_dir_name as *const () as usize,
        fd_read as *const () as usize,
        fd_seek as *const () as usize,
        fd_write as *const () as usize,
        path_open as *const () as usize,
        proc_exit as *const () as usize,
    ));
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_environ_get")]
pub unsafe extern "C" fn environ_get(_environ: *mut *mut u8, _buf: *mut u8) -> i32 {
    0
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_environ_sizes_get")]
pub unsafe extern "C" fn environ_sizes_get(count: *mut i32, size: *mut i32) -> i32 {
    // SAFETY: WASI passes writable out-pointers; null is tolerated here.
    unsafe {
        if !count.is_null() {
            *count = 0;
        }
        if !size.is_null() {
            *size = 0;
        }
    }
    0
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_clock_time_get")]
pub unsafe extern "C" fn clock_time_get(_id: i32, _precision: i64, time: *mut u64) -> i32 {
    // SAFETY: WASI passes a writable out-pointer; null is tolerated here.
    unsafe {
        if !time.is_null() {
            *time = 0;
        }
    }
    0
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_close")]
pub unsafe extern "C" fn fd_close(_fd: i32) -> i32 {
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_fdstat_get")]
pub unsafe extern "C" fn fd_fdstat_get(_fd: i32, _buf: *mut u8) -> i32 {
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_fdstat_set_flags")]
pub unsafe extern "C" fn fd_fdstat_set_flags(_fd: i32, _flags: i32) -> i32 {
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_prestat_get")]
pub unsafe extern "C" fn fd_prestat_get(_fd: i32, _buf: *mut u8) -> i32 {
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_prestat_dir_name")]
pub unsafe extern "C" fn fd_prestat_dir_name(_fd: i32, _path: *mut u8, _len: i32) -> i32 {
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_read")]
pub unsafe extern "C" fn fd_read(
    _fd: i32,
    _iovs: *const u8,
    _iovs_len: i32,
    nread: *mut i32,
) -> i32 {
    if !nread.is_null() {
        // SAFETY: WASI passes a writable out-pointer; null is tolerated here.
        unsafe { *nread = 0 };
    }
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_seek")]
pub unsafe extern "C" fn fd_seek(
    _fd: i32,
    _offset: i64,
    _whence: i32,
    newoffset: *mut u64,
) -> i32 {
    if !newoffset.is_null() {
        // SAFETY: WASI passes a writable out-pointer; null is tolerated here.
        unsafe { *newoffset = 0 };
    }
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_write")]
pub unsafe extern "C" fn fd_write(
    _fd: i32,
    _iovs: *const u8,
    _iovs_len: i32,
    nwritten: *mut i32,
) -> i32 {
    if !nwritten.is_null() {
        // SAFETY: WASI passes a writable out-pointer; null is tolerated here.
        unsafe { *nwritten = 0 };
    }
    EBADF
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_path_open")]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn path_open(
    _fd: i32,
    _dirflags: i32,
    _path: *const u8,
    _path_len: i32,
    _oflags: i32,
    _rights_base: i64,
    _rights_inheriting: i64,
    _fdflags: i32,
    _opened_fd: *mut i32,
) -> i32 {
    ENOENT
}

#[unsafe(export_name = "__imported_wasi_snapshot_preview1_proc_exit")]
pub unsafe extern "C" fn proc_exit(_code: i32) -> ! {
    core::arch::wasm32::unreachable()
}
