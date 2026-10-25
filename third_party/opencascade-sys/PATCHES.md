# Vendored `opencascade-sys` 0.3.0 + upstream PR #234 (Windows/MSVC)

Source: <https://crates.io/crates/opencascade-sys/0.3.0>
(sha256 `f29448a0152b2b23812c4887c3fe9ee79ecb6ba8a72dfecb40b155769b168acc`),
from the extracted registry copy at
`~/.cargo/registry/src/index.crates.io-*/opencascade-sys-0.3.0/`, with
`Cargo.lock` and `.cargo-ok` removed. The crate is licensed LGPL-2.1 by its
`Cargo.toml`.

It is a verbatim 0.3.0 with **[bschwind/opencascade-rs#234]** applied
(branch `fix-msvc-windows-build`, head `00b4505`). Only the hunks under
`crates/opencascade-sys/` are taken; that PR also updates
`crates/opencascade/`, a crate this workspace does not use.

Pinned through the workspace `[patch.crates-io]` entry:

```toml
opencascade-sys = { path = "third_party/opencascade-sys" }
```

## Why

`opencascade-sys` 0.3.0 does not compile for Windows/MSVC. OCCT 7.8's
`Standard_Handle.hxx` defines the legacy handle spelling differently per
compiler:

```cpp
#define Handle(Class) opencascade::handle<Class>

#if (defined(_MSC_VER) && _MSC_VER >= 1800)
// For Visual Studio 2013+, define Handle_Class as non-template class to allow
// exporting this type in C++/CLI.
#define DEFINE_STANDARD_HANDLECLASS(C1,C2,BC) class C1; class Handle_##C1 : public Handle(C1) { … };
#else
#define DEFINE_STANDARD_HANDLECLASS(C1,C2,BC) class C1; typedef Handle(C1) Handle_##C1;
#endif
```

So on MSVC `Handle_Geom_Surface` is a **derived class** and
`Handle(Geom_Surface)` is the template — two distinct types; on GCC/Clang they
are the same typedef. The crate's bridge assumed the alias, which surfaced in
three ways:

1. **`unique_ptr` ownership.** `std::unique_ptr<Handle_X>(new
   opencascade::handle<X>(…))` cannot initialize from the base pointer
   (`C2440`). The PR constructs the owned type: `new Handle_X(…)` — identical
   where `Handle_X` is a typedef, and the crate's own idiom elsewhere.
2. **cxx member/function pointers.** A `#[Self = "Type"]` shim takes the
   member's address first, which needs exact type identity
   (`void (*f)(const TopoDS_Face&, const Handle_Poly_Triangulation&) =
   &::BRepLib_ToolTriangulatedShape::ComputeNormals;`), so the `Handle_X`
   parameter no longer matches. The PR wraps `ComputeNormals`, `SetLaw`,
   `ConnectEdgesToWires` and `handle_try_deref<T>` in free functions.
3. **Link closure.** TKernel's `OSD_*` uses the Win32 security/registry API;
   without `advapi32` next to `user32`, linking any binary fails with ~23
   unresolved externals (`SetFileSecurityW`, `OpenProcessToken`,
   `RegOpenKeyExW`, …). The PR adds it in `build.rs`.

This was fixed once before (#196, which closed #27/#178/#179/#180) and
re-introduced when the bridge was split (#199).

## Status

- **Verification here:** `cargo check -p ferrocad_occt -p ferrocad_part`
  (which compiles every split bridge) passes on Linux. The Apple/Windows
  targets cannot be compiled in this environment; the Windows check is the
  dispatch-only `.github/workflows/edition-windows.yml`
  (`gh workflow run edition-windows.yml`).
- The vendored directory **must keep the name `opencascade-sys`**: the headers
  use `#include "opencascade-sys/include/…"`, which only resolves when the crate
  directory has that exact name (the registry checkout is
  `opencascade-sys-0.3.0`).
- Tracking issue: <https://github.com/ferrocad/ferrocad/issues/1>.

## Removal

Delete this directory and the `[patch.crates-io]` entry once a published
`opencascade-sys` release contains #234 (or an equivalent fix). Do not extend
this copy with local edits; upstream the change instead.

[bschwind/opencascade-rs#234]: https://github.com/bschwind/opencascade-rs/pull/234
