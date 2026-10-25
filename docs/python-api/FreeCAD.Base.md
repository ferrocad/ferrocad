# `FreeCAD.Base`

The `Base` value types FreeCAD exposes as a submodule.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

``FreeCAD.Base`` — the core data types.

## Class `BoundBox`

| Member | Summary |
| --- | --- |
| `Center` (attribute) | — |
| `XLength` (attribute) | — |
| `XMax` (attribute) | — |
| `XMin` (attribute) | — |
| `YLength` (attribute) | — |
| `YMax` (attribute) | — |
| `YMin` (attribute) | — |
| `ZLength` (attribute) | — |
| `ZMax` (attribute) | — |
| `ZMin` (attribute) | — |
| add(x, y, z) | — |
| enlarge(amount) | Grow the box by `amount` in every direction (`BoundBox.enlarge`). |
| getIntersectionPoint(point, direction) | The first box-surface point hit by a ray from `point` along `direction`. |
| intersect(other) | — |
| intersected(other) | — |
| isInside(value) | — |
| isValid() | — |
| setVoid() | — |

## Class `CADKernelError`

Kernel failure (``Part.OCCError`` when the Part module is absent).

## Class `Material`

| Member | Summary |
| --- | --- |
| `DiffuseColor` (attribute) | — |

## Class `Matrix`

| Member | Summary |
| --- | --- |
| `A` (attribute) | — |
| col(col) | — |
| decompose() | `(shear, scale, rotation, move)` such that `self == move * rotation * scale * shear`. |
| determinant() | — |
| diagonal() | — |
| hasScale(tol=0.0) | Classify scaling; returns a `FreeCAD.ScaleType` member like upstream. |
| inverse() | — |
| isNull() | — |
| isUnity(tol=0.0) | — |
| move(*args) | — |
| multVec(v) | Transform a vector by this matrix (FreeCAD `Matrix.multVec`). |
| multiply(o) | — |
| nullify() | — |
| rotateX(angle) | — |
| rotateY(angle) | — |
| rotateZ(angle) | — |
| row(row) | — |
| scale(*args) | — |
| setCol(col, v) | — |
| setRow(row, v) | — |
| transpose() | — |
| unity() | — |

## Class `Placement`

| Member | Summary |
| --- | --- |
| `Base` (attribute) | — |
| `Matrix` (attribute) | — |
| `Rotation` (attribute) | — |
| copy() | A detached copy (`Placement.copy`). |
| inverse() | — |
| isSame(o, tol=0.0) | — |
| multVec(v) | Transform a vector by this placement (FreeCAD `Placement.multVec`). |
| rotate(center, axis, angle, comp=False) | Rotate this placement in place (`Placement.rotate`). |
| toMatrix() | — |
| translate(*args) | Move this placement in place (`Placement.translate`). |

## Class `Precision`

``Base::Precision`` — the global tolerance defaults OCCT uses.

| Member | Summary |
| --- | --- |
| angular() -> float | — |
| approximation() -> float | — |
| confusion() -> float | — |
| firstParameter() -> float | — |
| intersection() -> float | — |
| lastParameter() -> float | — |
| squareConfusion() -> float | — |

## Class `Quantity`

| Member | Summary |
| --- | --- |
| `Format` (attribute) | — |
| `Unit` (attribute) | — |
| `UserString` (attribute) | — |
| `Value` (attribute) | — |
| getValueAs(unit) | — |
| toStr() | — |
| value_in(unit) | — |
| value_mm() | — |

## Class `Rotation`

| Member | Summary |
| --- | --- |
| `Angle` (attribute) | — |
| `Axes` (attribute) | — |
| `Axis` (attribute) | — |
| `Matrix` (attribute) | — |
| `Pitch` (attribute) | — |
| `Q` (attribute) | — |
| `RawAxis` (attribute) | — |
| `Roll` (attribute) | — |
| `Yaw` (attribute) | — |
| getYawPitchRoll() | — |
| inverse() | — |
| invert() | — |
| inverted() | `inverted()`: a new rotation with the inverse value (the quaternion conjugate). Upstream's `Rotation::inverse()` also negates the retained axis, so an axis set for an angle-0 rotation survives the inversion. |
| isSame(o, tol=0.0) | — |
| multVec(v) | Rotate a vector (FreeCAD `Rotation.multVec`). |
| multiply(o) | — |
| setYawPitchRoll(yaw, pitch, roll) | — |
| toMatrix() | — |

## Class `ScaleType`

Scaling mode returned by ``Matrix.hasScale()`` (``Base::ScaleType``).

| Member | Summary |
| --- | --- |
| `NoScaling` (attribute) | — |
| `NonUniformLeft` (attribute) | — |
| `NonUniformRight` (attribute) | — |
| `Other` (attribute) | — |
| `Uniform` (attribute) | — |

## Class `TypeId`

| Member | Summary |
| --- | --- |
| `Name` (attribute) | — |
| createInstance() | — |
| fromName(name) | — |
| getAllDerivedFrom(_parent) | — |

## Class `Unit`

## Class `Vector`

| Member | Summary |
| --- | --- |
| `Length` (attribute) | — |
| add(o) | — |
| cross(o) | — |
| distanceToLine(base, direction) | `distanceToLine(base, direction)`: distance from this point to the line. |
| distanceToPoint(o) | — |
| dot(o) | — |
| getAngle(o) | — |
| isEqual(o, tol) | — |
| multiply(factor) | `multiply(factor)`: scale in place (upstream mutates and returns self). |
| negative() | — |
| normalize() | `normalize()`: normalise in place and return self (upstream's semantics). |
| projectToPlane(base, normal) | `projectToPlane(base, normal)`: project in place onto the plane through `base` with the given normal. |
| scale(x, y=None, z=None) | `scale(factor)` or `scale(x, y, z)`: scale in place (upstream mutates and returns self). Draft's fillets use the three-argument form. |
| sub(o) | — |
| `x` (attribute) | — |
| `y` (attribute) | — |
| `z` (attribute) | — |

## Class `Vector2d`

| Member | Summary |
| --- | --- |
| rotate(angle) | — |
| `x` (attribute) | — |
| `y` (attribute) | — |
