# Property types: the C++ registry and the Python contract

Status: evaluation (2026-10-06), companion to
[`python-bindings.md`](python-bindings.md) and [`occt-integration.md`](occt-integration.md) §5.

Does FreeCAD expose how C++ holds property types, or is the property system pure
Python? It is **C++ with a reflective runtime type registry**, and Python addresses it
**by name**. That answers what our document-object SPI has to look like.

## 1. Upstream: C++ classes, addressed by string name

- `App::Property` is a C++ base class. Concrete types (`App::PropertyLength`,
  `App::PropertyFloat`, `Part::PropertyPartShape`, …) derive from it and register in
  FreeCAD's runtime type system, `Base::Type`.
- Python **names** a property type as a string:
  `obj.addProperty("App::PropertyLength", "Length", "Base", "tooltip")` →
  `DocumentObjectPy::addProperty` → `addDynamicProperty(sType, …)` →
  `Base::Type::fromName(sType)->createInstance()`.
- Python can **enumerate** the registered types: `obj.supportedProperties()` →
  `Base::Type::getAllDerivedFrom(App::Property::getClassTypeId())`.
- Introspection returns the registered name: `getTypeIdOfProperty(name)`. (Note the
  naming trap: `getTypeOfProperty(name)` returns *status flags* like `ReadOnly`, not the
  type name.)
- Python cannot define new property types. The Python-*holding* pieces are
  `App::PropertyPythonObject` and the `FeaturePython`/`Proxy` scripted-object mechanism
  (`__dict__` persistence) — which the Rust core already mirrors via `python_state`.

So it is not a pure-Python implementation: Python names and enumerates C++ property
types; the implementation and the registry are C++.

## 2. The compatibility contract is the name

Scripts do not care whether a property type is implemented in C++ or Rust. They care that
`"App::PropertyLength"` — and, once Part is loaded, `"Part::PropertyPartShape"` — resolves
to a value with the right semantics and persists. The contract is the **type-name string**,
so the SPI must be a **name-keyed registry**, mirroring `Base::Type`.

## 3. Where we are now

- `ferrocad_core::Property` is a closed enum, but each variant reports `type_name()`, so
  `getTypeIdOfProperty` already returns the right string for core types.
- Creation is a **closed mapping**: `default_property(type_id)` suffix-matches names onto
  enum variants, and `is_property_type()` accepts any string that contains `Property`.
- **Hazard.** An unknown-but-property-like name falls through to `Property::String`. So
  today `addProperty("Part::PropertyPartShape", "Shape")` would silently create an empty
  **String** property — the wrong type — instead of the shape property or an error.
  Upstream raises for an unregistered type; `supportedProperties()` is likewise a
  hardcoded 9-item list that would omit module types.

## 4. What this implies for the SPI

A registry `name -> property descriptor` (a factory plus `save`/`restore`), populated by
core with the `App::*` types and by modules with theirs. Then:

- `addProperty` resolves through the registry and **raises on an unknown name**;
- `supportedProperties()` enumerates it, so a module's types appear once it is loaded;
- a Part shape property registers under exactly `"Part::PropertyPartShape"`, with its
  `save`/`restore` (BREP bytes) living in Part — core never names it.

This is the same seam that [`occt-integration.md`](occt-integration.md) §5 identifies for
Part; Python compatibility is what makes it mandatory rather than nice-to-have.

## 5. Cost of getting it wrong

Scripts that `addProperty` a module type, or that read `supportedProperties()` /
`getTypeIdOfProperty`, would silently mis-type or miss types. The name-driven dispatch must
be in place before any module (Part) ships.

## 6. Status (2026-10-06): implemented

`ferrocad_core::property_types` is now the registry. Core seeds the `App::*` set;
`register(name, factory)` is the module SPI. `addProperty` resolves through it and
**raises** on an unknown name (the silent-`String` fallback is gone), and
`supportedProperties()` enumerates it.

The companion object-type seam, `ferrocad_core::object_registry`, and the property *value*
extension (`ferrocad_core::extension`; `Property::Extension`) are also in. A module can now
register a name, an object type and a value that holds its own data with `save`/`restore`
— which is everything `Part::PropertyPartShape` needs; see
[`property-value-extension.md`](property-value-extension.md).
