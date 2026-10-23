# `FreeCAD`

Upstream-compatible surface: documents, objects, the base value types.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

A drop-in, Rust-backed stand-in for FreeCAD's Python ``App`` module.

## Functions

| Function | Summary |
| --- | --- |
| `ParamGet(path='', create=True)` | Return the parameter group rooted at ``path``. |
| `Version()` | — |
| `activeDocument()` | — |
| `closeDocument(name)` | — |
| `getDocument(name)` | — |
| `listDocuments()` | — |
| `newDocument(name=None, hidden=False, temp=False)` | — |
| `openDocument(path, hidden=False, temporary=False)` | — |
| `writeRecoverySnapshotToTransientDir(doc, compressed=True)` | Write a recovery snapshot (metadata + document) to ``doc.TransientDir``. |

## Class `Document`

| Member | Summary |
| --- | --- |
| `ActiveObject` (attribute) | — |
| `Comment` (attribute) | — |
| `CountObjects` (attribute) | — |
| `FileName` (attribute) | — |
| `Label` (attribute) | — |
| `MemSize` (attribute) | — |
| `Meta` (attribute) | — |
| `Name` (attribute) | — |
| `Objects` (attribute) | — |
| `RedoCount` (attribute) | — |
| `RedoNames` (attribute) | — |
| `RootObjects` (attribute) | — |
| `TopologicalSortedObjects` (attribute) | — |
| `TransientDir` (attribute) | — |
| `UndoCount` (attribute) | — |
| `UndoMode` (attribute) | — |
| `UndoNames` (attribute) | — |
| `UndoRedoMemSize` (attribute) | — |
| abortTransaction() | — |
| addObject(type, name=None, objProxy=None, viewProxy=None, attach=False, viewType=None) | — |
| canWriteRecoverySnapshot() | Recovery snapshots are only writable outside a transaction. |
| clearUndos() | Drop the whole undo/redo history (FreeCAD `clearUndos`). |
| commitTransaction() | — |
| copyObject(object, recursive=False, return_all=False) | Copy one object (or a sequence of objects) from another document. |
| dumpContent(Compression=0) | — |
| findObjects(Type='', Name='', Label='') | — |
| getAvailableRedos(id=0) | The number of redo steps, or the depth of transaction `id` (FreeCAD `getAvailableRedos`). |
| getAvailableUndos(id=0) | The number of undo steps, or the depth of transaction `id` (FreeCAD `getAvailableUndos`). |
| getBookedTransactionID() | — |
| getObject(key) | — |
| getTempFileName(basename) | A file name inside the transient directory (`getTempFileName`). |
| isAutoCreated() | — |
| load(path) | — |
| openTransaction(name='') | — |
| recompute() | — |
| redo() | — |
| removeObject(name) | — |
| restore() | Re-load the document from its saved file (clearing current content). |
| restoreContent(data) | Replace this document's content from a `dumpContent` payload. |
| save() | — |
| saveAs(path) | Save under `path`, selecting the serializer from the suffix. |
| setAutoCreated(v) | — |
| settings(namespace) | Return a namespaced view over `Meta` (keys `"<namespace>.<key>"`). |
| undo() | — |

## Class `DocumentObject`

| Member | Summary |
| --- | --- |
| `Document` (attribute) | — |
| `ExpressionEngine` (attribute) | — |
| `ID` (attribute) | — |
| `InList` (attribute) | — |
| `Label` (attribute) | — |
| `MustExecute` (attribute) | — |
| `Name` (attribute) | — |
| `OutList` (attribute) | — |
| `PropertiesList` (attribute) | — |
| `State` (attribute) | — |
| `TypeId` (attribute) | — |
| `ViewObject` (attribute) | — |
| addExtension(name) | — |
| addObject(other) | — |
| addProperty(type_id, name, group='', doc='', attr=0, read_only=False, hidden=False, locked=False, enum_vals=None) | — |
| dumpContent() | Serialize this object's content to bytes (`dumpContent`). |
| dumpPropertyContent(name, Compression=0) | Serialize one property to bytes (`dumpPropertyContent`). |
| enforceRecompute() | — |
| evalExpression(source) | Evaluate an expression source in this object's context. |
| getDocumentationOfProperty(name) | `getDocumentationOfProperty`: the property's documentation string. |
| getElementIndex(name) | `getElementIndex(name)`: the index in an element reference. |
| getEnumerationsOfProperty(name) | `getEnumerationsOfProperty`: an enumeration's allowed values, else `None`. |
| getGroupOfProperty(name) | `getGroupOfProperty`: the UI group the property belongs to. |
| getLinkedObject(resolve=True) | `getLinkedObject(resolve=True)`: the object a link points at. |
| getObject(name) | — |
| getParent() | `getParent()`: the geo-feature group (`App::Part`) this object belongs to. |
| getParentGeoFeatureGroup() | The geo-feature group (`App::Part`) that lists this object. |
| getParentGroup() | The plain group (or group-extension object) that lists this object. |
| getPropertyByName(name) | — |
| getPropertyStatus(name='') | `getPropertyStatus(name="")`: with no name, the supported status names. |
| getStatusString() | A short status string: `Invalid`, `Touched` or `Valid`. |
| getSubObject(subname, retType=0, matrix=None, transform=True, depth=0) | Resolve sub-object(s) and return them per FreeCAD's `retType` convention: 0 and 1 → DocumentObject, 2 → `(obj, Matrix, None)`, 3 → Placement, 4 → Matrix. |
| getTypeIdOfProperty(name) | — |
| getTypeOfProperty(name) | `getTypeOfProperty`: the property's status flags as text names. |
| group_members() | Resolve this object's `Group` link list into `DocumentObject`s. |
| hasExtension(name) | — |
| hasObject(other) | — |
| isLink() | `isLink()`: true for `App::Link` and its registered containers, and for the `Part::LinkArray*` types. |
| purgeTouched() | Mark the object as unchanged (FreeCAD `purgeTouched`). |
| recompute() | Recompute this object (FreeCAD `DocumentObject.recompute`). |
| removeExtension(name) | — |
| removeProperty(name) | — |
| restoreContent(data) | Restore this object's content from bytes (`restoreContent`). |
| restorePropertyContent(name, data) | Restore one property from bytes (`restorePropertyContent`). |
| setEditorMode(prop, _modes) | Record a property editor mode (`ReadOnly`, …); POC: fires the event only. |
| setExpression(prop, source=None) | — |
| setLink(value, subname=None) | `App::Link.setLink(obj)`: point `LinkedObject` at `obj` (or clear it with `None`). `subname` narrows the link to one sub-element of the target. |
| setPropertyByName(name, value) | — |
| setPropertyStatus(name, val) | `setPropertyStatus(name, val)`: add (or, with a leading `-`, clear) flags. |
| set_group(value) | Assign the `Group` link list from a sequence of objects (or names). |
| supportedProperties() | — |
| touch(prop=None) | Mark the object for recompute (FreeCAD `DocumentObject.touch`). |

## Class `ParameterGrp`

An in-memory stand-in for ``Base.ParameterGrp`` (typed, nested groups).

| Member | Summary |
| --- | --- |
| Clear() | — |
| Export(path) | — |
| GetBool(name, default=False) | — |
| GetBools() | Names with a bool value in this group (FreeCAD returns names). |
| GetFloat(name, default=0.0) | — |
| GetFloats() | Names with a float value in this group (FreeCAD returns names). |
| GetGroup(name, _store=None, _group=None) | — |
| GetGroupName() | — |
| GetGroups() | The immediate sub-group names (FreeCAD `GetGroups`). |
| GetInt(name, default=0) | — |
| GetInts() | Names with an int value in this group (FreeCAD returns names). |
| GetString(name, default='') | — |
| GetStrings() | Names with a string value in this group (FreeCAD returns names). |
| GetUnsigned(name, default=0) | — |
| GetUnsigneds() | Names with an unsigned value in this group (FreeCAD returns names). |
| HasGroup(name) | — |
| Import(path) | — |
| RemBool(name) | — |
| RemFloat(name) | — |
| RemGroup(name) | Remove a child group, unless a live handle still references it. |
| RemInt(name) | — |
| RemString(name) | — |
| RemUnsigned(name) | — |
| SetBool(name, value) | — |
| SetFloat(name, value) | — |
| SetInt(name, value) | — |
| SetString(name, value) | — |
| SetUnsigned(name, value) | — |

## Class `ScaleType`

Scaling mode returned by ``Matrix.hasScale()`` (``Base::ScaleType``).

| Member | Summary |
| --- | --- |
| `NoScaling` (attribute) | — |
| `NonUniformLeft` (attribute) | — |
| `NonUniformRight` (attribute) | — |
| `Other` (attribute) | — |
| `Uniform` (attribute) | — |
| as_integer_ratio() | Return a pair of integers, whose ratio is equal to the original int. |
| bit_count() | Number of ones in the binary representation of the absolute value of self. |
| bit_length() | Number of bits necessary to represent self in binary. |
| conjugate() | Returns self, the complex conjugate of any int. |
| `denominator` (attribute) | — |
| from_bytes(bytes, byteorder='big', *, signed=False) | Return the integer represented by the given array of bytes. |
| `imag` (attribute) | — |
| is_integer() | Returns True. Exists for duck type compatibility with float.is_integer. |
| `numerator` (attribute) | — |
| `real` (attribute) | — |
| to_bytes(length=1, byteorder='big', *, signed=False) | Return an array of bytes representing an integer. |

## Values

| Name | Summary / value |
| --- | --- |
| `ActiveDocument` | None |
| `backend` | 'ferrocad' |
