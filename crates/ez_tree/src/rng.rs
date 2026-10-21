//! ez-tree's seeded random number generator (`rng.js`), a multiply-with-carry
//! generator, with JavaScript's integer semantics reproduced exactly.

/// Seeded RNG matching ez-tree's `RNG` class bit for bit.
///
/// The JavaScript source relies on implicit int32/uint32 conversions:
/// `& 0xffffffff` yields a *signed* 32-bit value (ToInt32), `>> 16` is an
/// arithmetic shift of that signed value, `<< 16` wraps to int32, and
/// `>>> 0` reinterprets as uint32. All intermediate sums fit exactly in an
/// `f64`/`i64`, so integer arithmetic here gives identical results.
#[derive(Clone, Debug)]
pub struct Rng {
    m_w: i32,
    m_z: i32,
}

/// JavaScript ToInt32 for integral values that fit in an `i64`.
#[inline]
fn to_int32(v: i64) -> i32 {
    v as i32 // keeps the low 32 bits, i.e. wraps modulo 2^32
}

impl Rng {
    /// `new RNG(seed)`.
    pub fn new(seed: i64) -> Self {
        Self {
            m_w: to_int32(123_456_789_i64.wrapping_add(seed)),
            m_z: to_int32(987_654_321_i64.wrapping_sub(seed)),
        }
    }

    /// `random()`: a value in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        let m_z = i64::from(self.m_z);
        let m_w = i64::from(self.m_w);
        self.m_z = to_int32(36969 * (m_z & 65535) + (m_z >> 16));
        self.m_w = to_int32(18000 * (m_w & 65535) + (m_w >> 16));
        // (m_z << 16) wraps to int32 before the (unwrapped) addition.
        let high = i64::from(self.m_z.wrapping_shl(16));
        let low = i64::from(self.m_w) & 65535;
        let result = (high + low) as u32; // `>>> 0`
        f64::from(result) / 4_294_967_296.0
    }

    /// `random(max, min)`: `(max - min) * random() + min`.
    pub fn random(&mut self, max: f64, min: f64) -> f64 {
        let r = self.next_f64();
        (max - min) * r + min
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Values printed by Node running ez-tree's rng.js.
    #[test]
    fn matches_node_seed_0() {
        let mut rng = Rng::new(0);
        assert_eq!(rng.next_f64(), 0.7322976540308446);
        assert_eq!(rng.next_f64(), 0.05806277762167156);
        assert_eq!(rng.next_f64(), 0.8218648040201515);
    }

    #[test]
    fn matches_node_preset_seed() {
        let mut rng = Rng::new(29919);
        assert_eq!(rng.next_f64(), 0.35936520155519247);
        assert_eq!(rng.next_f64(), 0.7075402059126645);
        assert_eq!(rng.next_f64(), 0.05636478797532618);
    }

    #[test]
    fn matches_node_negative_and_wrapping_seeds() {
        let mut rng = Rng::new(-5);
        assert_eq!(rng.next_f64(), 0.5528028223197907);
        let mut rng = Rng::new(3_000_000_000);
        assert_eq!(rng.next_f64(), 0.47912494745105505);
        assert_eq!(rng.next_f64(), 0.2676705466583371);
        let mut rng = Rng::new(-3_000_000_000);
        assert_eq!(rng.next_f64(), 0.9854703608434647);
        let mut rng = Rng::new(2_147_483_647);
        assert_eq!(rng.next_f64(), 0.79638800653629);
    }
}
