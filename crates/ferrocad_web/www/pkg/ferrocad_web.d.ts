/* tslint:disable */
/* eslint-disable */

/**
 * Chroma subsampling format
 */
export enum ChromaSampling {
    /**
     * Both vertically and horizontally subsampled.
     */
    Cs420 = 0,
    /**
     * Horizontally subsampled.
     */
    Cs422 = 1,
    /**
     * Not subsampled.
     */
    Cs444 = 2,
    /**
     * Monochrome.
     */
    Cs400 = 3,
}

/**
 * The document's state after the last recompute, for the page.
 */
export class Status {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * The kernel's reason when `ok` is false.
     */
    readonly error: string | undefined;
    /**
     * Faces in the last built shape.
     */
    readonly faces: number;
    /**
     * Whether the last recompute produced a valid shape.
     */
    readonly ok: boolean;
    /**
     * The radius the document holds, valid or not.
     */
    readonly radius: number;
    /**
     * Triangles in the last built shape.
     */
    readonly triangles: number;
}

/**
 * The document's state, without rebuilding it.
 */
export function fillet_status(): Status;

export function main(): void;

/**
 * Recompute the document's fillet at `radius` and report the result.
 *
 * The geometry on screen is the last **valid** result: `Part::Fillet` keeps its
 * last good shape when the kernel refuses, so a radius that cannot exist leaves
 * the box visible and reports why — the state the failure model is about.
 */
export function set_fillet_radius(radius: number): Status;

/**
 * Exercise `BRepFeat_SplitShape` and report upstream's two groups.
 *
 * The page never splits a shape, so nothing else here would notice if `TKFeat`
 * fell out of the wasm kernel again: `split_shape` stays reachable through the
 * `GeometryBackend` vtable, and a missing toolkit only shows up as a link error
 * when something builds *this* module. The Node smoke test calls this so the
 * toolkit is exercised by behaviour too, not only by linking.
 *
 * It is the native `make_split_shape_returns_two_groups` case from
 * `ferrocad_part_py`: a 1×2×2 box, its first face, and one of that face's edges
 * shifted 0.5 mm along the face as the cutter. Upstream splits the face into one
 * `DirectLeft` and one `Left` piece.
 *
 * Returns `[direct, left]` piece counts, or the kernel's error.
 */
export function split_check(): Uint32Array;

/**
 * The demo document's triangle count, for the Node smoke test.
 *
 * Builds the same `Part` document as the page — box, fillet, `recompute` — so the
 * smoke test covers the workbench and document layers, not just the kernel.
 */
export function triangles(): number;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly cxxbridge1$rust_vec$bool$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$bool$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$bool$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$bool$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$bool$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$bool$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$bool$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$bool$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$f32$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$f32$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$f32$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$f32$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$f64$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$f64$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$f64$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$f64$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i16$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$i16$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$i16$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$i16$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$str$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$string$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$string$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$string$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$string$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u8$drop: (a: number) => void;
    readonly cxxbridge1$slice$len: (a: number) => number;
    readonly cxxbridge1$slice$new: (a: number, b: number, c: number) => void;
    readonly cxxbridge1$slice$ptr: (a: number) => number;
    readonly cxxbridge1$str$from: (a: number, b: number, c: number) => number;
    readonly cxxbridge1$str$len: (a: number) => number;
    readonly cxxbridge1$str$new: (a: number) => void;
    readonly cxxbridge1$str$ref: (a: number, b: number) => void;
    readonly cxxbridge1$string$capacity: (a: number) => number;
    readonly cxxbridge1$string$clone: (a: number, b: number) => void;
    readonly cxxbridge1$string$drop: (a: number) => void;
    readonly cxxbridge1$string$from_utf16: (a: number, b: number, c: number) => number;
    readonly cxxbridge1$string$from_utf16_lossy: (a: number, b: number, c: number) => void;
    readonly cxxbridge1$string$from_utf8: (a: number, b: number, c: number) => number;
    readonly cxxbridge1$string$from_utf8_lossy: (a: number, b: number, c: number) => void;
    readonly cxxbridge1$string$len: (a: number) => number;
    readonly cxxbridge1$string$new: (a: number) => void;
    readonly cxxbridge1$string$ptr: (a: number) => number;
    readonly cxxbridge1$string$reserve_additional: (a: number, b: number) => void;
    readonly cxxbridge1$string$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$exception: (a: number, b: number, c: number) => void;
    readonly __imported_wasi_snapshot_preview1_clock_time_get: (a: number, b: bigint, c: number) => number;
    readonly __imported_wasi_snapshot_preview1_environ_get: (a: number, b: number) => number;
    readonly __imported_wasi_snapshot_preview1_environ_sizes_get: (a: number, b: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_close: (a: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_fdstat_get: (a: number, b: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_fdstat_set_flags: (a: number, b: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_prestat_dir_name: (a: number, b: number, c: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_read: (a: number, b: number, c: number, d: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_seek: (a: number, b: bigint, c: number, d: number) => number;
    readonly __imported_wasi_snapshot_preview1_path_open: (a: number, b: number, c: number, d: number, e: number, f: bigint, g: bigint, h: number, i: number) => number;
    readonly __imported_wasi_snapshot_preview1_proc_exit: (a: number) => void;
    readonly __wbg_status_free: (a: number, b: number) => void;
    readonly fillet_status: () => [number, number, number];
    readonly main: () => void;
    readonly set_fillet_radius: (a: number) => [number, number, number];
    readonly split_check: () => [number, number, number, number];
    readonly status_error: (a: number) => [number, number];
    readonly status_faces: (a: number) => number;
    readonly status_ok: (a: number) => number;
    readonly status_radius: (a: number) => number;
    readonly status_triangles: (a: number) => number;
    readonly triangles: () => number;
    readonly cxxbridge1$rust_vec$i32$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$i64$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$isize$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$str$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$u16$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$u32$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$u64$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$usize$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$char$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$f32$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$f64$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i16$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i32$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i64$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i8$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$isize$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$str$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u16$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u32$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u64$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u8$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$usize$truncate: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$char$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$f32$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$f64$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i16$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i32$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i64$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i8$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$isize$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$str$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$string$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u16$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u32$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u64$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u8$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$usize$set_len: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$char$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$f32$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$f64$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$i16$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$i32$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$i64$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$i8$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$isize$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$str$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$string$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$u16$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$u32$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$u64$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$u8$capacity: (a: number) => number;
    readonly cxxbridge1$rust_vec$usize$capacity: (a: number) => number;
    readonly cxxbridge1$str$ptr: (a: number) => number;
    readonly cxxbridge1$rust_vec$char$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$i32$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$i64$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$i8$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$isize$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$str$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$string$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$u16$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$u32$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$u64$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$u8$new: (a: number) => void;
    readonly cxxbridge1$rust_vec$usize$new: (a: number) => void;
    readonly __imported_wasi_snapshot_preview1_fd_write: (a: number, b: number, c: number, d: number) => number;
    readonly __imported_wasi_snapshot_preview1_fd_prestat_get: (a: number, b: number) => number;
    readonly cxxbridge1$rust_vec$char$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$i32$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$i64$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$i8$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$isize$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$str$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$u16$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$u32$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$u64$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$u8$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$usize$len: (a: number) => number;
    readonly cxxbridge1$rust_vec$char$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$f32$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$f64$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$i16$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$i32$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$i64$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$i8$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$isize$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$str$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$string$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$u16$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$u32$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$u64$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$u8$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$usize$data: (a: number) => number;
    readonly cxxbridge1$rust_vec$char$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i32$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i64$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$i8$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$isize$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u16$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u32$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u64$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$u8$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$usize$reserve_total: (a: number, b: number) => void;
    readonly cxxbridge1$rust_vec$char$drop: (a: number) => void;
    readonly cxxbridge1$rust_vec$i8$drop: (a: number) => void;
    readonly wasm_bindgen_c829f85ea4a5eb48___convert__closures_____invoke___wasm_bindgen_c829f85ea4a5eb48___sys__JsNullable_wgpu_6210bb69b4d37a00___backend__webgpu__webgpu_sys__gen_GpuAdapter__GpuAdapter___core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_c829f85ea4a5eb48___JsError___true_: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_c829f85ea4a5eb48___convert__closures_____invoke___wasm_bindgen_c829f85ea4a5eb48___sys__JsNullable_wgpu_6210bb69b4d37a00___backend__webgpu__webgpu_sys__gen_GpuAdapter__GpuAdapter___core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_c829f85ea4a5eb48___JsError___true__12: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_c829f85ea4a5eb48___convert__closures_____invoke___wasm_bindgen_c829f85ea4a5eb48___sys__JsNullable_wgpu_6210bb69b4d37a00___backend__webgpu__webgpu_sys__gen_GpuAdapter__GpuAdapter___core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_c829f85ea4a5eb48___JsError___true__13: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_c829f85ea4a5eb48___convert__closures_____invoke___wasm_bindgen_c829f85ea4a5eb48___sys__JsNullable_wgpu_6210bb69b4d37a00___backend__webgpu__webgpu_sys__gen_GpuAdapter__GpuAdapter___core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_c829f85ea4a5eb48___JsError___true__14: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_c829f85ea4a5eb48___convert__closures_____invoke___web_sys_df7ed8bedf85487___features__gen_Event__Event______true_: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_c829f85ea4a5eb48___convert__closures_____invoke_______true_: (a: number, b: number) => void;
    readonly __wbindgen_export: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
