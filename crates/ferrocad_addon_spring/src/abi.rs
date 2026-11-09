//! The bundle's exported entry points, over [`ferrocad_addon`]'s ABI.

#[unsafe(no_mangle)]
pub extern "C" fn ferrocad_alloc(len: usize) -> *mut u8 {
    ferrocad_addon::alloc(len)
}

/// # Safety
///
/// `ptr` must come from [`ferrocad_alloc`] with the same `len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferrocad_dealloc(ptr: *mut u8, len: usize) {
    // SAFETY: the loader passes back a buffer it allocated through `ferrocad_alloc`.
    unsafe { ferrocad_addon::dealloc(ptr, len) }
}

/// The bundle's command list, as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn ferrocad_commands() -> u64 {
    ferrocad_addon::respond(crate::spring::commands_json())
}

/// Run a command: the id as `(ptr, len)`; `1` when it ran, `0` otherwise.
///
/// # Safety
///
/// `ptr..ptr + len` must be a valid buffer in this module's memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ferrocad_run(ptr: *const u8, len: usize) -> u32 {
    // SAFETY: the loader wrote `len` bytes at `ptr` for this call.
    let id = unsafe { ferrocad_addon::read(ptr, len) };
    match crate::spring::run(&id, &ferrocad_addon::doorway) {
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
    let have = unsafe { ferrocad_addon::read(ptr, len) };
    let have = ferrocad_addon::have(&have);
    // A hook that cannot draw adds nothing: a failure is the empty answer, not a broken
    // sync.
    ferrocad_addon::respond(
        crate::spring::stage_meshes(&have, &ferrocad_addon::doorway)
            .unwrap_or_else(|_| "[]".to_string()),
    )
}
