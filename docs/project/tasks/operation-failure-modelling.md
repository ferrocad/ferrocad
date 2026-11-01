# Operation failure: a typed, recoverable failure model for Part features

Status: task spec (2026-11-01). Tracks
[ferrocad/ferrocad#2](https://github.com/ferrocad/ferrocad/issues/2). Companion to
[`topology-selection.md`](../../developer/interaction/topology-selection.md) (the F0 seam)
and [`visual-language.md`](../../developer/interaction/visual-language.md) §5 (the state
model) and §7 (one token source).

The premise is the project's standing bar: **with good UX the user should not have to be
careful.** A fillet that fails today is reported as a sentence in a status bar, if at
all. This task turns that sentence into a value the model can act on, so the UI can
steer the user to a valid action instead of asking them to parse prose.

---

## 0. What the tree does today (verified 2026-11-01)

The failure information is erased three times on its way to the user:

| Layer | Type | Where |
| --- | --- | --- |
| kernel | `OcctError { WrongBackend, NotDone { operation }, Exception { operation, message }, InvalidInput { what } }` | `crates/ferrocad_occt/src/error.rs` |
| seam | `GeomError = Box<dyn Error + Send + Sync + 'static>` | `crates/ferrocad_geom/src/lib.rs` |
| workbench | `String` | `crates/ferrocad_part/src/dress_up.rs` (`map_err(|e| e.to_string())`) |
| document | `invalid: bool` + `error: Option<String>` | `crates/ferrocad_core/src/document/mod.rs` |

Facts that constrain the design:

- **`ferrocad_part` cannot name `OcctError`.** Its only kernel dependency is
  `ferrocad_geom`, and the trait is object-safe, so `GeomError` is deliberately boxed.
  Any typed model has to cross that seam as data, not be downcast in the workbench.
- **`ferrocad_core` does not depend on `ferrocad_geom`** (and `ferrocad_geom` depends on
  `ferrocad_curves`, `ferrocad_lineage`, `ferrocad_types`). `DocumentObject` is where a
  failure must live, so the type cannot simply be declared in `ferrocad_geom` and stored
  in core without deciding where it belongs (§3).
- **The message already exists.** `Document::mark_invalid_with` stores it and
  `PyDocumentObject::getStatusString` renders `Invalid: <reason>`. Only the *class* of
  failure is missing, not the text.
- **`dress_up::store` keeps the last good shape** as of 2026-11-01 and records the
  reason; `containers::store` still blanks the output. That asymmetry is deliberate for
  now (§4).

So the current state is: the user is told *that* it failed and (for fillets, chamfers and
a few patterns) *why*, in words. Nothing in the model knows whether the user asked for
something impossible — which is recoverable — or the kernel is buggy — which is
reportable and must never be presented as the user's fault.

---

## 1. The two failure classes

| | `InvalidGeometry` | `Kernel` |
| --- | --- | --- |
| means | the user asked for something invalid | a bug in FerroCAD or OCCT |
| recoverable | yes, by steering to a valid input | not by the user |
| glyph | `!` in `SUBTRACT_COLOR` | same glyph |
| affordance | a typed, clickable recovery | a "report" action carrying the record |
| must **not** offer | "report" | "fix your input" |

FreeCAD conflates these: a kernel refusal and a bad radius both surface as a shape that
disappears. They are different products. A domain error presented as a bug teaches users
to distrust the tool; a kernel bug presented as user error hides real regressions.

---

## 2. The types

```rust
pub enum OperationFailure {
    /// Domain error: the user asked for something invalid. Recoverable.
    InvalidGeometry { reason: InvalidReason, recover: Recovery },
    /// Kernel anomaly: a bug in the application or OCCT. Reportable.
    Kernel(StandardFailureRecord),
}

pub enum InvalidReason {
    FilletOnFilletEdge,
    RadiusExceedsLocalCurvature,
    SelfIntersectingResult,
    EmptySelection,
    NonPositiveValue,      // radius / distance <= 0 or non-finite
    EdgeNotOnShape,
    // ...
}

pub enum Recovery {
    /// Steer the user to the existing feature that owns the offending edge.
    AdjustExistingFillet { fillet: FilletRef },
    /// Fix implicitly by clamping the field to the largest value that works.
    ReduceRadius { max: f64 },
    /// No safe automatic repair: hold the session and explain.
    RollBackOnly,
}
```

`FilletRef` is deliberately not `ObjectId` (§3.3): the recovery is produced by a layer
that may not know the document's id type.

`StandardFailureRecord` is a plain-data snapshot, not `OcctError`:

```rust
pub struct StandardFailureRecord {
    pub operation: &'static str,   // "fillet", "chamfer", "cut", ...
    pub message: String,           // OCCT's own text
    pub kernel: &'static str,      // "OCCT 7.8.1"
    // plus whatever the report path needs (input fingerprint, doc name)
}
```

It must be constructible by any kernel, so it carries no OCCT types. The OCCT backend
fills `message` from `Standard_Failure::GetMessageString()` (7.x) / `what()` (8.x).

---

## 3. Where the types live

### 3.1 The seam is the classification boundary

`GeometryBackend` returns one boxed error. Three ways to get a class out of it:

| Option | Shape | Cost | Verdict |
| --- | --- | --- | --- |
| **A. Typed `GeomError`** | `GeomError` becomes a struct/enum carrying `FailureKind` | every `.into()` in every backend and test changes; `UnsupportedOperation` and `Box<dyn Error>` users must migrate | invasive, but the most honest type |
| **B. Classify on the trait** | `fn classify(&self, error: &GeomError) -> FailureKind`, defaulting to `Kernel` | additive: no existing signature changes, a kernel that cannot classify falls back to the *reportable* default | **recommended first step** |
| **C. Downcast in the workbench** | `(error as &dyn Error).downcast_ref::<OcctError>()` | impossible: `ferrocad_part` cannot depend on a specific backend | rejected |

Option B is also the kernel-swap-friendly shape: `OcctError::InvalidInput` →
`FailureKind::Invalid`, `Exception`/`NotDone` → `FailureKind::Kernel`, and a future kernel
writes its own two-line mapping. The default (`Kernel`) is safe because it never blames
the user.

### 3.2 `FerroCAD` domain knowledge stays in `ferrocad_geom`

`InvalidReason` names fillet/chamfer facts, and the trait already owns `fillet`,
`chamfer`, `cut`, … So `InvalidReason` and `FailureKind` belong beside the trait.
`OperationFailure` and `Recovery` reference a document object, so they belong in
`ferrocad_core` — unless core gains a dependency on `ferrocad_geom` or the shared types
move to a small crate. **This is the first decision to take** (§6).

### 3.3 The recovery's object reference

`Recovery::AdjustExistingFillet` has to name the feature that already owns the edge. The
producer (a backend), the holder (core) and the consumer (UI) are three layers. Two
shapes work:

- a plain `u64` id plus a documented "document object id" meaning, or
- a name (`String`) that the UI resolves through `Document::get_by_name`.

A name survives serialization and needs no id plumbing; an id survives a rename. Pick one
in §6 before implementing `F3`.

---

## 4. Where the failure surfaces

1. **`DocumentObject`** gains the typed failure. Keep `invalid` and `error` as the
   FreeCAD-compatible projection (`getStatusString`), derived from it, so nothing in the
   Python surface changes on day one.
2. **`mark_failed(id, OperationFailure)`** joins `mark_invalid` / `mark_invalid_with`.
   The workbench calls it; `mark_invalid_with` stays for callers that only have prose.
3. **Stale geometry is the failsafe.** A failed rebuild keeps the last good shape (done
   for `dress_up`). The document already treats a persisted `Shape` as a cache: `from_saved`
   marks registered objects `must_execute`, so the failure is re-derived at open and the
   cached body stays visible instead of blanking at load. Extend the same rule to
   `containers::store` as part of `F5`.
4. **Session state.** The visual-language §5 `SESSION_HOLD` gains a payload:
   `SESSION_HOLD(failure: OperationFailure)`. The status-bar segment, the radial-menu arc
   and the 3-D edge badge all read that one value (the §7 single-source rule).

### 4.1 UI contract

- `InvalidGeometry` → `!` triangle in `SUBTRACT_COLOR`; the detail disclosure shows
  `reason` and the **typed** recovery as a clickable affordance:
  - `AdjustExistingFillet { fillet }` selects that feature and opens its radius field;
  - `ReduceRadius { max }` clamps the field's drag range to `max` and offers to apply it;
  - `RollBackOnly` offers rollback only.
- `Kernel(record)` → the same glyph, a different payload, and a "report" action.
- **Never offer "report" for `InvalidGeometry`.** This belongs in
  `visual-language.md` §10 (currently *Non-goals*) as an explicit non-goal, because it is
  a product rule, not a rendering detail.
- The UI must never parse `error` to decide which class it has. The enum is the truth;
  the glyph is presentation.

---

## 5. Stages and acceptance

| Stage | Work | Acceptance |
| --- | --- | --- |
| **F1** ✅ | `FailureKind` + `GeometryBackend::classify`; OCCT maps `InvalidInput` → domain and `Exception`/`NotDone` → kernel | A unit test per mapping; no signature changes; existing suites green |
| **F2** | `OperationFailure`, `mark_failed`, `DocumentObject.failure`; `getStatusString` unchanged | a failed fillet stores `InvalidGeometry`; a throwing `Build` stores `Kernel`; both still render `Invalid: …` |
| **F3** | Fillet/chamfer reasons and recoveries: `NonPositiveValue`, `EmptySelection`, `EdgeNotOnShape`, `RadiusExceedsLocalCurvature`; `max_fillet_radius` for `ReduceRadius` | each bad input produces its reason and, where possible, a working `max`; `AdjustExistingFillet` is produced when the edge is already filleted |
| **F4** | UI: `SESSION_HOLD(failure)`, `!` glyph, clickable recovery, report path for `Kernel` only | a test asserts a domain failure has no report affordance; a kernel failure does |
| **F5** | `containers::store` parity (boolean/compound keep the last good shape and a typed failure) | a failed `Cut` keeps the previous body and reports `InvalidGeometry` |

`F1` and `F2` are cheap and unblock everything. `F3` is where the real domain work is:
`RadiusExceedsLocalCurvature` and `AdjustExistingFillet` need kernel queries, not string
matching.

---

## 5.1 What landed (F1, 2026-11-01)

- `ferrocad_geom`: `InvalidReason { NonPositiveValue, EmptySelection, NullShape,
  EdgeNotOnShape, UnresolvableElement, Other(String) }`, `FailureKind { Invalid {
  reason, message }, Kernel { operation, message } }`, and a defaulted
  `GeometryBackend::classify` whose default is `Kernel` — the safe direction: an
  unclassifiable error is never blamed on the user.
- `ferrocad_occt`: `OcctBackend::classify` delegates to one auditable `classify` in
  `error.rs`. `InvalidInput` is the only domain case; `NotDone`, `Exception`,
  `WrongBackend` and foreign errors are `Kernel`.
- The `what` → `InvalidReason` mapping is guard-based over the closed set of literals the
  backend itself writes, with `Other` as the fallback so nothing is misfiled.
- Tests: `ferrocad_geom` has the default-classification test and the `Display` test;
  `ferrocad_occt` has one test per `OcctError` variant plus three end-to-end
  classifications. `cargo test -p ferrocad_occt --lib` → **78 passed**, up from 69.
- One correction to this document's premise: a **foreign edge no longer reaches
  `Build()`** (it is pre-rejected), so it classifies as `Invalid { EdgeNotOnShape }`.
  The kernel path is covered by a radius the kernel refuses (`NotDone`). Both are
  tested.

---

## 6. Open decisions

1. **Where `OperationFailure` lives.** `ferrocad_core` (needs `ferrocad_geom` or a new
   `ferrocad_failure` module in `ferrocad_types`) vs `ferrocad_geom` (needs core to
   depend on it). Affects the first commit of `F2`.
2. **`FilletRef` shape** (§3.3): id or name.
3. **Is the kernel record persisted?** Today `error` is explicitly never persisted. A bug
   report needs the record at the moment of failure; keeping it out of `.fcstd` is right,
   but the report path must be able to capture it before the next recompute.
4. **Does a kernel failure keep stale geometry too?** Current answer: yes for dress-up,
   because blanking is the worse outcome. Revisit if a wrong-but-visible body is judged
   more misleading than a missing one.

---

## 7. Non-goals

- The sketch solver's solved/failed/over-constrained state. It needs the same *channel*
  and is the second consumer of `F2`, but the reasons are different.
- OCCT 8's `Standard_Failure : std::exception`. It changes the catch, not the model; see
  [`../research/occt-8-upgrade.md`](../research/occt-8-upgrade.md).
- Retrying or auto-repairing geometry without user consent. A recovery is a proposal.
