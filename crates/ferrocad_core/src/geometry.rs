//! Core geometry types used by FreeCAD's `Base` module: `Vector3`, `Matrix4`,
//! `Rotation`, `Placement`, and `TypeId`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Vector3
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Vector3 { x, y, z }
    }

    pub fn zero() -> Self {
        Vector3::new(0.0, 0.0, 0.0)
    }

    pub fn add(&self, o: &Vector3) -> Vector3 {
        Vector3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    pub fn sub(&self, o: &Vector3) -> Vector3 {
        Vector3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(&self, f: f64) -> Vector3 {
        Vector3::new(self.x * f, self.y * f, self.z * f)
    }

    pub fn neg(&self) -> Vector3 {
        Vector3::new(-self.x, -self.y, -self.z)
    }

    pub fn dot(&self, o: &Vector3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(&self, o: &Vector3) -> Vector3 {
        Vector3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(&self) -> f64 {
        self.dot(self).sqrt()
    }

    /// Squared length (FreeCAD `Vector3::Sqr`).
    pub fn sqr(&self) -> f64 {
        self.dot(self)
    }

    /// Exact null test (FreeCAD `Vector3::IsNull` compares components to 0).
    pub fn is_null(&self) -> bool {
        self.x == 0.0 && self.y == 0.0 && self.z == 0.0
    }

    pub fn normalize(&self) -> Vector3 {
        let l = self.length();
        if l == 0.0 {
            Vector3::zero()
        } else {
            self.scale(1.0 / l)
        }
    }

    pub fn distance(&self, o: &Vector3) -> f64 {
        self.sub(o).length()
    }

    pub fn angle(&self, o: &Vector3) -> f64 {
        let l = self.length() * o.length();
        if l == 0.0 {
            f64::NAN
        } else {
            (self.dot(o) / l).clamp(-1.0, 1.0).acos()
        }
    }

    pub fn is_equal(&self, o: &Vector3, tol: f64) -> bool {
        self.distance(o) <= tol
    }
}

// ---------------------------------------------------------------------------
// ScaleType (FreeCAD `Base::ScaleType`, mirrored by `FreeCAD.ScaleType`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ScaleType {
    Other = -1,
    NoScaling = 0,
    NonUniformRight = 1,
    NonUniformLeft = 2,
    Uniform = 3,
}

// ---------------------------------------------------------------------------
// Matrix4 (row-major [16]; identity is diag(1,1,1,1))
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Matrix4 {
    pub m: [f64; 16],
}

impl Matrix4 {
    pub fn identity() -> Self {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Matrix4 { m }
    }

    pub fn from_values(values: [f64; 16]) -> Self {
        Matrix4 { m: values }
    }

    /// Multiply two matrices (this * other).
    pub fn mul(&self, o: &Matrix4) -> Matrix4 {
        let mut r = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.m[i * 4 + k] * o.m[k * 4 + j];
                }
                r[i * 4 + j] = s;
            }
        }
        Matrix4 { m: r }
    }

    /// Transform a point (w=1), row-major `M * v`.
    pub fn transform(&self, v: &Vector3) -> Vector3 {
        let m = &self.m;
        let w = m[12] * v.x + m[13] * v.y + m[14] * v.z + m[15];
        let x = (m[0] * v.x + m[1] * v.y + m[2] * v.z + m[3]) / w;
        let y = (m[4] * v.x + m[5] * v.y + m[6] * v.z + m[7]) / w;
        let z = (m[8] * v.x + m[9] * v.y + m[10] * v.z + m[11]) / w;
        Vector3::new(x, y, z)
    }

    /// The 4x4 matrix elements in FreeCAD's A11..A44 order.
    pub fn values(&self) -> [f64; 16] {
        self.m
    }

    pub fn is_unity(&self, tol: f64) -> bool {
        Matrix4::identity()
            .m
            .iter()
            .zip(self.m.iter())
            .all(|(a, b)| (a - b).abs() <= tol)
    }

    pub fn is_null(&self) -> bool {
        self.m.iter().all(|v| v.abs() <= 1e-30)
    }

    pub fn unity(&mut self) {
        *self = Matrix4::identity();
    }

    pub fn nullify(&mut self) {
        self.m = [0.0; 16];
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.m;
        m[0] * det3(m[5], m[6], m[7], m[9], m[10], m[11], m[13], m[14], m[15])
            - m[1] * det3(m[4], m[6], m[7], m[8], m[10], m[11], m[12], m[14], m[15])
            + m[2] * det3(m[4], m[5], m[7], m[8], m[9], m[11], m[12], m[13], m[15])
            - m[3] * det3(m[4], m[5], m[6], m[8], m[9], m[10], m[12], m[13], m[14])
    }

    /// Determinant of the top-left 3x3 block (FreeCAD `Matrix4D::determinant3`).
    pub fn determinant3(&self) -> f64 {
        let m = &self.m;
        let va = m[0] * m[5] * m[10];
        let vb = m[1] * m[6] * m[8];
        let vc = m[4] * m[9] * m[2];
        let vd = m[2] * m[5] * m[8];
        let ve = m[4] * m[1] * m[10];
        let vf = m[0] * m[9] * m[6];
        (va + vb + vc) - (vd + ve + vf)
    }

    pub fn transpose(&self) -> Matrix4 {
        let m = &self.m;
        let mut r = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                r[i * 4 + j] = m[j * 4 + i];
            }
        }
        Matrix4 { m: r }
    }

    /// The inverse, or `None` if the matrix is (near-)singular.
    pub fn inverse(&self) -> Option<Matrix4> {
        let det = self.determinant();
        if det.abs() < 1e-15 {
            return None;
        }
        // Adjugate / det.
        let m = &self.m;
        let mut r = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                // cofactor C_ji (transposed)
                let mut minor = [[0.0; 3]; 3];
                let mut mi = 0;
                for ri in 0..4 {
                    if ri == j {
                        continue;
                    }
                    let mut mj = 0;
                    for ci in 0..4 {
                        if ci == i {
                            continue;
                        }
                        minor[mi][mj] = m[ri * 4 + ci];
                        mj += 1;
                    }
                    mi += 1;
                }
                let c = det3(
                    minor[0][0], minor[0][1], minor[0][2],
                    minor[1][0], minor[1][1], minor[1][2],
                    minor[2][0], minor[2][1], minor[2][2],
                );
                let sign = if (i + j) % 2 == 0 { 1.0 } else { -1.0 };
                r[i * 4 + j] = sign * c / det;
            }
        }
        Some(Matrix4 { m: r })
    }

    /// In-place inverse via Gauss-Jordan with partial pivoting, mirroring
    /// FreeCAD's `Matrix4D::inverseGauss` (bit-for-bit, including the GL
    /// transpose in/out). For an orthonormal frame this yields the transpose.
    pub fn inverse_gauss(&mut self) {
        let mut matrix = [0.0f64; 16];
        for i in 0..4 {
            for j in 0..4 {
                matrix[i + 4 * j] = self.m[4 * i + j];
            }
        }
        let mut inv = Matrix4::identity().m;
        if gauss_invert(&mut matrix, &mut inv) {
            for i in 0..4 {
                for j in 0..4 {
                    self.m[4 * i + j] = inv[i + 4 * j];
                }
            }
        } else {
            *self = self.transpose();
        }
    }

    pub fn row(&self, r: usize) -> Vector3 {
        Vector3::new(self.m[r * 4], self.m[r * 4 + 1], self.m[r * 4 + 2])
    }

    pub fn set_row(&mut self, r: usize, v: Vector3) {
        self.m[r * 4] = v.x;
        self.m[r * 4 + 1] = v.y;
        self.m[r * 4 + 2] = v.z;
    }

    pub fn col(&self, c: usize) -> Vector3 {
        Vector3::new(self.m[c], self.m[4 + c], self.m[8 + c])
    }

    pub fn set_col(&mut self, c: usize, v: Vector3) {
        self.m[c] = v.x;
        self.m[4 + c] = v.y;
        self.m[8 + c] = v.z;
    }

    pub fn diagonal(&self) -> Vector3 {
        Vector3::new(self.m[0], self.m[5], self.m[10])
    }

    pub fn set_diagonal(&mut self, v: Vector3) {
        self.m[0] = v.x;
        self.m[5] = v.y;
        self.m[10] = v.z;
    }

    /// Pre-multiply by a translation.
    pub fn pre_move(&mut self, t: Vector3) {
        let mut m = Matrix4::identity();
        m.m[3] = t.x;
        m.m[7] = t.y;
        m.m[11] = t.z;
        *self = m.mul(self);
    }

    /// Pre-multiply by a (possibly non-uniform) scale.
    pub fn pre_scale(&mut self, s: Vector3) {
        let mut m = Matrix4::identity();
        m.m[0] = s.x;
        m.m[5] = s.y;
        m.m[10] = s.z;
        *self = m.mul(self);
    }

    /// Pre-multiply by a rotation about the X/Y/Z axis by `angle` (radians).
    pub fn pre_rotate(&mut self, axis: usize, angle: f64) {
        let unit = match axis {
            0 => Vector3::new(1.0, 0.0, 0.0),
            1 => Vector3::new(0.0, 1.0, 0.0),
            _ => Vector3::new(0.0, 0.0, 1.0),
        };
        let r = Rotation::from_axis_angle(&unit, angle).to_matrix();
        *self = r.mul(self);
    }

    /// Classify the linear part as scaled/rotated/sheared (FreeCAD
    /// `Matrix4D::hasScale`). Distinguishes scaling applied from the left
    /// (pre-multiplied, "Right") from the right (post-multiplied, "Left").
    pub fn has_scale(&self, tol: f64) -> ScaleType {
        let tol = if tol == 0.0 { 1e-9 } else { tol };
        let close_abs = |a: f64, b: f64| {
            let (aa, ab) = (a.abs(), b.abs());
            if ab > aa {
                (ab - aa) / ab <= tol
            } else if aa > ab {
                (aa - ab) / aa <= tol
            } else {
                true
            }
        };

        let dx = self.col(0).sqr();
        let dy = self.col(1).sqr();
        let dz = self.col(2).sqr();
        let dxyz = (dx * dy * dz).sqrt();

        let du = self.row(0).sqr();
        let dv = self.row(1).sqr();
        let dw = self.row(2).sqr();
        let duvw = (du * dv * dw).sqrt();

        let d3 = self.determinant3();

        // projection / shearing / ...
        if !close_abs(dxyz, d3) && !close_abs(duvw, d3) {
            return ScaleType::Other;
        }
        if close_abs(duvw, d3) && (!close_abs(du, dv) || !close_abs(dv, dw)) {
            return ScaleType::NonUniformLeft;
        }
        if close_abs(dxyz, d3) && (!close_abs(dx, dy) || !close_abs(dy, dz)) {
            return ScaleType::NonUniformRight;
        }
        if (d3 - 1.0).abs() > tol {
            return ScaleType::Uniform;
        }
        ScaleType::NoScaling
    }

    /// Decompose into `[shear, scale, rotation, move]` such that
    /// `self == move * rotation * scale * shear` (FreeCAD `Matrix4D::decompose`).
    pub fn decompose(&self) -> [Matrix4; 4] {
        let mut move_matrix = Matrix4::identity();
        move_matrix.set_col(3, self.col(3));
        let mut residual = *self;
        residual.set_col(3, Vector3::zero());

        // Find an orthonormal frame from the (possibly scaled) column vectors.
        let mut prim_dir: i32 = -1;
        let mut dirs = [
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ];
        for i in 0..3 {
            if residual.col(i).is_null() {
                continue;
            }
            if prim_dir < 0 {
                dirs[i] = residual.col(i).normalize();
                prim_dir = i as i32;
                continue;
            }

            let cross = dirs[prim_dir as usize].cross(&residual.col(i));
            if cross.is_null() {
                continue;
            }
            let cross = cross.normalize();
            let last_dir = 3 - i - prim_dir as usize;
            if i as i32 - prim_dir == 1 {
                dirs[last_dir] = cross;
                dirs[i] = cross.cross(&dirs[prim_dir as usize]);
            } else {
                dirs[last_dir] = cross.neg();
                dirs[i] = dirs[prim_dir as usize].cross(&cross.neg());
            }
            prim_dir = -2; // done
            break;
        }
        if prim_dir >= 0 {
            // only one valid direction
            let pd = prim_dir as usize;
            let mut cross = dirs[pd].cross(&Vector3::new(0.0, 0.0, 1.0));
            if cross.is_null() {
                cross = dirs[pd].cross(&Vector3::new(0.0, 1.0, 0.0));
            }
            dirs[(pd + 1) % 3] = cross;
            dirs[(pd + 2) % 3] = dirs[pd].cross(&cross);
        }

        let mut rotation = Matrix4::identity();
        rotation.set_col(0, dirs[0]);
        rotation.set_col(1, dirs[1]);
        rotation.set_col(2, dirs[2]);
        // `inverseGauss` on an orthonormal frame is effectively the transpose.
        rotation.inverse_gauss();
        residual = rotation.mul(&residual);
        // Keep the signs of the scale factors equal.
        if residual.determinant() < 0.0 {
            rotation.pre_rotate(2, std::f64::consts::PI);
            residual.pre_rotate(2, std::f64::consts::PI);
        }
        rotation.inverse_gauss();

        // Extract scale.
        let x_scale = residual.m[0];
        let y_scale = residual.m[5];
        let z_scale = residual.m[10];
        let mut scale_matrix = Matrix4::identity();
        scale_matrix.m[0] = x_scale;
        scale_matrix.m[5] = y_scale;
        scale_matrix.m[10] = z_scale;

        // The remaining shear.
        residual.pre_scale(Vector3::new(
            if x_scale != 0.0 { 1.0 / x_scale } else { 1.0 },
            if y_scale != 0.0 { 1.0 / y_scale } else { 1.0 },
            if z_scale != 0.0 { 1.0 / z_scale } else { 1.0 },
        ));
        residual.set_diagonal(Vector3::new(1.0, 1.0, 1.0));

        // Remove values close to zero.
        for i in 0..3 {
            if scale_matrix.m[i * 4 + i].abs() < 1e-15 {
                scale_matrix.m[i * 4 + i] = 0.0;
            }
            for j in 0..3 {
                if residual.m[i * 4 + j].abs() < 1e-15 {
                    residual.m[i * 4 + j] = 0.0;
                }
                if rotation.m[i * 4 + j].abs() < 1e-15 {
                    rotation.m[i * 4 + j] = 0.0;
                }
            }
        }

        [residual, scale_matrix, rotation, move_matrix]
    }
}

/// A 3x3 determinant (row-major).
fn det3(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) -> f64 {
    a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
}

/// Gauss-Jordan inversion of `a` (row-major [16]); writes the inverse into `b`.
/// A direct port of FreeCAD's `Matrix_gauss`. Returns `false` if singular.
fn gauss_invert(a: &mut [f64; 16], b: &mut [f64; 16]) -> bool {
    let mut ipiv = [0i32; 4];
    let mut indxr = [0usize; 4];
    let mut indxc = [0usize; 4];
    for i in 0..4 {
        let mut big = 0.0f64;
        let mut irow = 0usize;
        let mut icol = 0usize;
        for j in 0..4 {
            if ipiv[j] != 1 {
                for k in 0..4 {
                    if ipiv[k] == 0 {
                        if a[4 * j + k].abs() >= big {
                            big = a[4 * j + k].abs();
                            irow = j;
                            icol = k;
                        }
                    } else if ipiv[k] > 1 {
                        return false;
                    }
                }
            }
        }
        ipiv[icol] += 1;
        if irow != icol {
            for l in 0..4 {
                a.swap(4 * irow + l, 4 * icol + l);
                b.swap(4 * irow + l, 4 * icol + l);
            }
        }
        indxr[i] = irow;
        indxc[i] = icol;
        if a[4 * icol + icol] == 0.0 {
            return false;
        }
        let pivinv = 1.0 / a[4 * icol + icol];
        a[4 * icol + icol] = 1.0;
        for l in 0..4 {
            a[4 * icol + l] *= pivinv;
            b[4 * icol + l] *= pivinv;
        }
        for ll in 0..4 {
            if ll != icol {
                let dum = a[4 * ll + icol];
                a[4 * ll + icol] = 0.0;
                for l in 0..4 {
                    a[4 * ll + l] -= a[4 * icol + l] * dum;
                    b[4 * ll + l] -= b[4 * icol + l] * dum;
                }
            }
        }
    }
    for l in (0..4).rev() {
        if indxr[l] != indxc[l] {
            for k in 0..4 {
                a.swap(4 * k + indxr[l], 4 * k + indxc[l]);
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Rotation (quaternion: w, x, y, z)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rotation {
    pub q: [f64; 4],
    /// The axis the rotation was last set with (FreeCAD `_axis`). The quaternion
    /// alone cannot represent "axis X, angle 0", and `RawAxis` must survive a
    /// save, so it is retained here when set explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_axis: Option<Vector3>,
}

/// Two rotations are equal when their quaternions match; the cached raw axis is
/// presentation metadata and is ignored (mirroring FreeCAD).
impl PartialEq for Rotation {
    fn eq(&self, other: &Self) -> bool {
        self.q == other.q
    }
}

impl Rotation {
    pub fn identity() -> Self {
        Rotation { q: [1.0, 0.0, 0.0, 0.0], raw_axis: None }
    }

    /// Rotation around `axis` (normalized) by `angle` (radians).
    pub fn from_axis_angle(axis: &Vector3, angle: f64) -> Self {
        // FreeCAD normalizes the angle into [0, 2*pi) inside `setValue(axis, angle)`.
        let two_pi = 2.0 * std::f64::consts::PI;
        let angle = angle - (angle / two_pi).floor() * two_pi;
        let a = axis.normalize();
        let half = angle * 0.5;
        let s = half.sin();
        Rotation { q: [half.cos(), a.x * s, a.y * s, a.z * s], raw_axis: None }
    }

    pub fn to_matrix(&self) -> Matrix4 {
        // Normalize the quaternion first, matching FreeCAD `Rotation::getValue(Matrix4D&)`.
        let l = (self.q[0] * self.q[0]
            + self.q[1] * self.q[1]
            + self.q[2] * self.q[2]
            + self.q[3] * self.q[3])
            .sqrt();
        let (w, x, y, z) = if l > 0.0 {
            (self.q[0] / l, self.q[1] / l, self.q[2] / l, self.q[3] / l)
        } else {
            (1.0, 0.0, 0.0, 0.0)
        };
        Matrix4::from_values([
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
            0.0,
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
            0.0,
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }

    /// Rotation angle in radians (about the rotation axis).
    /// Mirrors FreeCAD `Rotation::evaluateVector`: a quaternion whose scalar
    /// part is not strictly inside (-1, 1) is treated as angle 0.
    pub fn angle(&self) -> f64 {
        let w = self.q[0];
        if w > -1.0 && w < 1.0 {
            2.0 * w.acos()
        } else {
            0.0
        }
    }

    /// The rotation axis. Prefers the axis retained by an explicit
    /// `setAxis`/`setAngle` (FreeCAD `getRawAxis`/`getAxis`), falling back to
    /// the axis derived from the quaternion (Z if near-identity).
    pub fn axis(&self) -> Vector3 {
        if let Some(a) = self.raw_axis {
            return a;
        }
        let w = self.q[0];
        if !(-1.0..=1.0).contains(&w) {
            return Vector3::new(0.0, 0.0, 1.0);
        }
        let s = (1.0 - w * w).sqrt();
        if s < 1e-12 {
            Vector3::new(0.0, 0.0, 1.0)
        } else {
            Vector3::new(self.q[1] / s, self.q[2] / s, self.q[3] / s)
        }
    }

    /// Rebuild the rotation about the current axis with a new `angle` (radians).
    /// A retained raw axis is preserved.
    pub fn set_angle(&mut self, angle: f64) {
        let raw = self.raw_axis;
        *self = Rotation::from_axis_angle(&self.axis(), angle);
        self.raw_axis = raw;
    }

    /// Quaternion product (`self * o`): apply `o` first, then `self`.
    pub fn multiply(&self, o: &Rotation) -> Rotation {
        let (w1, x1, y1, z1) = (self.q[0], self.q[1], self.q[2], self.q[3]);
        let (w2, x2, y2, z2) = (o.q[0], o.q[1], o.q[2], o.q[3]);
        Rotation {
            q: [
                w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
                w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
                w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
                w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
            ],
            raw_axis: None,
        }
    }

    /// The inverse rotation (quaternion conjugate).
    pub fn inverse(&self) -> Rotation {
        Rotation { q: [self.q[0], -self.q[1], -self.q[2], -self.q[3]], raw_axis: None }
    }

    /// Whether two rotations are equal within `tol`.
    pub fn is_same(&self, o: &Rotation, tol: f64) -> bool {
        let dot = self.q[0] * o.q[0] + self.q[1] * o.q[1] + self.q[2] * o.q[2] + self.q[3] * o.q[3];
        (1.0 - dot.abs()) <= tol.max(1e-12)
    }

    /// FreeCAD's `Rotation(yaw, pitch, roll)` (degrees) = Rz*Ry*Rx.
    pub fn from_euler_deg(yaw: f64, pitch: f64, roll: f64) -> Rotation {
        Rotation::from_yaw_pitch_roll(
            yaw.to_radians(),
            pitch.to_radians(),
            roll.to_radians(),
        )
    }

    pub fn from_yaw_pitch_roll(yaw: f64, pitch: f64, roll: f64) -> Rotation {
        // Direct half-angle formula, mirroring FreeCAD `Rotation::setYawPitchRoll`
        // (XY'Z''). Building it from `from_axis_angle` would normalize the angles
        // and flip quaternion signs, which the gimbal-lock read-back is sensitive to.
        let c1 = (yaw / 2.0).cos();
        let s1 = (yaw / 2.0).sin();
        let c2 = (pitch / 2.0).cos();
        let s2 = (pitch / 2.0).sin();
        let c3 = (roll / 2.0).cos();
        let s3 = (roll / 2.0).sin();
        Rotation {
            q: [
                c1 * c2 * c3 + s1 * s2 * s3,
                c1 * c2 * s3 - s1 * s2 * c3,
                c1 * s2 * c3 + s1 * c2 * s3,
                s1 * c2 * c3 - c1 * s2 * s3,
            ],
            raw_axis: None,
        }
    }

    /// Rotation that maps `from` onto `to` (FreeCAD `Rotation::setValue(from, to)`).
    pub fn from_vectors(from: Vector3, to: Vector3) -> Rotation {
        let u = from.normalize();
        let v = to.normalize();
        let dot = u.dot(&v);
        let w = u.cross(&v);
        if w.length() == 0.0 {
            if dot > 0.0 {
                // Parallel, same direction.
                Rotation::identity()
            } else {
                // Anti-parallel: any axis perpendicular to `u`.
                let mut t = u.cross(&Vector3::new(1.0, 0.0, 0.0));
                if t.length() < 1e-15 {
                    t = u.cross(&Vector3::new(0.0, 1.0, 0.0));
                }
                let t = t.normalize();
                Rotation { q: [0.0, t.x, t.y, t.z], raw_axis: None }
            }
        } else {
            Rotation::from_axis_angle(&w, dot.clamp(-1.0, 1.0).acos())
        }
    }

    /// Extract `(yaw, pitch, roll)` (radians) from R = Rz*Ry*Rx, mirroring
    /// FreeCAD `Rotation::getYawPitchRoll` (quaternion-based, OCC gimbal tolerance).
    pub fn yaw_pitch_roll(&self) -> (f64, f64, f64) {
        // Upstream `quat` is (x, y, z, w); ours is [w, x, y, z].
        let (x, y, z, w) = (self.q[1], self.q[2], self.q[3], self.q[0]);
        let (q00, q11, q22, q33) = (x * x, y * y, z * z, w * w);
        let (q01, q02, q03) = (x * y, x * z, x * w);
        let (q12, q13, q23) = (y * z, y * w, z * w);
        let qd2 = 2.0 * (q13 - q02);
        let tol = 16.0 * f64::EPSILON;
        let half_pi = std::f64::consts::FRAC_PI_2;
        if (qd2 - 1.0).abs() <= tol {
            // north pole
            (0.0, half_pi, 2.0 * x.atan2(w))
        } else if (qd2 + 1.0).abs() <= tol {
            // south pole
            (0.0, -half_pi, 2.0 * x.atan2(w))
        } else {
            let yaw = (2.0 * (q01 + q23)).atan2((q00 + q33) - (q11 + q22));
            let pitch = if qd2 > 1.0 {
                half_pi
            } else if qd2 < -1.0 {
                -half_pi
            } else {
                qd2.asin()
            };
            let roll = (2.0 * (q12 + q03)).atan2((q22 + q33) - (q00 + q11));
            (yaw, pitch, roll)
        }
    }

    /// Extract a rotation from a matrix, mirroring FreeCAD `Rotation::setValue(Matrix4D)`:
    /// take the rotation part of the decomposition, then read the quaternion.
    pub fn from_matrix(m: &Matrix4) -> Rotation {
        let mc = m.decompose()[2];
        let g = |r: usize, c: usize| mc.m[4 * r + c];
        let (m00, m11, m22) = (g(0, 0), g(1, 1), g(2, 2));
        let trace = m00 + m11 + m22;
        if trace > 0.0 {
            let s = (1.0 + trace).sqrt();
            let w = 0.5 * s;
            let s = 0.5 / s;
            Rotation {
                q: [w, (g(2, 1) - g(1, 2)) * s, (g(0, 2) - g(2, 0)) * s, (g(1, 0) - g(0, 1)) * s],
                raw_axis: None,
            }
        } else {
            let mut i = 0usize;
            if m11 > m00 {
                i = 1;
            }
            let mm = [m00, m11, m22];
            if mm[2] > mm[i] {
                i = 2;
            }
            let j = (i + 1) % 3;
            let k = (i + 2) % 3;
            let s = ((mm[i] - (mm[j] + mm[k])) + 1.0).sqrt();
            let mut q = [0.0f64; 4];
            q[1 + i] = s * 0.5;
            let s = 0.5 / s;
            q[0] = (g(k, j) - g(j, k)) * s;
            q[1 + j] = (g(j, i) + g(i, j)) * s;
            q[1 + k] = (g(k, i) + g(i, k)) * s;
            Rotation { q, raw_axis: None }
        }
    }
}

// ---------------------------------------------------------------------------
// Placement (base point + rotation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub base: Vector3,
    pub rotation: Rotation,
}

impl Placement {
    pub fn identity() -> Self {
        Placement { base: Vector3::zero(), rotation: Rotation::identity() }
    }

    pub fn new(base: Vector3, rotation: Rotation) -> Self {
        Placement { base, rotation }
    }

    /// Compose two placements (this * other).
    pub fn mul(&self, o: &Placement) -> Placement {
        let rot = self.rotation.to_matrix();
        Placement {
            base: self.base.add(&rot.transform(&o.base)),
            rotation: Rotation::identity(), // quaternion composition elided for POC
        }
    }

    pub fn to_matrix(&self) -> Matrix4 {
        let mut m = self.rotation.to_matrix();
        // Row-major: translation occupies the 4th column (m[3], m[7], m[11]).
        m.m[3] = self.base.x;
        m.m[7] = self.base.y;
        m.m[11] = self.base.z;
        m
    }

    /// The inverse placement.
    pub fn inverse(&self) -> Placement {
        let inv = self.rotation.inverse().to_matrix();
        Placement {
            base: inv.transform(&self.base.neg()),
            rotation: self.rotation.inverse(),
        }
    }

    /// Whether two placements are equal within `tol`.
    pub fn is_same(&self, o: &Placement, tol: f64) -> bool {
        self.base.is_equal(&o.base, tol.max(1e-12))
            && self.rotation.is_same(&o.rotation, tol)
    }
}

// ---------------------------------------------------------------------------
// TypeId (an interned type-name identifier)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeId(pub String);

impl TypeId {
    pub fn from_name(name: &str) -> Self {
        TypeId(name.to_string())
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_arithmetic() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);
        assert_eq!(a.add(&b), Vector3::new(5.0, 7.0, 9.0));
        assert_eq!(a.dot(&b), 32.0);
        assert_eq!(a.cross(&b), Vector3::new(-3.0, 6.0, -3.0));
        assert!((Vector3::new(3.0, 4.0, 0.0).length() - 5.0).abs() < 1e-12);
    }

    #[test]
    fn matrix_identity_and_transform() {
        let id = Matrix4::identity();
        let v = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(id.transform(&v), v);
    }

    #[test]
    fn rotation_identity_angle() {
        let pi = std::f64::consts::PI;
        assert!(Rotation::identity().angle().abs() < 1e-12);
        let r = Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), pi);
        assert!((r.angle() - pi).abs() < 1e-12);
    }

    #[test]
    fn rotation_direction_matches_right_hand_rule() {
        let half = std::f64::consts::FRAC_PI_2;
        // Rotation about +Z by +90° maps +X to +Y.
        let r = Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), half);
        let v = r.to_matrix().transform(&Vector3::new(1.0, 0.0, 0.0));
        assert!(v.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9));

        // Rotation about +Y by -90° maps +X to +Z.
        let r = Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), -half);
        let v = r.to_matrix().transform(&Vector3::new(1.0, 0.0, 0.0));
        assert!(v.is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9));
    }

    #[test]
    fn matrix_has_scale_classifies() {
        assert_eq!(Matrix4::identity().has_scale(0.0), ScaleType::NoScaling);

        let mut non_uniform = Matrix4::identity();
        non_uniform.pre_scale(Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(non_uniform.has_scale(0.0), ScaleType::NonUniformLeft);

        let mut uniform = Matrix4::identity();
        uniform.pre_scale(Vector3::new(2.0, 2.0, 2.0));
        assert_eq!(uniform.has_scale(0.0), ScaleType::Uniform);

        // A pure rotation is not a scale.
        let rot = Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 1.0).to_matrix();
        assert_eq!(rot.has_scale(0.0), ScaleType::NoScaling);

        // Shearing is neither.
        let mut shear = Matrix4::identity();
        shear.set_row(1, Vector3::new(0.0, 1.0, 1.0));
        assert_eq!(shear.has_scale(0.0), ScaleType::Other);
    }

    #[test]
    fn matrix_decompose_round_trips() {
        // move * rotation * scale (shear-free) must recompose.
        let rot = Rotation::from_yaw_pitch_roll(0.3, -0.4, 0.5).to_matrix();
        let mut scale_m = Matrix4::identity();
        scale_m.pre_scale(Vector3::new(2.0, 3.0, 4.0));
        let mut m = rot.mul(&scale_m);
        m.set_col(3, Vector3::new(1.0, 2.0, 3.0));

        let [shear, scale, rotation, mv] = m.decompose();
        let rebuilt = mv.mul(&rotation.mul(&scale.mul(&shear)));
        for (a, b) in m.m.iter().zip(rebuilt.m.iter()) {
            assert!((a - b).abs() <= 1e-9, "recompose mismatch: {a} vs {b}");
        }
        assert!(shear.is_unity(1e-9));
        assert!((scale.m[0] - 2.0).abs() < 1e-9);
        assert!((scale.m[5] - 3.0).abs() < 1e-9);
        assert!((scale.m[10] - 4.0).abs() < 1e-9);
        assert!(mv.col(3).is_equal(&Vector3::new(1.0, 2.0, 3.0), 1e-12));
    }

    #[test]
    fn rotation_from_vectors_maps_source_to_target() {
        let identity = Rotation::from_vectors(Vector3::new(0.0, 0.0, 1.0), Vector3::new(0.0, 0.0, 1.0));
        assert!(identity.is_same(&Rotation::identity(), 1e-12));

        let r = Rotation::from_vectors(Vector3::new(1.0, 0.0, 0.0), Vector3::new(0.0, 1.0, 0.0));
        let v = r.to_matrix().transform(&Vector3::new(1.0, 0.0, 0.0));
        assert!(v.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9));
    }

    #[test]
    fn rotation_angle_wraps_to_two_pi() {
        let a = Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 270f64.to_radians());
        let b = Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), 630f64.to_radians());
        assert!(a.is_same(&b, 1e-12));
        assert!(a.axis().is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-12));
        assert!(b.axis().is_equal(&Vector3::new(1.0, 0.0, 0.0), 1e-12));
    }
}
