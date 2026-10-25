# Vendored `opencascade-sys` 0.3.0

Source: <https://crates.io/crates/opencascade-sys/0.3.0>
(sha256 `f29448a0152b2b23812c4887c3fe9ee79ecb6ba8a72dfecb40b155769b168acc`),
from the extracted registry copy at
`~/.cargo/registry/src/index.crates.io-*/opencascade-sys-0.3.0/`, with
`Cargo.lock` and `.cargo-ok` removed. The crate is licensed LGPL-2.1 by its
`Cargo.toml`.

Pinned through the workspace `[patch.crates-io]` entry:

```toml
opencascade-sys = { path = "third_party/opencascade-sys" }
```

## Patch: construct the `Handle_*` type that `unique_ptr` owns

Upstream's bridge headers allocate `new opencascade::handle<T>(...)` and hand
it to `std::unique_ptr<Handle_T>`. That only compiles where `Handle_T` is a
typedef of `opencascade::handle<T>`. OCCT 7.8's `Standard_Handle.hxx` makes
`Handle_T` a *derived class* on MSVC (`_MSC_VER >= 1800`, for C++/CLI export),
so the base-pointer produced by `new opencascade::handle<T>` cannot convert to
the `Handle_T*` that `unique_ptr` stores (MSVC error C2440).

All 17 sites in `include/` now construct `new Handle_T(...)` instead. That is
the crate's own idiom elsewhere (`t_col_gp.hxx`, `law.hxx`, `poly.hxx`,
`geom.hxx`) and is valid under both definitions: the MSVC class has converting
constructors from `opencascade::handle<U>` and from `T*`, while on GCC/Clang
`Handle_T` is the template itself.

Changed files and lines (17 allocations in 6 headers):

- `include/b_rep.hxx`: `Geom_Surface`, `Geom_Curve`, `Poly_Triangulation`
- `include/gc.hxx`: `Geom_TrimmedCurve` (x2), `Geom2d_TrimmedCurve`
- `include/geom.hxx`: `Geom_CylindricalSurface`, `Geom_Surface` (x2),
  `Geom_BezierSurface`, `Geom_Plane`
- `include/geom_api.hxx`: `Geom_BSplineCurve`
- `include/geom2d.hxx`: `Geom2d_Ellipse`, `Geom2d_Curve` (x2),
  `Geom2d_TrimmedCurve`
- `include/top_tools.hxx`: `TopTools_HSequenceOfShape`

Report upstream at <https://github.com/bschwind/opencascade-rs> (and drop this
copy once a release carries the fix). No other file differs from 0.3.0.
