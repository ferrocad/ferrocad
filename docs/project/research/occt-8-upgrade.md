# OCCT 8.x: what the upgrade actually costs

Status: audit **and first build** (2026-11-01). The fork compiles against 8.0.1 and
`ferrocad_occt`'s suite passes on it (§2.1); §7 lists what remains unverified.

Question: what does moving from OCCT **7.8.1** to **8.0.1** take in
[`ferrocad_occt_cxx`](https://github.com/ferrocad/ferrocad_occt_cxx) (tag
`v0.3.0-ferrocad.22`) and FerroCAD, and what does it buy?

Related: [`../../developer/architecture/occt-bundling.md`](../../developer/architecture/occt-bundling.md)
§8 (the 7.8.1 decision), [`occt-8-api-audit.md`](occt-8-api-audit.md) (the symbol-level
companion audit), [bschwind/opencascade-rs#237](https://github.com/bschwind/opencascade-rs/issues/237)
(the upstream request).

---

## 1. What 8.x changes that reaches us

| Change | Evidence (`V8_0_1`) | Effect here |
| --- | --- | --- |
| `Standard_Failure` derives from `std::exception` | `Standard_Failure.hxx:28`, `what()` at `:56`, `GetMessageString()` deprecated at `:59` | cxx's `Result` machinery would catch OCCT exceptions natively; on 7.8.1 it cannot |
| C++17 is the minimum | `CMakeLists.txt:71-88` (`BUILD_CPP_STANDARD`, default `C++17`, `CMAKE_CXX_STANDARD_REQUIRED ON`) | our bridge build is pinned to C++11 (§3) |
| `DEFINE_STANDARD_HANDLE` is no longer used by class headers | the macro still exists (`Standard_Handle.hxx:498`) but `Geom_Surface`, `Geom_Curve`, `Poly_Triangulation`, … only use `DEFINE_STANDARD_RTTIEXT` | 34 of our 35 `Handle_*` types lose their C++ definition (§2) |
| Handles are spelled `occ::handle<T>` / `opencascade::handle<T>` | `Standard_Handle.hxx:415-445` (`occ::handle` alias), `#define Handle(Class)` still at `:448` | the macro still works, so our `Handle(…)` call sites survive |
| `Standard_Boolean/Integer/Real/True/False` are deprecated but defined | `Standard_TypeDef.hxx:59-132` | warnings only; we use `Standard_Integer` ×129, `Standard_Real` ×57, `Standard_Boolean` ×19, `Standard_True` ×2 |
| Old NCollection names move to `src/Deprecated/NCollectionAliases/` | `TColgp_HArray1OfPnt.hxx`, `TopTools_HSequenceOfShape.hxx` are deprecated typedef headers | still compiles with warnings; the `Handle_` aliases for them are gone |
| Toolkits are unchanged | all 24 linked toolkits have a `src/*/<TK>/CMakeLists.txt` in the 8.0.1 tree | no link-list change |
| OCCT ships its own migration scripts | `adm/scripts/migration_800/README.md` | a checklist and a sanity check for our own port, not a tool to run on the crate |

Two of those are worth stating plainly:

- **The exception problem solves itself.** On 8.x, `Standard_Failure` *is* a
  `std::exception`, so a `Result`-returning binding is catchable by stock cxx. It still
  does not make `()`-returning bindings safe: cxx only wraps `Result` functions, and a
  `()` shim stays `noexcept`. The checked helpers therefore stay; only the reason for
  hand-writing `catch (const Standard_Failure&)` disappears.
- **OCCT removed ~1,970 `DEFINE_STANDARD_HANDLE` calls** (its own migration README, phase
  5). That is the whole source of the handle breakage, and it is mechanical.

---

## 2. The handle surface, measured

`ferrocad_occt_cxx` declares 35 opaque handle types in its cxx bridges. Checking each
class header at `V8_0_1` for a surviving `DEFINE_STANDARD_HANDLE`:

| Result | Count | Types |
| --- | --- | --- |
| alias still defined | **1** | `Standard_Type` (`Standard_Type.hxx:199`) |
| alias gone | **34** | `Geom_*` (21), `Geom2d_*` (4), `ShapeFix_*` (7), `Poly_Triangulation`, `Law_Function`, `TColgp_HArray1OfPnt`, `TopTools_HSequenceOfShape` |

The earlier estimate in `occt-bundling.md` §8 was that every handle-typed signature must
move to `occ::handle<T>` — a port of 259 `Handle_` occurrences in `src/shape_fix.rs`
alone. **That estimate is too pessimistic.** Because the Rust-side names are opaque to
cxx, the C++ side only needs the *names* to exist. One compatibility header does that:

```cpp
// include/occt8_handle_compat.hxx — included before the bridge headers
#include <Standard_Version.hxx>
#if OCC_VERSION_MAJOR >= 8
#include <Geom_Surface.hxx>
// … one include per class …
using Handle_Geom_Surface = opencascade::handle<Geom_Surface>;
// … 33 more …
#endif
```

Why this is the right shape rather than a rewrite:

- No Rust bridge, no Rust binding, and no `fc_*`-style call site changes. The 34 names
  keep meaning "the OCCT handle to `T`".
- On 8.x every alias is `opencascade::handle<T>`, which is *the* type OCCT's own API uses.
  That removes the MSVC split that issue
  [ferrocad/ferrocad#1](https://github.com/ferrocad/ferrocad/issues/1) and upstream PR
  #234 work around: `Handle_X` is a derived class only in 7.8's MSVC branch, and that
  branch is not taken for an alias we define ourselves.
- It is gated on `OCC_VERSION_MAJOR >= 8`, so 7.8 keeps its own definition and the two
  do not collide.
- `Handle_Standard_Type` must be excluded (8.0.1 still defines it; re-declaring would
  conflict).

Cost: one header, ~34 includes and aliases, plus a `#include` in each bridge header that
uses those names (or one in `bindings_common.hxx`, which they already include).

---

## 2.1 The first 8.0.1 build (2026-11-01)

The existing micromamba recipe fetched a conda-forge `occt=8.0.1` prefix
(`micromamba create -p .occt8/prefix -c conda-forge 'occt=8.0.1=novtk_*'`; 205 MB — the
`all` variant depends on VTK and pulls Python/X11 for 3.5 GB, so pin the build string),
and the fork was compiled against it. Three more fixes
were needed beyond the handle aliases — all API changes the header audit missed, which
is exactly why the build was worth doing:

| Break | Why | Fix |
| --- | --- | --- |
| `TopTools_ListOfShape` (and 11 siblings) undeclared | OCCT 8 dropped the forwarding includes that made the `Deprecated/NCollectionAliases/` typedefs reachable transitively | define the 13 container names in the shim from `NCollection_*` directly |
| `NCollection_List::Size()` returns `size_t` | cxx binds a member through an exact-type member pointer, so `int (T::*)()` no longer matches | free wrapper `list_size()` returning `int`; `Extent()` is the legacy int API and is unchanged |
| `class TopoDS` became `namespace TopoDS` | the casts are now free functions, and a cxx `#[Self]` binding needs a type | seven free wrappers `topods_*`; a small Rust `TopoDS` module re-exports them so every `TopoDS::Vertex(..)` call site is unchanged |

Result: the fork compiles on **8.0.1**, and `ferrocad_occt`'s **78 tests pass** on it
(the workspace was pointed at the local fork with a temporary `[patch.crates-io] path`).
The same 78 pass on 7.8.1 with the published tag, so the shim is dual-version.

This is a **local** verification: nothing is tagged and no CI leg exists yet. Two API
changes reach downstream Rust: `TopTools_ListOfShape::Size()` is now the free
`list_size(&list)` (the FerroCAD codebase never called it; it uses `Extent()`), and
`TopoDS` is a module rather than a type (the call spelling is preserved).

---

## 3. Build-level blockers

These are the things that fail before any API question arises:

1. **`-std=c++11`.** `crates/opencascade-sys/build.rs:142` passes
   `flag_if_supported("-std=c++11")`. OCCT 8's headers use C++17 constructs
   (`inline constexpr` in `Standard_TypeDef.hxx:60/64` alone), so this cannot compile.
   Change to `c++17` — a one-line change, but it also means auditing our own headers for
   accidental C++11-only assumptions (none expected).
2. **The version gate.** `build.rs:4` is `const OCCT_VERSION: (u8, u8) = (7, 8)` and
   `build.rs:207` rejects any major other than 7. It needs a supported range
   (`major >= 7 && (major > 7 || minor >= 8)`) or a feature gate.
3. **`occt-sys` (the `builtin` feature).** `crates/occt-sys/Cargo.toml` is version
   `7.8.1` and vendors the OCCT submodule at `bd2a789f`. Getting `--features builtin` on
   8.x means bumping the crate version and the submodule to `V8_0_1`, and re-checking
   the CMake recipe for the `CMake 4.x removed cmake_minimum_required` note in CI.
4. **CMake probe.** `OcctConfig::detect()` shells out to CMake; nothing in it is
   7.8-specific beyond the version check, but the probe must be run once against a
   conda 8.0.1 prefix to confirm.
5. **The prefix must be an *install*, not a build tree.** Twelve container headers the
   bridges include (the `NCollection` aliases: `TColgp_HArray1OfPnt`,
   `TopTools_HSequenceOfShape`, `TopTools_ListIteratorOfListOfShape`, `TColStd_*`, …) live
   under `src/Deprecated/NCollectionAliases/` in 8.x. They are flattened into the install
   prefix (`CMakeLists.txt:1106-1108`) but are not copied into a build-tree `inc/`. Our
   CI and packaging already use an install prefix, so this works today; it becomes a
   requirement to state, and is measured in [`occt-8-api-audit.md`](occt-8-api-audit.md) §3a.

---

## 4. Where 7.8.1 is pinned today

An upgrade touches all of these, and the list is the real "cost of version ownership":

| Location | What |
| --- | --- |
| `.github/workflows/ci.yml` | two `occt=7.8.1` micromamba prefixes (geometry + link jobs) |
| `.github/workflows/crates.yml` | `occt=7.8.1` (publishes the fork crate) |
| `.github/workflows/release.yml` | Linux and Windows `occt=7.8.1` prefixes |
| `.github/workflows/edition-windows.yml` | `occt=7.8.1` (the manual MSVC harness) |
| `packaging/README.md` | two mentions of a 7.8.1 prefix |
| `docs/developer/architecture/occt-bundling.md` | §3/§8 commands and rationale |
| `workspace .cargo/config.toml` (via `OpenCASCADE_DIR`) | points at `.occt/prefix`, whatever is fetched |

Availability is not a blocker: conda-forge publishes **8.0.1** for `linux-64`,
`linux-aarch64`, `linux-ppc64le`, `osx-64`, `osx-arm64` and `win-64` (checked against
`api.anaconda.org` on 2026-11-01). Ubuntu's `libocct-*` packages are 7.9.2, so apt
cannot provide 8.x — and that 7.9 is also the version-mixing hazard the repository
notes warn about when it leaks into a 7.8.1 link.

---

## 5. What the upgrade buys

- **Exceptions become catchable by cxx.** `Standard_Failure : std::exception` means a
  `Result` binding no longer needs a C++ `catch (const Standard_Failure&)`. This is a
  simplification, not a fix for the abort: a `()` binding still terminates. It does,
  however, make it possible to retire the hand-written per-call try/catch helpers over
  time, and it removes the reason the crash was surprising.
- **The MSVC handle split disappears.** With 8.x there is no `DEFINE_STANDARD_HANDLE` in
  the class headers, so the `Handle_X`-as-derived-class trap (issue #1, upstream PR #234)
  is gone by construction. Our compat alias is the same type everywhere.
- **The `Standard_*` typedefs stay deprecated, so nothing forces a rewrite.** Moving 207
  spellings to `bool`/`int`/`double` is cleanup, not a prerequisite.
- **It does not buy a stable API.** 8.0 is a major release with its own removals; the
  next major will do the same. The durable lesson is that the sys crate must own a
  version-compatibility shim (like the handle header) rather than assume one OCCT.

---

## 6. Staging

| Step | Work | Exit criterion | Status |
| --- | --- | --- | --- |
| **O1** | C++17 + version range in `ferrocad_occt_cxx`; keep 7.8.1 working | 7.8.1 build still green | ✅ 2026-11-01 |
| **O2** | `occt8_handle_compat.hxx`: 34 handle aliases plus 13 container names, gated `>= 8`, included from `bindings_common.hxx` | no bridge-signature changes; 7.8.1 green | ✅ 2026-11-01 |
| **O3** | Compile and test against `occt=8.0.1`, then make it the CI fetch | 78 tests pass on 8.0.1 | ✅ 2026-11-01: fork tag `v0.3.0-ferrocad.23`, workspace re-pinned, CI/packaging fetch `occt=8.0.1=novtk_*` |
| **O4** | Windows leg for 8.0.1; confirm the #234 patch is no longer needed there | `ferrocad_parametric` builds on MSVC/8.0.1 | open |
| **O5** | Flip CI/release/packaging pins to 8.0.1 | green release with 8.0.1 binaries | ✅ pins flipped; a release is still to cut |

O1–O3 were additive and reversible: the code builds on *both* versions, so the upgrade
was a pin change, not a branch. That is also how the removals we could not see from
headers alone were found.

The `builtin` feature still builds the vendored **7.8.1** submodule. That is fine for the
shared default path and keeps 7.8 supported, but a self-contained 8.0.1 build needs the
`crates/occt-sys` submodule bumped as well.

---

## 7. What is not verified

- **Windows and macOS on 8.x.** Only Linux was built. Our aliases do not take the
  MSVC branch of `DEFINE_STANDARD_HANDLECLASS`, so the #234 type split should be gone
  by construction — but that is reasoning, not a build.
- **The `builtin` submodule recipe on 8.x** was not exercised; OCCT 8 may have changed
  CMake options we pass. The verified path is a conda *install* prefix, which is what
  CI already fetches.
- **Behaviour beyond `ferrocad_occt`'s unit tests.** The 78 tests cover fillet, boolean,
  history, tessellation and the Part features, not the full geometry conformance suite
  or a GUI edition run. Algorithm output can still differ from 7.8.1.
- **`GetMessageString()`** is deprecated in 8.0.1 but still present and `inline`-forwarding
  to `what()`, so our 37 `GetMessageString()` call sites keep compiling; prefer `what()`
  behind an `OCC_VERSION_MAJOR` guard when we touch them.
- **Compiler warnings are not clean.** The deprecated `Standard_*` typedefs, the
  deprecated container headers and the 37 `GetMessageString()` sites all warn on 8.0.1.
  Nothing treats warnings as errors, but the noise will hide a real one.

---

## 8. Recommendation

O1–O3 and the pin flip are done: fork `v0.3.0-ferrocad.23`, workspace re-pinned, CI and
packaging fetch `occt=8.0.1=novtk_*`, and the suite is green on 8.0.1 (78/78) and on
7.8.1. What remains is **O4 (Windows)**: the Linux pass proves the shim, not the MSVC
theory, so do not treat the Windows build as verified until that leg is green.

`builtin` still vendors 7.8.1; bump the `occt-sys` submodule separately if a
self-contained 8.0.1 build is needed.

The error-modelling work
([`../tasks/operation-failure-modelling.md`](../tasks/operation-failure-modelling.md))
does not depend on this and should not wait for it. The one place they touch: on 8.x
`OcctError::Exception` can be produced by a stock `Result` binding, but the
domain/kernel *classification* is still the same problem on both versions.
