# Property value extension: letting a module hold its own data

Status: implemented (2026-10-06), companion to
[`property-types.md`](property-types.md) and [`occt-integration.md`](occt-integration.md) §5.

`ferrocad_core::property_types` lets a module register a property **name**, but the value
is still one of core's `Property` enum variants. A Part shape cannot be stored that way.
This note settles how a module holds its own value.

## The constraint

`Property` is a closed enum with `Debug, Clone, PartialEq, Serialize, Deserialize`, and
`SavedObject.properties` is a `BTreeMap<String, Property>` — so the enum is *directly*
serde-serialised. Any variant holding `Box<dyn Trait>` breaks all five derives at once,
and the persistence trait must not force core to serialise a kernel handle.

## The design: one boxing variant with a hand-written newtype

Add exactly one variant, `Property::Extension(ExtensionValue)`, where `ExtensionValue`
is a newtype over `Box<dyn ExtensionData>` with **manual** `Clone`/`Debug`/`PartialEq`/
`Serialize`/`Deserialize`. That keeps `Property`'s own derives intact — serde only needs
the field type to implement the traits, which `ExtensionValue` does.

```rust
pub trait ExtensionData: Send {
    fn type_name(&self) -> &str;              // "Part::PropertyPartShape"
    fn clone_box(&self) -> Box<dyn ExtensionData>;
    fn as_any(&self) -> &dyn std::any::Any;   // for equality
    fn eq(&self, other: &dyn ExtensionData) -> bool;
    fn save(&self) -> Vec<u8>;                // module-owned (BREP bytes)
    fn debug(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}
```

The bound is `Send` and stops there. Documents are shared behind `Arc<Mutex<…>>`, which
is `Sync` as long as its contents are `Send`, so a document holding a module value stays
shareable without the value itself being `Sync` — a kernel handle that is only `Send`
(OCCT's `TopoDS_Shape`) is stored as-is. (`Shape` shares its payload the same way: it
lets the kernel clone its own handle rather than putting the payload behind an `Arc`,
which would demand `Sync`.) The one remaining place that could force `Sync` is PyO3,
whose `#[pyclass]` types must be `Send + Sync`; the binding therefore never holds a whole
`Property` in a pyclass (the geometry write-through view stores the `Placement` it came
from, not the enclosing `Property`), so a `!Sync` extension never reaches PyO3.

- **Clone** → `clone_box`. **PartialEq** → compare `type_name`, then trait `eq`.
- **Serialize** → `{ "type": <name>, "bytes": <save()> }`.
- **Deserialize** → read `{ type, bytes }` and ask the registry to reconstruct:
  `property_types::restore_extension(type, bytes)`. An unregistered type is a load error
  (the module is not loaded), which is the faithful behaviour.

## Registration

`property_types` gains a paired registration, since a factory alone cannot deserialise:

```rust
pub fn register_extension(
    name: &str,
    make_default: impl Fn() -> Property + Send + Sync + 'static, // returns Property::Extension
    restore: fn(&[u8]) -> Option<Box<dyn ExtensionData>>,
);
```

`addProperty("Part::PropertyPartShape", …)` then resolves through the existing
`default_for`, and a `.FCStd` round-trips through `restore`.

## Consequences at the edges

- `Property::type_name` must become `&str` (dynamic for extensions), not `&'static str`.
- Python conversion (`py_to_property`/`property_to_py`) has no generic answer for an
  extension value; it raises until the owning module registers a conversion (Part will,
  for `obj.Shape`). This is the same split as upstream: the value is opaque to core.
- `dump_property`/`restore_property` (the bytes API) delegate to `save`/`restore`, so they
  work unchanged.

## What stays out

- We do **not** convert `PropertyContainer` to `Box<dyn Property>`. That is the fully
  faithful FreeCAD model and would touch every `match` on `Property`; the single boxing
  variant gets the same extensibility for a fraction of the churn.
- We do **not** attempt generic Python conversion of extension values.
