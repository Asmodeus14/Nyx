// src/drivers/gpu/intel/render/math.rs
//
// Minimal CPU-side 3D math for the render engine (Phase 6).
//
// Pure software, zero GPU interaction — this is the safe half of Phase 6. We build
// model/view/projection matrices here and (initially) transform vertices on the CPU,
// feeding already-projected clip-space coordinates to the proven Phase-5 pass-through
// VS. That reuses the entire known-good pipeline and isolates "does indexed geometry +
// a transform work" from "does the depth buffer work" from "does new shader code work".
//
// Conventions (match OpenGL so the existing SF_CLIP viewport, which maps NDC [-1,1] ->
// screen with Y flipped, keeps working unchanged):
//   - Column-major storage: m[col*4 + row]. A point is a column vector; p' = M * p.
//   - Right-handed world space; perspative() produces clip space with w = -z_eye, and
//     NDC z in [-1, 1] (GL convention). The clipper's perspective divide (enabled in
//     3DSTATE_CLIP) turns this into NDC.
//
// no_std: core has no sin/cos (those live in std/libm and we pull in neither), so we
// provide small range-reduced polynomial approximations. Accuracy is ~1e-6 over a full
// turn — far more than enough for a spinning cube, and deterministic (important: the
// workflow/replay tooling forbids Math::random-style nondeterminism, and these are pure).

#![allow(dead_code)]

pub const PI: f32 = 3.14159265358979323846;
pub const TAU: f32 = 2.0 * PI;

// ---------------------------------------------------------------------------
// Scalar helpers (no_std: implement the trig we need).
// ---------------------------------------------------------------------------

/// Reduce an angle to [-PI, PI].
#[inline]
fn wrap_pi(mut x: f32) -> f32 {
    // x -= TAU * round(x / TAU)
    let k = (x / TAU + if x >= 0.0 { 0.5 } else { -0.5 }) as i32 as f32;
    x -= TAU * k;
    // Guard against fp error nudging us just outside the range.
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    x
}

/// sin(x), range-reduced 7th-order minimax-ish polynomial (Taylor coeffs, good to ~1e-6
/// on [-PI,PI]).
pub fn sin(x: f32) -> f32 {
    // Reduce to [-PI, PI], then fold into [-PI/2, PI/2] via sin(x) = sin(PI - x).
    // The Taylor poly is ~1e-6 near 0 but degrades to ~7e-3 at the endpoints, so
    // without this fold sin(PI) returns ~0.0069 instead of 0 (busted the self-test).
    let mut x = wrap_pi(x);
    if x > PI / 2.0 {
        x = PI - x;
    } else if x < -PI / 2.0 {
        x = -PI - x;
    }
    let x2 = x * x;
    // x - x^3/3! + x^5/5! - x^7/7! + x^9/9!
    x * (1.0
        + x2 * (-1.0 / 6.0
            + x2 * (1.0 / 120.0 + x2 * (-1.0 / 5040.0 + x2 * (1.0 / 362880.0)))))
}

/// cos(x) = sin(x + PI/2).
#[inline]
pub fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}

/// tan(x) = sin/cos. Undefined near PI/2; callers (perspective) stay well away.
#[inline]
pub fn tan(x: f32) -> f32 {
    sin(x) / cos(x)
}

/// Newton-Raphson sqrt (2 iters off a bit-twiddle seed). Only used by vec3 normalize.
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    // Fast inverse-sqrt seed, then refine, then invert.
    let i = x.to_bits();
    let i = 0x5f37_5a86u32.wrapping_sub(i >> 1);
    let mut y = f32::from_bits(i); // ~1/sqrt(x)
    y = y * (1.5 - 0.5 * x * y * y);
    y = y * (1.5 - 0.5 * x * y * y);
    x * y // x * (1/sqrt(x)) = sqrt(x)
}

#[inline]
pub fn radians(deg: f32) -> f32 {
    deg * (PI / 180.0)
}

// ---------------------------------------------------------------------------
// Vectors
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    #[inline]
    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    #[inline]
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    #[inline]
    pub fn length(self) -> f32 {
        sqrt(self.dot(self))
    }
    #[inline]
    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len == 0.0 {
            self
        } else {
            let inv = 1.0 / len;
            Vec3::new(self.x * inv, self.y * inv, self.z * inv)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
    pub const fn from_point(p: Vec3) -> Self {
        Self { x: p.x, y: p.y, z: p.z, w: 1.0 }
    }
    #[inline]
    pub fn to_bits(self) -> [u32; 4] {
        [self.x.to_bits(), self.y.to_bits(), self.z.to_bits(), self.w.to_bits()]
    }
}

// ---------------------------------------------------------------------------
// Mat4 — column-major (m[col*4 + row]), p' = M * p.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Mat4 {
    pub m: [f32; 16],
}

impl Mat4 {
    pub const fn zero() -> Self {
        Self { m: [0.0; 16] }
    }

    pub const fn identity() -> Self {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Self { m }
    }

    #[inline]
    fn at(&self, row: usize, col: usize) -> f32 {
        self.m[col * 4 + row]
    }

    /// self * rhs (both column-major).
    pub fn mul(&self, rhs: &Mat4) -> Mat4 {
        let mut out = Mat4::zero();
        for col in 0..4 {
            for row in 0..4 {
                let mut s = 0.0f32;
                for k in 0..4 {
                    s += self.at(row, k) * rhs.at(k, col);
                }
                out.m[col * 4 + row] = s;
            }
        }
        out
    }

    /// M * v (v is a column vector).
    pub fn mul_vec4(&self, v: Vec4) -> Vec4 {
        Vec4::new(
            self.at(0, 0) * v.x + self.at(0, 1) * v.y + self.at(0, 2) * v.z + self.at(0, 3) * v.w,
            self.at(1, 0) * v.x + self.at(1, 1) * v.y + self.at(1, 2) * v.z + self.at(1, 3) * v.w,
            self.at(2, 0) * v.x + self.at(2, 1) * v.y + self.at(2, 2) * v.z + self.at(2, 3) * v.w,
            self.at(3, 0) * v.x + self.at(3, 1) * v.y + self.at(3, 2) * v.z + self.at(3, 3) * v.w,
        )
    }

    pub fn translate(t: Vec3) -> Mat4 {
        let mut r = Mat4::identity();
        r.m[12] = t.x; // col 3, row 0
        r.m[13] = t.y;
        r.m[14] = t.z;
        r
    }

    pub fn scale(s: Vec3) -> Mat4 {
        let mut r = Mat4::identity();
        r.m[0] = s.x;
        r.m[5] = s.y;
        r.m[10] = s.z;
        r
    }

    /// U3: orthographic screen-space projection. Maps pixel coordinates in `[0,w] x [0,h]`
    /// (origin top-left, +y down) to clip/NDC `[-1,1] x [-1,1]` with Y flipped so pixel-top lands at
    /// NDC +y (the sf_clip viewport then flips NDC +y back to screen-top — net: pixel top-left == screen
    /// top-left). z collapses to a constant plane (no depth). No perspective: w passes through = 1, so a
    /// quad `MVP = ortho_screen(W,H) * translate(x,y,0) * scale(w,h,1)` places a pixel-space rectangle.
    /// This is the atom U4's GPU compositor draws each window as. Column-major (m[col*4+row]).
    pub fn ortho_screen(w: f32, h: f32) -> Mat4 {
        let mut r = Mat4::zero();
        r.m[0] = 2.0 / w;   // col0,row0: x scale
        r.m[5] = -2.0 / h;  // col1,row1: y scale (flip)
        r.m[10] = 0.0;      // col2,row2: z -> constant plane
        r.m[12] = -1.0;     // col3,row0: x translate
        r.m[13] = 1.0;      // col3,row1: y translate
        r.m[14] = 0.0;      // col3,row2: z plane = 0 (inside [0,1] depth range)
        r.m[15] = 1.0;      // col3,row3: w = 1 (affine, no perspective divide)
        r
    }

    pub fn rotate_x(a: f32) -> Mat4 {
        let (c, s) = (cos(a), sin(a));
        let mut r = Mat4::identity();
        // column-major: m[col*4+row]
        r.m[5] = c;
        r.m[6] = s; // col1,row2
        r.m[9] = -s; // col2,row1
        r.m[10] = c;
        r
    }

    pub fn rotate_y(a: f32) -> Mat4 {
        let (c, s) = (cos(a), sin(a));
        let mut r = Mat4::identity();
        r.m[0] = c;
        r.m[2] = -s; // col0,row2
        r.m[8] = s; // col2,row0
        r.m[10] = c;
        r
    }

    pub fn rotate_z(a: f32) -> Mat4 {
        let (c, s) = (cos(a), sin(a));
        let mut r = Mat4::identity();
        r.m[0] = c;
        r.m[1] = s; // col0,row1
        r.m[4] = -s; // col1,row0
        r.m[5] = c;
        r
    }

    /// Right-handed perspective, GL clip convention (NDC z in [-1,1]).
    /// fovy in radians.
    pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / tan(fovy * 0.5);
        let mut r = Mat4::zero();
        r.m[0] = f / aspect; // col0,row0
        r.m[5] = f; // col1,row1
        r.m[10] = (far + near) / (near - far); // col2,row2
        r.m[11] = -1.0; // col2,row3  (w = -z_eye)
        r.m[14] = (2.0 * far * near) / (near - far); // col3,row2
        r
    }

    /// Right-handed lookAt.
    pub fn look_at(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
        let f = center.sub(eye).normalize(); // forward
        let s = f.cross(up).normalize(); // right
        let u = s.cross(f); // true up
        let mut r = Mat4::identity();
        // Rotation (row vectors s,u,-f) stored column-major.
        r.m[0] = s.x;
        r.m[4] = s.y;
        r.m[8] = s.z;
        r.m[1] = u.x;
        r.m[5] = u.y;
        r.m[9] = u.z;
        r.m[2] = -f.x;
        r.m[6] = -f.y;
        r.m[10] = -f.z;
        // Translation.
        r.m[12] = -s.dot(eye);
        r.m[13] = -u.dot(eye);
        r.m[14] = f.dot(eye);
        r
    }
}

// ---------------------------------------------------------------------------
// Boot-time self-check — logs over serial. Catches storage-order / trig mistakes
// on the CPU before any of this feeds the GPU (where a bad transform is invisible
// among a dozen other possible hang causes).
// ---------------------------------------------------------------------------
pub fn self_test() -> bool {
    let mut ok = true;
    let mut fail: u32 = 0; // bitmask of which check failed, for diagnosis
    let approx = |a: f32, b: f32| -> bool { (a - b).abs() < 1e-3 };
    macro_rules! chk {
        ($bit:expr, $cond:expr) => {
            if !($cond) {
                ok = false;
                fail |= 1 << $bit;
            }
        };
    }

    // Trig sanity.
    chk!(0, approx(sin(0.0), 0.0));
    chk!(1, approx(sin(PI / 2.0), 1.0));
    chk!(2, approx(cos(0.0), 1.0));
    chk!(3, approx(cos(PI), -1.0));
    chk!(4, approx(sin(PI), 0.0));

    // identity * v == v.
    let v = Vec4::new(1.0, 2.0, 3.0, 1.0);
    let iv = Mat4::identity().mul_vec4(v);
    chk!(5, approx(iv.x, 1.0) && approx(iv.y, 2.0) && approx(iv.z, 3.0) && approx(iv.w, 1.0));

    // translate then check point moves.
    let t = Mat4::translate(Vec3::new(5.0, -2.0, 1.0)).mul_vec4(Vec4::new(0.0, 0.0, 0.0, 1.0));
    chk!(6, approx(t.x, 5.0) && approx(t.y, -2.0) && approx(t.z, 1.0));

    // rotate_z(90deg) maps +X -> +Y.
    let rz = Mat4::rotate_z(PI / 2.0).mul_vec4(Vec4::new(1.0, 0.0, 0.0, 1.0));
    chk!(7, approx(rz.x, 0.0) && approx(rz.y, 1.0));

    // (A*B)*v == A*(B*v): matrix mul associativity/order check.
    let a = Mat4::rotate_y(0.7);
    let b = Mat4::translate(Vec3::new(1.0, 2.0, 3.0));
    let via_mat = a.mul(&b).mul_vec4(v);
    let via_seq = a.mul_vec4(b.mul_vec4(v));
    chk!(8, approx(via_mat.x, via_seq.x)
        && approx(via_mat.y, via_seq.y)
        && approx(via_mat.z, via_seq.z));

    // perspective: a point on -Z inside the frustum yields |ndc| <= 1 after divide.
    let p = Mat4::perspective(radians(60.0), 16.0 / 9.0, 0.1, 100.0)
        .mul_vec4(Vec4::new(0.0, 0.0, -1.0, 1.0));
    chk!(9, p.w > 0.0); // w = -z_eye = 1.0
    let ndc_z = p.z / p.w;
    chk!(10, ndc_z > -1.001 && ndc_z < 1.001);

    if ok {
        crate::serial_println!("[MATH] self-test PASS (trig, mat4 mul, translate/rotate, perspective)");
    } else {
        crate::serial_println!("[MATH] self-test FAIL — failing-check bitmask=0x{:x}", fail);
    }
    ok
}
