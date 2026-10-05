// Minimal C ABI over the OCCT calls the lineage spike needs.
//
// OCCT is C++ with no C ABI of its own. Rather than depend on a third-party Rust
// binding's (unverified, offline) coverage of shape history, the spike binds a
// handful of `extern "C"` functions we control. This doubles as the first sketch
// of `ferrocad_geom_occt`'s shim. See ../../docs/occt-history-spike.md.
#ifndef FERROCAD_OCCT_SHIM_H
#define FERROCAD_OCCT_SHIM_H

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handles.
typedef struct OcctShape OcctShape;
typedef struct OcctHistory OcctHistory;

// Sub-shape kinds (stable across the boundary).
enum {
    OCCT_KIND_SOLID = 0,
    OCCT_KIND_FACE = 1,
    OCCT_KIND_EDGE = 2,
    OCCT_KIND_VERTEX = 3,
};

// Solvers. Coordinates are the box corner (x,y,z) plus its (dx,dy,dz) extents.
OcctShape* occt_make_box(double x, double y, double z, double dx, double dy, double dz);
// Fuse two shapes. On success returns the result and, if `history_out` is non-null,
// a history handle the caller must free. Returns NULL on failure.
OcctShape* occt_fuse(const OcctShape* a, const OcctShape* b, OcctHistory** history_out);
// Round the `edge_ordinal`-th edge (1-based, TopExp order) of `shape`.
OcctShape* occt_fillet(const OcctShape* shape, int edge_ordinal, double radius, OcctHistory** history_out);

void occt_shape_free(OcctShape* shape);
void occt_history_free(OcctHistory* history);

// Number of sub-shapes of `kind` in `shape`.
int occt_shape_count(const OcctShape* shape, int kind);

// History queries: how many output sub-shapes the `ordinal`-th input sub-shape
// (1-based) was mapped to. `-1` signals a bad handle/index.
int occt_history_modified(const OcctHistory* history, const OcctShape* input, int kind, int ordinal);
int occt_history_generated(const OcctHistory* history, const OcctShape* input, int kind, int ordinal);
// 1 if the input sub-shape was deleted, 0 if not, -1 on error.
int occt_history_deleted(const OcctHistory* history, const OcctShape* input, int kind, int ordinal);

#ifdef __cplusplus
}
#endif

#endif  // FERROCAD_OCCT_SHIM_H
