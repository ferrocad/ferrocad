# OCCT 8.0.1 API audit for the binding crate

Status: read-only source audit (2026-11-01) — **not compiled**.

Scope: everything reachable from `ferrocad_occt_cxx/crates/opencascade-sys`
(lib `opencascade_sys`): the 62 headers in `include/` (61 bridge shims plus the
new compat header), the `include!`-ed headers and every `extern "C++"` binding
in `src/*.rs`, checked against OCCT `V8_0_1` and diffed against `V7_8_1` where
the question is "what changed".

Snapshot audited: commit `0aea31b` **plus the uncommitted working-tree changes**
that add `include/occt8_handle_compat.hxx`, include it from
`bindings_common.hxx`, and move `build.rs` to `.std("c++17")` with an 8.x
version gate. The handle breakage and the C++ standard are analysed in
[`occt-8-upgrade.md`](occt-8-upgrade.md); this note is the rest of the surface.

---

## 1. Verified compatible

Mechanically checked, not sampled (see §4 for what "mechanically" covered):

| Surface | Result |
| --- | --- |
| OCCT headers we include | 218 distinct OCCT `.hxx` headers, all present in the `V8_0_1` tree. 12 of them exist **only** under `src/Deprecated/NCollectionAliases/` (§2 row 2, §3a). |
| Linked toolkits | all 24 (`TKMath`, `TKernel`, `TKDE`, `TKFeat`, `TKGeomBase`, `TKG2d`, `TKG3d`, `TKTopAlgo`, `TKGeomAlgo`, `TKBRep`, `TKPrim`, `TKDESTEP`, `TKDEIGES`, `TKDESTL`, `TKMesh`, `TKShHealing`, `TKFillet`, `TKBool`, `TKBO`, `TKOffset`, `TKXSBase`, `TKCAF`, `TKLCAF`, `TKXCAF`) still have `src/<Module>/<TK>/FILES.cmake`; the link list does not change. |
| cxx member bindings | 156 distinct `(class, method)` pairs declared as `fn X(self: …)` resolved against the class or its base-class closure in 8.0.1; **no method disappeared**, and no return-type or constness change that breaks a caller. The only constness deltas are `BRepFilletAPI_MakeFillet::Add` / `MakeChamfer::Add`, which went non-const → `const` (`7.8` `Add(...)` vs 8.0.1 `Add(...) const`), i.e. the safe direction. |
| cxx static bindings | 30 `#[Self = "X"]` bindings (`BRep_Tool`, `BRepGProp`, `BRepBndLib`, `BRepLib`, `BRepTools`, `TopExp`, `TopoDS`, `gp`, `Precision`) all still declared with the same parameter shapes. `Precision::Confusion()` is now `static constexpr` (`Precision.hxx:165`) — still callable. |
| Enums mirrored by value in Rust | all 14 (`TopAbs_ShapeEnum`, `TopAbs_Orientation`, `TopAbs_State`, `GeomAbs_Shape`, `GeomAbs_CurveType`, `GeomAbs_SurfaceType`, `GeomAbs_JoinType`, `BOPAlgo_Operation`, `BOPAlgo_GlueEnum`, `BRepOffset_Mode`, `BRepBuilderAPI_TransitionMode`, `BRepBuilderAPI_WireError`, `IFSelect_ReturnStatus`, `CSLib_NormalStatus`) have **identical ordinal order** in 8.0.1, so the `#[repr(u32)]` mirrors stay ABI-correct. |
| `Handle(Class)` macro | still defined: `Standard_Handle.hxx:448`. Our `Handle(Geom_Curve)::DownCast(...)` call sites (`include/geom.hxx:214`, `:364`) keep working. |
| `DEFINE_STANDARD_HANDLE` | still defined (`Standard_Handle.hxx:498`), and still used by `Standard_Type.hxx:199`, which is why `Handle_Standard_Type` needs no shim (`include/standard.hxx:4`). |
| `opencascade::handle<T>` (question d) | unchanged API: `handle(const T*)` `:67`, copy `:74`, move `:81`, `Nullify` `:91`, `IsNull` `:94`, `reset` `:97`, `get()` `:122`, `operator->` `:125`, `DownCast` `:198`/`:208`; `occ::handle` alias `:415-445`. `bindings_common.hxx:24-29` (`IsNull` + `operator*`) and every `new Handle_X(...)` remain valid. |
| `Standard_Failure` | `class Standard_Failure : public std::exception` (`Standard_Failure.hxx:28`), `what()` `:56`, `GetMessageString()` deprecated-but-inline `:59-60`, `ExceptionType()` `:65`. `Raise()` static helpers are gone, but the crate never calls them. Exception *hierarchy* intact: `Standard_OutOfRange : Standard_RangeError : Standard_DomainError : Standard_Failure` (`Standard_OutOfRange.hxx:42`, `Standard_RangeError.hxx:39`, `Standard_DomainError.hxx:32`), so `catch (const Standard_Failure&)` still catches everything OCCT throws. |
| `Standard_*` scalar aliases (question b) | all still defined, unconditionally, in `Standard_TypeDef.hxx:59-100`. |
| `BRepFilletAPI_MakeFillet` / `MakeChamfer` | `Build(const Message_ProgressRange&)`, the `Add` overloads, `Shape`, `IsDone`, `Modified`, `Generated`, `IsDeleted` all present; our `Handle_*` names are the only thing that changed. |
| `Message_ProgressRange` | default ctor still present (`Message_ProgressRange.hxx:43`); `op.Build(Message_ProgressRange())` in `include/b_rep_fillet_api.hxx:52` compiles unchanged. |
| `BRepLProp_CLProps` / `BRepLProp_SLProps` | now alias templates: `using BRepLProp_CLProps = GeomLProp_CLPropsBase<gp_Pnt,gp_Vec,gp_Dir,BRepAdaptor_Curve>` (`BRepLProp_CLProps.hxx:20`) and `GeomLProp_SLPropsBase<BRepAdaptor_Surface>` (`BRepLProp_SLProps.hxx:20`). The 4-arg/5-arg constructors, `Normal(Dir&)`, `D1/D2/D3`, `Curvature`, `TangentU/V`, `Is*Defined`, `Min/MaxCurvature` all survive on the template base (`GeomLProp_CLProps.hxx`, `GeomLProp_SLProps.hxx:54-284`). |
| `BRepAdaptor_Curve` / `_Surface` | rebased onto `GeomAdaptor_TransformedCurve` / `…Surface` (`BRepAdaptor_Curve.hxx:37`); `Value`, `D0-D3`, `DN`, `GetType`, `Continuity`, `Bezier`, `BSpline`, `OffsetCurve`, `FirstParameter`… are inherited instead of redeclared. Calls are unchanged. Same for `BRepAdaptor_CompCurve` (2- and 5-arg ctors preserved). |
| `NCollection_List` / `NCollection_Sequence` | `Size()`/`Extent()`/`Length()` moved into the bases but are still there: `NCollection_BaseList.hxx:99-105`, `NCollection_BaseSequence.hxx:108-111`. `include/top_tools.hxx:27` (`handle->Length()`) is fine. |
| `BRep_Tool` / `BRep_Builder` / `BRepTools` | `Surface`, `Curve` (both overloads), `CurveOnSurface` (including the 6-arg `(E, C, S, L, first, last)` out-parameter form, `BRep_Tool.hxx:160-165`), `Triangulation`, `Range`, `Degenerated`, `IsClosed`, `Tolerance`; `MakeFace`, `UpdateEdge`, `Range`, `Transfert`, `MakeWire` (`BRep_Builder.hxx:298`, `:318`); `OuterWire`, `Write/Read` (file and stream) all present. |
| Boolean / offset / primitive builders | `BRepAlgoAPI_*` (`SetArguments`, `SetTools`, `SetGlue`, `SetNonDestructive`, `Build`, `Shape`, `IsDone`, `IsDeleted`, `SectionEdges`, `Arguments`, `Tools`, `Modified`, `Generated`), `BRepOffsetAPI_*`, `BRepPrimAPI_*`, `BRepBuilderAPI_*`, `BOPAlgo_BuilderFace`, `BRepFeat_*`, `ChFi2d_*`, `ShapeFix_*` mode setters: no removed symbol in any of these headers between 7.8.1 and 8.0.1 (only `DEFINE_STANDARD_HANDLE` and `Handle` vanish). `BRepOffsetAPI_MakeOffsetShape::MakeOffset()` is still `const` (`…MakeOffsetShape.hxx:118`), which our const shim at `include/b_rep_offset_api.hxx:43-44` needs. |
| I/O and geometry | `StlAPI_Writer::Write(shape, const char*)` (`StlAPI_Writer.hxx:43`), `STEPControl_Writer::Transfer/Write` (`STEPControl_Writer.hxx:90`, `:104`), `STEPControl_AsIs` (`STEPControl_StepModelType.hxx:34`), `IFSelect_ReturnStatus` values (`IFSelect_ReturnStatus.hxx:27-33`), `IGESControl_*`, `BinTools::Write/Read`, `Poly_Triangulation` ctors (`Poly_Triangulation.hxx:77-80`), `BRepGProp::{Linear,Surface,Volume}Properties` (`BRepGProp.hxx:88`, `:133`, `:196`), `Geom_*`/`Geom2d_*`/`GC_*`/`GeomAPI_*` constructors, `Law_*`, `Bnd_Box`, `GProp_*`, `CSLib::Normal` (9-arg overload with `bool& theDone` preserved), `GCPnts_*`, `BSplCLib`, `TopExp::Vertices` (bound as `#[cxx_name = "Vertices"]`, `TopExp.hxx:126-137`), `TopoDS_Shape::Location(setter)` / `Move` / `Orientation` (`TopoDS_Shape.hxx:90-95`, `:193`). |

---

## 2. Incompatible

Ordered by severity. "P1" = breaks the build (or silently changes meaning); "P2" =
compiles with deprecation noise and a migration debt; "P3" = behavioural, only a
running test can confirm.

> **Row 1 was fixed in the tree on 2026-11-01**, after this audit was written: the alias
> is now `NCollection_HSequence<TopoDS_Shape>` with `<NCollection_HSequence.hxx>`
> included. The row is kept as the evidence for why, and because the same mistake is
> easy to reintroduce. The 7.8.1 build is green with the fix; row 1 itself can only be
> compiled once an OCCT 8 prefix exists.

| # | Sev | Our file:line | OCCT 8.0.1 evidence | Required change |
| --- | --- | --- | --- | --- |
| 1 | **P1** | `include/occt8_handle_compat.hxx:98` | `TopTools_HSequenceOfShape` is `NCollection_HSequence<TopoDS_Shape>`, i.e. `class NCollection_HSequence : public NCollection_Sequence<T>, public Standard_Transient` (`NCollection_HSequence.hxx:25`). The compat line aliases the handle to the **non-transient base** `NCollection_Sequence<TopoDS_Shape>` instead of the class, so the handle has no reference counter to manage. | `using Handle_TopTools_HSequenceOfShape = opencascade::handle<NCollection_HSequence<TopoDS_Shape>>;` and add `#include <NCollection_HSequence.hxx>` to the compat header. As written, `handle_try_deref<TopTools_HSequenceOfShape>` (`include/top_tools.hxx:12`) is an instantiation mismatch, `h->FixLoopWire(*wires)` (`include/shape_fix.hxx:695`) passes `NCollection_Sequence&` where `Handle(NCollection_HSequence)&` is expected, and the `OverTolerance`/`InTolerance`/`ConnectEdgesToWires` shims (`include/shape_analysis.hxx:24-58`) cannot convert. This is a compile error, not a warning. |
| 2 | **P1 if the OCCT prefix is a build tree** | `include/t_col_gp.hxx:1-5`, `include/t_col_std.hxx:1-3`, `include/top_tools.hxx:1-4`, `include/shape_analysis.hxx:11`, `include/shape_fix.hxx:21`, `include/geom.hxx:23-27`, `include/geom2d.hxx:5-7`, `include/geom_api.hxx:17`, `include/bspl_clib.hxx:2`, `include/b_rep_algo.hxx:5`, `include/b_rep_feat.hxx:10`, `include/b_rep_fillet_api.hxx:9`, `include/b_rep_lib.hxx:7`, `include/b_rep_offset_api.hxx:12`, `bop_algo.hxx:6`, `topo_ds.hxx:1` | 12 container headers exist **only** in `src/Deprecated/NCollectionAliases/`. They *are* installed by a normal `cmake --install`: `install(DIRECTORY src/Deprecated/NCollectionAliases/ DESTINATION "${INSTALL_DIR_INCLUDE}")` (`CMakeLists.txt:1106-1108`), which flattens them next to the real headers. They are **not** copied into the build-tree `inc/` directory, because `COLLECT_AND_INSTALL_OCCT_HEADER_FILES` walks only `<BUILD_TOOLKITS>/<Package>` (`adm/cmake/occt_macros.cmake:353-412`). | Today's `occ_info.txt` points at `.occt/prefix/include/opencascade` (an install tree), so this works — but state it as a requirement (OCCT must be *installed*, not merely built) or remove the dependency by spelling the modern types (full mapping in §3a). Each inclusion also emits one `#pragma GCC warning` through `Standard_HEADER_DEPRECATED` (`Standard_Macro.hxx:50-53`). |
| 3 | P2 | `include/top_tools.hxx` (`type TopTools_ListIteratorOfListOfShape`, `src/top_tools.rs`) | There is no `TopTools_ListIteratorOfListOfShape.hxx` and no such class in 8.0.1; the name survives only as a typedef **inside** the deprecated `TopTools_ListOfShape.hxx`: `typedef NCollection_List<TopoDS_Shape>::Iterator TopTools_ListIteratorOfListOfShape;`. | Keep the deprecated include (works today), or bind `NCollection_List<TopoDS_Shape>::Iterator` and add the corresponding alias to the compat header. |
| 4 | P2 | 37 sites, e.g. `include/b_rep_fillet_api.hxx:43`, `include/b_rep_offset_api.hxx:37`, `include/shape_analysis.hxx:85/96/107/116/135`, `include/gp.hxx:26` | `GetMessageString()` is `Standard_DEPRECATED("Use what() instead")` (`Standard_Failure.hxx:59-60`). | Replace with `what()` (identical semantics on 8.x: `GetMessageString()` is `inline` returning `what()`). If the same headers must still build on 7.8, guard with `#if OCC_VERSION_MAJOR >= 8`. |
| 5 | P2 | `include/gc.hxx:1`, `:17` | `GCE2d_MakeSegment` is now `using GCE2d_MakeSegment = GC_MakeSegment2d` and is deprecated (`GCE2d_MakeSegment.hxx`, deprecated since 8.0.0). `GC_MakeSegment` and `GC_MakeArcOfCircle` are still real classes. | Prefer `GC_MakeSegment2d` / `<GC_MakeSegment2d.hxx>`; the `Value()` return type (`const occ::handle<Geom2d_TrimmedCurve>&`) is unchanged so the shim body stays as is. |
| 6 | P2 | `include/shape_analysis.hxx:24-27` | `ShapeAnalysis_FreeBounds::ConnectEdgesToWires(edges, toler, shared, wires)` is `Standard_DEPRECATED("Use ConnectEdgesToWires() returning handle by value instead")`; the new form is 3-arg and returns `occ::handle<NCollection_HSequence<TopoDS_Shape>>` (`ShapeAnalysis_FreeBounds.hxx:124-134`). | Switch to the by-value overload, or accept the warning (it still compiles). |
| 7 | P2 | `include/b_rep_fillet_api.hxx:36-40`, `include/chfi2d.hxx:7-9` | Comments assert "on OCCT 7.x does not derive from `std::exception`"; on 8.x `Standard_Failure : public std::exception` (`Standard_Failure.hxx:28`). | Comment-only fix. The catch *order* is already correct — `catch (const Standard_Failure&)` precedes `catch (const std::exception&)` at `b_rep_fillet_api.hxx:54-58`/`:77-81`, `chfi2d.hxx:40-45`, `b_rep_feat.hxx:31-38`, `shape_fix.hxx:36-38`, so OCCT failures are still classified as OCCT failures. Do not reorder them. |
| 8 | P2 | 19 × `Standard_Boolean`, 129 × `Standard_Integer`, 57 × `Standard_Real`, 2 × `Standard_True` in `include/*.hxx` | Deprecated aliases, `Standard_TypeDef.hxx:59-100`. `Standard_True`/`Standard_False` are now `inline constexpr bool` **variables**, not macros (`:60`, `:64`). | Warning-only. They are valid as values; they can no longer be used in `#if` or stringified as macros (grep confirms we never do). Cleanup, not a prerequisite. |
| 9 | P3 | `include/b_rep.hxx:52`, `:60` | `BRep_Tool::Triangulation` and `BRep_Tool::Curve(edge, L, first, last)` now return `const occ::handle<…>&` (`BRep_Tool.hxx:73-76`, `:101-104`) instead of by value; `Curve(edge, first, last)` still returns by value (`:109-111`). | Our shims copy the handle into a `new Handle_X(...)` immediately, so this is safe. Do not start holding the returned reference past the call (it may alias the `TFace`'s stored handle). |
| 10 | P3 | `src/top_exp.rs` (`TopExp_Explorer` bindings) | The `Next()` doc in 8.0.1 no longer states that exhausting the explorer raises `Standard_NoMoreObject` (`TopExp_Explorer.hxx`), while `Standard_NoMoreObject` itself still exists (`Standard_NoMoreObject.hxx:32`). | Nothing to change in C++; confirm with the topology tests that iteration still terminates the same way. If 8.x stopped throwing, code that relied on the throw would loop instead. |
| 11 | P3 | `include/b_rep_algo_api.hxx` usage of `BRepAlgoAPI_BuilderAlgo` | 8.0.1 deletes copy/move construction/assignment (`BRepAlgoAPI_BuilderAlgo(const BRepAlgoAPI_BuilderAlgo&) = delete;` etc.). | None today (we only hold `UniquePtr`); a warning for future code that tries to copy an algorithm object. |

Additive changes worth knowing, none of which affect us: `TopoDS_Shape` gained a generalized move constructor accepting its sub-classes (`TopoDS_Shape.hxx:52-62`, a `template <class T2> TopoDS_Shape(T2&&)` SFINAE'd on `is_base_of<TopoDS_Shape, T2>`), so `new TopoDS_Shape(expr)` in the shims keeps resolving; `BRepAdaptor_Curve` documents that `Value`/`D0-D3`/`DN` are inherited and `final` (`BRepAdaptor_Curve.hxx:93`).

---

## 2.1 Found by the first compile (not in the table above)

The table is a header and binding audit. The first real 8.0.1 build found three more
incompatibilities that only a compile surfaces; all are fixed in the tree as of
2026-11-01.

| # | Sev | Where | OCCT 8.0.1 evidence | Fix |
| --- | --- | --- | --- | --- |
| 1 | **P1** | `include/topo_ds.hxx`, `src/topo_ds.rs` | `TopoDS.hxx` declares `namespace TopoDS` (`:58`); 7.8.1 declared `class TopoDS` (`:50`). A cxx `#[Self]` binding needs a type. | seven free wrappers `topods_{vertex,edge,wire,face,shell,solid,compound}`; a Rust `TopoDS` module re-exports them so the call spelling is unchanged. |
| 2 | **P1** | `src/top_tools.rs` (`Size`) | `NCollection_BaseList::Size()` returns `size_t` (`NCollection_BaseList.hxx:105`); `Extent()` still returns `int` (`:99`). A cxx member binding requires the exact member-pointer type. | free wrapper `list_size()` returning `int`. `Extent()` stays a direct binding. |
| 3 | **P1** | `b_rep_algo_api.rs.cc`, `bop_algo.rs`, … | The `Deprecated/NCollectionAliases/` typedefs are no longer pulled in transitively: `TopTools_ListOfShape` was undeclared in a translation unit that only references it through another bridge's `type` alias. | define the 13 container names in the compat shim from `NCollection_*` (the definitions are copied from the deprecated headers, so the types are identical). |

The lesson for §4: an audit that resolves *names* and *types* cannot see a
class-to-namespace change, and it cannot see a return-type change on a member that
still exists under the same name. Only compiling catches those.

---

## 3. Direct answers

### (a) Are `TColgp_HArray1OfPnt.hxx` and `TopTools_HSequenceOfShape.hxx` installed by a default OCCT 8 build?

**Yes — by `install`, not by `build`.**

- They live in `src/Deprecated/NCollectionAliases/` and are installed by
  `install(DIRECTORY src/Deprecated/NCollectionAliases/ DESTINATION "${INSTALL_DIR_INCLUDE}")`
  (`CMakeLists.txt:1106-1108`). The trailing slash makes `install` copy the *contents*, so
  the headers land directly in `<prefix>/include/opencascade/`, next to the class headers.
- They are **not** part of the build-tree `inc/` directory: `COLLECT_AND_INSTALL_OCCT_HEADER_FILES`
  iterates `BUILD_TOOLKITS` → packages (`adm/cmake/occt_macros.cmake:353-412`), and
  `src/Deprecated` is not a toolkit. A consumer pointed at a build tree will not find them.
- This crate reads its include directory from `occ_info.txt`, which points at an OCCT
  *install* prefix, so the 12 deprecated includes resolve today. That dependency should be
  made explicit rather than accidental.
- They are header-level deprecated: `Standard_HEADER_DEPRECATED(...)` expands to
  `#pragma GCC warning` (via `Standard_Macro.hxx:50-53`), one warning per translation unit
  per header, plus a `Standard_DEPRECATED` warning on each use of the typedef.
- Either way, the `Handle_*` names for them no longer exist in C++ — that is a separate
  break, handled by the compat header (§2 row 1).

Modern replacements for the 12 deprecated headers (plus the iterator typedef, which has no
header of its own):

| Deprecated | Modern |
| --- | --- |
| `TColgp_Array1OfPnt` | `NCollection_Array1<gp_Pnt>` |
| `TColgp_Array2OfPnt` | `NCollection_Array2<gp_Pnt>` |
| `TColgp_Array1OfPnt2d` | `NCollection_Array1<gp_Pnt2d>` |
| `TColgp_Array1OfDir` | `NCollection_Array1<gp_Dir>` |
| `TColStd_Array1OfReal` | `NCollection_Array1<double>` |
| `TColStd_Array2OfReal` | `NCollection_Array2<double>` |
| `TColStd_Array1OfInteger` | `NCollection_Array1<int>` |
| `TColgp_HArray1OfPnt` | `NCollection_HArray1<gp_Pnt>` |
| `TopTools_HSequenceOfShape` | `NCollection_HSequence<TopoDS_Shape>` |
| `TopTools_ListOfShape` | `NCollection_List<TopoDS_Shape>` |
| `TopTools_ListIteratorOfListOfShape` | `NCollection_List<TopoDS_Shape>::Iterator` |
| `TopTools_IndexedMapOfShape` | `NCollection_IndexedMap<TopoDS_Shape, TopTools_ShapeMapHasher>` |
| `TopTools_IndexedDataMapOfShapeListOfShape` | `NCollection_IndexedDataMap<TopoDS_Shape, NCollection_List<TopoDS_Shape>, TopTools_ShapeMapHasher>` |

### (b) Do `Standard_Boolean/Integer/Real/True/False` still exist, unconditionally?

**Yes, and unconditionally.** `Standard_TypeDef.hxx` declares them with no `#ifdef` guard
around the definitions — only the `Standard_DEPRECATED(...)` attribute, which itself
becomes empty when `OCCT_NO_DEPRECATED` is defined. Lines observed:

- `:59-60` `inline constexpr bool Standard_False = false;`
- `:63-64` `inline constexpr bool Standard_True = true;`
- `:67-68` `typedef int Standard_Integer;`
- `:75-76` `typedef double Standard_Real;`
- `:79-80` `typedef bool Standard_Boolean;`

(`Standard_ShortReal`, `Standard_Character`, `Standard_Byte`, `Standard_Address`,
`Standard_Size`, `Standard_Time`, `Standard_ExtCharacter`, … follow the same pattern.)
The one real difference from 7.8 is that `Standard_True`/`Standard_False` are now
`inline constexpr bool` **objects**, not `#define`s — valid in any value context (our 2
uses are), invalid in preprocessor expressions.

### (c) Are the 24 linked toolkit names still CMake targets?

**Yes.** Each name is still a toolkit directory with its own `FILES.cmake`, which is what
produces the `TK*` target in OCCT's CMake:

| Toolkit | Directory (V8_0_1) | Toolkit | Directory |
| --- | --- | --- | --- |
| `TKMath` | `src/FoundationClasses/TKMath` | `TKPrim` | `src/ModelingAlgorithms/TKPrim` |
| `TKernel` | `src/FoundationClasses/TKernel` | `TKDESTEP` | `src/DataExchange/TKDESTEP` |
| `TKDE` | `src/DataExchange/TKDE` | `TKDEIGES` | `src/DataExchange/TKDEIGES` |
| `TKFeat` | `src/ModelingAlgorithms/TKFeat` | `TKDESTL` | `src/DataExchange/TKDESTL` |
| `TKGeomBase` | `src/ModelingData/TKGeomBase` | `TKMesh` | `src/ModelingAlgorithms/TKMesh` |
| `TKG2d` | `src/ModelingData/TKG2d` | `TKShHealing` | `src/ModelingAlgorithms/TKShHealing` |
| `TKG3d` | `src/ModelingData/TKG3d` | `TKFillet` | `src/ModelingAlgorithms/TKFillet` |
| `TKTopAlgo` | `src/ModelingAlgorithms/TKTopAlgo` | `TKBool` | `src/ModelingAlgorithms/TKBool` |
| `TKGeomAlgo` | `src/ModelingAlgorithms/TKGeomAlgo` | `TKBO` | `src/ModelingAlgorithms/TKBO` |
| `TKBRep` | `src/ModelingData/TKBRep` | `TKOffset` | `src/ModelingAlgorithms/TKOffset` |
| `TKXSBase` | `src/DataExchange/TKXSBase` | `TKCAF` | `src/ApplicationFramework/TKCAF` |
| `TKLCAF` | `src/ApplicationFramework/TKLCAF` | `TKXCAF` | `src/DataExchange/TKXCAF` |

No `OCCT_LIBS` change is needed (`build.rs:14-39`). Note that these are *directories*, so
`FILES.cmake` presence is the check; a `CMakeLists.txt` per toolkit is not how 7.8/8.0
lay the tree out.

### (d) Does `opencascade::handle<T>` keep the semantics our `.hxx` helpers rely on?

**Yes, unchanged.** From `Standard_Handle.hxx` (V8_0_1):

- `handle(const T*)` `:67`, copy `:74`, move `:81` (nulls the source), `~handle` releases.
- `Nullify()` `:91`, `IsNull()` `:94`, `reset(T*)` `:97`, `get()` `:122`,
  `operator->()` `:125`, `operator*`.
- `DownCast(const handle<T2>&)` `:198` and `DownCast(const T2*)` `:208`, SFINAE'd on
  `is_base_but_not_same`, plus a deprecated same-type overload. So
  `opencascade::handle<Geom_Line>::DownCast(curve)` in `include/geom.hxx:425` and
  `Handle(Geom_Curve)::DownCast(curve->Transformed(t))` at `:214` behave as before.
- `occ::handle<T>` is `template <class T> using handle = opencascade::handle<T>;`
  (`:415-445`), and `occ::down_cast` is new sugar.
- `Handle(Class)` still expands to `opencascade::handle<Class>` (`:448`), so the one place
  we use the old macro spelling compiles.
- `bindings_common.hxx:24-29` uses only `IsNull()` and `operator*`; both unchanged.

The one gap is not in `handle<T>` but in the names: `Handle_Geom_Curve` etc. are gone from
the class headers, which is the compat-header surface covered in
[`occt-8-upgrade.md`](occt-8-upgrade.md) §2.

---

## 4. Method and limits

### What was checked mechanically

1. **Include enumeration.** All `#include <…>` directives in the 61 bridge shims in
   `crates/opencascade-sys/include/` (62 files there once the compat header is counted),
   plus every `include!("…")` in `src/*.rs` (one per bridge; 60 of the 61 sources, `lib.rs`
   excepted). 225 distinct include targets: 218 OCCT `.hxx` headers, 2 local shims
   (`bindings_common.hxx`, `occt8_handle_compat.hxx`), 5 C++ standard headers. Every OCCT
   header was resolved against the `V8_0_1` git tree (36,501 entries, not truncated).
2. **Symbol-presence diff.** For each OCCT header we include, fetched the `V7_8_1` and
   `V8_0_1` versions and diffed the set of identifiers followed by `(` (i.e. declared
   functions/constructors), intersected with the crate's own source text. Any name we use
   that disappeared from the header is in the report. False positives from members that
   moved to a base class or to an alias target were resolved by hand (that is how
   `BRepAdaptor_*`, `BRepLProp_*`, `NCollection_*` were cleared).
3. **Member-function bindings.** Parsed all `fn Name(self: [Pin<&mut] T, …)` declarations in
   `src/*.rs` (156 distinct `(class, method)` pairs, 69 classes), then for each looked the
   method up in the 8.0.1 header of the class **and its transitive base classes**
   (`class X : public Y` closure), comparing presence, `const`-ness after the parameter
   list, and the return-type token sequence. Two constness changes, both benign (§1).
4. **Static bindings.** The 30 `#[Self = "…"]` functions checked the same way.
5. **Enum ABI.** Extracted the C++ body of each of the 14 enums that Rust mirrors with
   `#[repr(u32)]` and compared value order with the Rust declaration — exact match.
6. **Toolkits.** Directory and `FILES.cmake` presence for all 24 `OCCT_LIBS` entries.
7. **Handle surface.** Cross-checked the 35 `type Handle_*;` declarations against the 34
   `using` aliases in `occt8_handle_compat.hxx`; `Standard_Type` is the 35th and is
   deliberately excluded (OCCT still defines it at `Standard_Type.hxx:199`).

### What was sampled, and why

- **Constructors (144 `#[cxx_name = "construct_unique"]` bindings).** Arity was compared
  mechanically, but OCCT reflowed most constructor declarations across lines in 8.0, which
  makes an automated parameter-count comparison unreliable. The multi-argument primitives
  were therefore read by hand: `BRepPrimAPI_Make{Box,Cone,Cylinder,Sphere,Torus,Revol,Prism}`,
  `BRepMesh_IncrementalMesh`, `BRepFeat_MakeDPrism`, `Poly_Triangulation`,
  `BRepAdaptor_{Curve,Surface,CompCurve}`, `BRepBuilderAPI_{MakeFace,MakeEdge,Sewing,GTransform,Copy,MakeShapeOnMesh}`,
  `BRepOffsetAPI_{MakePipe,MakePipeShell}`, `Geom_{BSplineCurve,BezierCurve,TrimmedCurve,OffsetCurve}`.
  No constructor we call lost a parameter.
- **Shim bodies.** All 62 headers in `include/` were read (the 61 bridge shims plus
  `occt8_handle_compat.hxx`, ≈4.6 kLOC), but not every overload of every call was traced;
  overload resolution was reasoned about, not compiled.
- **OCCT headers.** For each header we include, the 7.8.1→8.0.1 diff was used rather than a
  full read; headers referenced only transitively (base classes, `Adaptor3d_*`,
  `BRepTools_History`, `NCollection_*` internals) were opened only when a diff or a
  binding pointed at them.

### What only a real compile could settle — and did (2026-11-01)

A build against a conda `occt=8.0.1` prefix has now been run. It settled the items that
were listed here:

- overload ambiguity and template instantiation across the 62 bridge headers — **compiled**;
- alias collisions — **none**: the shim's `Handle_*` and container typedefs coexist with
  OCCT's own deprecated typedefs as legal redeclarations;
- the `Handle_TopTools_HSequenceOfShape` bug — **confirmed and fixed** (§2 row 1);
- deprecation volume — **confirmed** as warnings only; nothing builds with `-Werror`.

It also found three incompatibilities this audit missed (§2.1).

What a Linux compile still does not settle: MSVC and macOS behaviour, the `builtin`
recipe, and algorithm output beyond `ferrocad_occt`'s 78 tests.

- **Behavioural changes.** Whether `TopExp_Explorer::Next` still raises at exhaustion,
  whether boolean/fillet/offset results are geometrically identical, and whether the
  handle-returning `BRep_Tool` accessors alias internal storage in a way our immediate
  copies hide: these need the topology conformance suite on 8.0.1, not a header diff.
