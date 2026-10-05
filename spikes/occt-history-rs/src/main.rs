//! Rust OCCT binding spike: build, fuse, fillet, and read shape history.
//!
//! This is the Rust rewrite of `../occt-history/probe.cpp`. It drives OCCT through
//! the small C ABI in `shim/`, which is the part a future `ferrocad_geom_occt`
//! would own. The point is not the geometry; it is to see whether shape history
//! (`Modified`/`Generated`/`Deleted`) survives the Rust <-> OCCT boundary and is
//! usable to build an element map. See `../../docs/occt-history-spike.md`.
//!
//! Build/run (OCCT 7.8+ required):
//!   cargo run --release

use std::ffi::c_int;

/// Opaque OCCT shape, owned by the shim.
#[repr(C)]
struct OcctShape {
    _private: [u8; 0],
}

/// Opaque operation history, owned by the shim.
#[repr(C)]
struct OcctHistory {
    _private: [u8; 0],
}

extern "C" {
    fn occt_make_box(
        x: f64,
        y: f64,
        z: f64,
        dx: f64,
        dy: f64,
        dz: f64,
    ) -> *mut OcctShape;
    fn occt_fuse(
        a: *const OcctShape,
        b: *const OcctShape,
        history_out: *mut *mut OcctHistory,
    ) -> *mut OcctShape;
    fn occt_fillet(
        shape: *const OcctShape,
        edge_ordinal: c_int,
        radius: f64,
        history_out: *mut *mut OcctHistory,
    ) -> *mut OcctShape;
    fn occt_shape_free(shape: *mut OcctShape);
    fn occt_history_free(history: *mut OcctHistory);
    fn occt_shape_count(shape: *const OcctShape, kind: c_int) -> c_int;
    fn occt_history_modified(
        history: *const OcctHistory,
        input: *const OcctShape,
        kind: c_int,
        ordinal: c_int,
    ) -> c_int;
    fn occt_history_generated(
        history: *const OcctHistory,
        input: *const OcctShape,
        kind: c_int,
        ordinal: c_int,
    ) -> c_int;
    fn occt_history_deleted(
        history: *const OcctHistory,
        input: *const OcctShape,
        kind: c_int,
        ordinal: c_int,
    ) -> c_int;
}

const SOLID: c_int = 0;
const FACE: c_int = 1;
const EDGE: c_int = 2;
const VERTEX: c_int = 3;

/// RAII wrapper: frees the OCCT shape on drop.
struct Shape(*mut OcctShape);
impl Shape {
    fn ptr(&self) -> *const OcctShape {
        self.0
    }
}
impl Drop for Shape {
    fn drop(&mut self) {
        unsafe { occt_shape_free(self.0) }
    }
}

/// RAII wrapper: frees the OCCT history on drop.
struct History(*mut OcctHistory);
impl History {
    fn ptr(&self) -> *const OcctHistory {
        self.0
    }
}
impl Drop for History {
    fn drop(&mut self) {
        unsafe { occt_history_free(self.0) }
    }
}

fn kind_name(kind: c_int) -> &'static str {
    match kind {
        SOLID => "Solid",
        FACE => "Face",
        EDGE => "Edge",
        _ => "Vertex",
    }
}

fn print_counts(label: &str, shape: &Shape) {
    print!("{label}:");
    for kind in [SOLID, FACE, EDGE, VERTEX] {
        let n = unsafe { occt_shape_count(shape.ptr(), kind) };
        print!(" {n} {}s", kind_name(kind));
    }
    println!();
}

/// One line per input sub-shape: what did the operation do to it?
fn report_history(phase: &str, history: &History, input: &Shape, kind: c_int) {
    let n = unsafe { occt_shape_count(input.ptr(), kind) };
    println!(
        "-- {phase} history over {n} {}s of the input:",
        kind_name(kind)
    );
    for ordinal in 1..=n {
        let modified = unsafe { occt_history_modified(history.ptr(), input.ptr(), kind, ordinal) };
        let generated = unsafe { occt_history_generated(history.ptr(), input.ptr(), kind, ordinal) };
        let deleted = unsafe { occt_history_deleted(history.ptr(), input.ptr(), kind, ordinal) };

        let mut tags = String::new();
        if deleted > 0 {
            tags.push_str(" DELETED");
        }
        if modified > 0 {
            tags.push_str(&format!(" Modified->{modified}"));
        }
        if generated > 0 {
            tags.push_str(&format!(" Generated->{generated}"));
        }
        if tags.is_empty() {
            tags.push_str(" unchanged/unreported");
        }
        println!("   {}{ordinal}:{tags}", kind_name(kind));
    }
}

fn main() {
    println!("OCCT history probe (Rust)\n=========================");

    let a = Shape(unsafe { occt_make_box(0.0, 0.0, 0.0, 10.0, 10.0, 10.0) });
    let b = Shape(unsafe { occt_make_box(5.0, 0.0, 0.0, 10.0, 10.0, 10.0) });

    println!("\n== inputs ==");
    print_counts("box a", &a);
    print_counts("box b", &b);

    let mut fuse_history: *mut OcctHistory = std::ptr::null_mut();
    let fused = Shape(unsafe { occt_fuse(a.ptr(), b.ptr(), &mut fuse_history) });
    if fused.0.is_null() || fuse_history.is_null() {
        eprintln!("fuse failed");
        std::process::exit(2);
    }
    let fuse_history = History(fuse_history);

    println!("\n== fuse output ==");
    print_counts("fused", &fused);

    report_history("FUSE", &fuse_history, &a, FACE);
    report_history("FUSE", &fuse_history, &a, EDGE);
    report_history("FUSE", &fuse_history, &b, FACE);
    report_history("FUSE", &fuse_history, &b, EDGE);

    let mut fillet_history: *mut OcctHistory = std::ptr::null_mut();
    let filleted = Shape(unsafe { occt_fillet(fused.ptr(), 1, 1.0, &mut fillet_history) });
    if filleted.0.is_null() || fillet_history.is_null() {
        eprintln!("fillet failed");
        std::process::exit(3);
    }
    let fillet_history = History(fillet_history);

    println!("\n== fillet output ==");
    print_counts("filleted", &filleted);
    report_history("FILLET", &fillet_history, &fused, FACE);
    report_history("FILLET", &fillet_history, &fused, EDGE);

    println!("\n(note: OCCT never names elements; only history is reported)");
    println!("(a stable element map is our code, built from this history)");
}
