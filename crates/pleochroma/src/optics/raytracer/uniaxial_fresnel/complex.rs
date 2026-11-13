//! Minimal complex-number and complex-3-vector arithmetic.
//!
//! This crate has no `num-complex` dependency, and the WGSL mirror needs the exact
//! same explicit re/im arithmetic (a `vec2<f32>`-based complex type there,
//! hand-written `add`/`mul`/`div`/`sqrt`, exactly mirrors [`Cplx`]).

use glam::Vec3;

/// Minimal complex number. Hand-written so the WGSL `vec2<f32>` mirror can match it
/// operation for operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cplx {
    pub re: f32,
    pub im: f32,
}

impl Cplx {
    pub(crate) const ZERO: Self = Self { re: 0.0, im: 0.0 };

    #[inline]
    pub(crate) const fn re(re: f32) -> Self {
        Self { re, im: 0.0 }
    }

    #[inline]
    pub(crate) const fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }

    #[inline]
    pub(crate) const fn sub(self, o: Self) -> Self {
        Self {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }

    #[inline]
    pub(crate) const fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }

    #[inline]
    pub(crate) const fn scale(self, s: f32) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    #[inline]
    pub(crate) const fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    #[inline]
    pub(crate) fn norm_sqr(self) -> f32 {
        self.im.mul_add(self.im, self.re * self.re)
    }

    /// Complex division. The squared divisor magnitude is floored at `1e-20`, so a zero
    /// divisor gives a finite result instead of NaN.
    #[inline]
    pub(crate) fn div(self, o: Self) -> Self {
        let denom = o.im.mul_add(o.im, o.re * o.re).max(1e-20);
        Self {
            re: self.im.mul_add(o.im, self.re * o.re) / denom,
            im: self.re.mul_add(-o.im, self.im * o.re) / denom,
        }
    }

    /// Principal square root with the branch chosen so a negative real input returns
    /// `im > 0`: the evanescent root then decays as `z -> +infinity`, matching the
    /// forward `zhat` convention.
    ///
    /// A pure negative-real input (`im == 0.0`, the usual internal-TIR case) takes the
    /// direct `sqrt(-re)` path. Through `atan2` the sign of the near-zero real part is
    /// rounding noise, which can pick the wrong branch and differ between CPU and GPU
    /// transcendental implementations.
    #[inline]
    pub(crate) fn sqrt_forward_branch(self) -> Self {
        if self.im == 0.0 && self.re < 0.0 {
            return Self {
                re: 0.0,
                im: (-self.re).sqrt(),
            };
        }
        if self.im == 0.0 {
            // Real non-negative argument: the general path below gives these same bits
            // (`atan2(0, x) = 0`, `cos 0 = 1`, `sin 0 = 0`) at the cost of three
            // transcendental calls.
            return Self {
                re: self.norm_sqr().sqrt().sqrt(),
                im: 0.0,
            };
        }
        let r = self.norm_sqr().sqrt().sqrt();
        let theta = self.im.atan2(self.re) * 0.5;
        let mut out = Self {
            re: r * theta.cos(),
            im: r * theta.sin(),
        };
        if out.re < 0.0 {
            out = Self {
                re: -out.re,
                im: -out.im,
            };
        }
        if out.re.abs() < 1e-9 && out.im < 0.0 {
            out = Self {
                re: -out.re,
                im: -out.im,
            };
        }
        out
    }
}

impl std::ops::Add for Cplx {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::add(self, o)
    }
}
impl std::ops::Sub for Cplx {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self::sub(self, o)
    }
}
impl std::ops::Mul for Cplx {
    type Output = Self;
    #[inline]
    fn mul(self, o: Self) -> Self {
        Self::mul(self, o)
    }
}

/// A complex 3-vector: `re + i*im`, both real `Vec3`s. Used for the (possibly
/// evanescent) wavevector `k` and the mode field vectors `D`/`E`/`H` it produces.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CVec3 {
    pub re: Vec3,
    pub im: Vec3,
}

impl CVec3 {
    #[inline]
    pub(crate) const fn from_real(v: Vec3) -> Self {
        Self {
            re: v,
            im: Vec3::ZERO,
        }
    }

    /// `(a*that + q*zhat)` with `a` real and `q` complex -- the wavevector construction
    /// every mode field starts from.
    #[inline]
    pub(crate) fn wavevector(that: Vec3, zhat: Vec3, tangential: f32, q: Cplx) -> Self {
        Self {
            re: that * tangential + zhat * q.re,
            im: zhat * q.im,
        }
    }

    #[inline]
    pub(crate) fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }

    #[inline]
    pub(crate) fn scale_real(self, s: f32) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    #[inline]
    pub(crate) fn scale_complex(self, s: Cplx) -> Self {
        Self {
            re: self.re * s.re - self.im * s.im,
            im: self.re * s.im + self.im * s.re,
        }
    }

    /// Cross product with a REAL vector: `(re + i*im) x r = (re x r) + i*(im x r)`.
    #[inline]
    pub(crate) fn cross_real(self, r: Vec3) -> Self {
        Self {
            re: self.re.cross(r),
            im: self.im.cross(r),
        }
    }

    /// Cross product with another complex vector.
    #[inline]
    pub(crate) fn cross(self, o: Self) -> Self {
        Self {
            re: self.re.cross(o.re) - self.im.cross(o.im),
            im: self.re.cross(o.im) + self.im.cross(o.re),
        }
    }

    /// Dot product with a REAL vector -> complex scalar.
    #[inline]
    pub(crate) fn dot_real(self, r: Vec3) -> Cplx {
        Cplx {
            re: self.re.dot(r),
            im: self.im.dot(r),
        }
    }

    #[inline]
    pub(crate) fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }
}

impl std::ops::Add for CVec3 {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self::add(self, o)
    }
}
