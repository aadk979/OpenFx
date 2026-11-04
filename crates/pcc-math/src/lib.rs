//! Finite, SI-space vector primitives. Coordinates are right-handed/Y-up metres.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathError {
    NonFinite,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    pub const UP: Self = Self {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    pub fn checked(x: f32, y: f32, z: f32) -> Result<Self, MathError> {
        if x.is_finite() && y.is_finite() && z.is_finite() {
            let canonical = |v: f32| if v == 0.0 { 0.0 } else { v };
            Ok(Self {
                x: canonical(x),
                y: canonical(y),
                z: canonical(z),
            })
        } else {
            Err(MathError::NonFinite)
        }
    }
    pub fn add_checked(self, rhs: Self) -> Result<Self, MathError> {
        Self::checked(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
    pub fn sub_checked(self, rhs: Self) -> Result<Self, MathError> {
        Self::checked(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
    pub fn scale_checked(self, s: f32) -> Result<Self, MathError> {
        Self::checked(self.x * s, self.y * s, self.z * s)
    }
    #[must_use]
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }
    pub fn cross_checked(self, rhs: Self) -> Result<Self, MathError> {
        Self::checked(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }
    #[must_use]
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    #[must_use]
    pub fn normalized_or_up(self) -> Self {
        let len_sq = self.dot(self);
        if len_sq.is_finite() && len_sq > 1e-12 {
            let inv = 1.0 / len_sq.sqrt();
            Self::checked(self.x * inv, self.y * inv, self.z * inv).unwrap_or(Self::UP)
        } else {
            Self::UP
        }
    }
}

/// 10-round Philox-4x32 counter-based PRNG (Salmon et al. 2011).
/// Provides bit-exact, thread-schedule-independent pseudorandom numbers
/// indexed by `(node_seed, element_id, frame, substep)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Philox4x32 {
    key: [u32; 2],
}

impl Philox4x32 {
    const M0: u64 = 0xD251_1F53;
    const M1: u64 = 0xCD9E_8D57;
    const W0: u32 = 0x9E37_79B9;
    const W1: u32 = 0xBB67_AE85;

    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            key: [seed as u32, (seed >> 32) as u32],
        }
    }

    #[must_use]
    pub const fn generate(&self, counter: [u32; 4]) -> [u32; 4] {
        let mut ctr = counter;
        let mut key = self.key;
        let mut round = 0;
        while round < 10 {
            let prod0 = (ctr[0] as u64).wrapping_mul(Self::M0);
            let prod1 = (ctr[2] as u64).wrapping_mul(Self::M1);
            let hi0 = (prod0 >> 32) as u32;
            let lo0 = prod0 as u32;
            let hi1 = (prod1 >> 32) as u32;
            let lo1 = prod1 as u32;
            ctr = [hi1 ^ ctr[1] ^ key[0], lo1, hi0 ^ ctr[3] ^ key[1], lo0];
            key[0] = key[0].wrapping_add(Self::W0);
            key[1] = key[1].wrapping_add(Self::W1);
            round += 1;
        }
        ctr
    }

    /// Generates four deterministic uniform floats in `[0.0, 1.0)`.
    #[must_use]
    pub fn generate_unit_f32(&self, counter: [u32; 4]) -> [f32; 4] {
        let bits = self.generate(counter);
        const SCALE: f32 = 1.0 / ((1u32 << 24) as f32);
        [
            ((bits[0] >> 8) as f32) * SCALE,
            ((bits[1] >> 8) as f32) * SCALE,
            ((bits[2] >> 8) as f32) * SCALE,
            ((bits[3] >> 8) as f32) * SCALE,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_nan() {
        assert_eq!(Vec3::checked(f32::NAN, 0.0, 0.0), Err(MathError::NonFinite));
    }

    #[test]
    fn philox4x32_is_deterministic_and_within_unit_interval() {
        let rng = Philox4x32::new(0x1234_5678_9ABC_DEF0);
        let a = rng.generate([1, 2, 3, 4]);
        let b = rng.generate([1, 2, 3, 4]);
        let c = rng.generate([1, 2, 3, 5]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let floats = rng.generate_unit_f32([42, 0, 100, 0]);
        for f in floats {
            assert!(f.is_finite() && (0.0..1.0).contains(&f));
        }
    }
}
