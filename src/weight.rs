//! Converting between fontconfig's weight scale and OpenType's.
//!
//! Fontconfig numbers weights 0..215 with uneven spacing, and OpenType uses
//! 0..1000. The mapping is a piecewise-linear interpolation through a table of
//! anchor points, not a ratio -- `FC_WEIGHT_REGULAR` is 80 against OpenType's
//! 400, but `FC_WEIGHT_BOLD` is 200 against 700.

/// Anchor points, OpenType against fontconfig, from `map` in `fcweight.c`.
///
/// Note the first two both sit at fontconfig 0: an OpenType weight of 0 and of
/// 100 are both "thin", which is why the reverse direction starts its search
/// at index 1.
const MAP: [(f64, f64); 13] = [
    (0.0, 0.0),      // thin
    (100.0, 0.0),    // thin
    (200.0, 40.0),   // extralight
    (300.0, 50.0),   // light
    (350.0, 55.0),   // demilight
    (380.0, 75.0),   // book
    (400.0, 80.0),   // regular
    (500.0, 100.0),  // medium
    (600.0, 180.0),  // demibold
    (700.0, 200.0),  // bold
    (800.0, 205.0),  // extrabold
    (900.0, 210.0),  // black
    (1000.0, 215.0), // extrablack
];

fn lerp(x: f64, x1: f64, x2: f64, y1: f64, y2: f64) -> f64 {
    if x2 == x1 {
        return y1;
    }
    y1 + (x - x1) * (y2 - y1) / (x2 - x1)
}

/// A fontconfig weight as an OpenType one, or `-1.0` if out of range.
pub fn to_opentype(weight: f64) -> f64 {
    if !(0.0..=215.0).contains(&weight) {
        return -1.0;
    }
    let mut i = 1;
    while i < MAP.len() - 1 && weight > MAP[i].1 {
        i += 1;
    }
    if weight == MAP[i].1 {
        return MAP[i].0;
    }
    lerp(weight, MAP[i - 1].1, MAP[i].1, MAP[i - 1].0, MAP[i].0)
}

/// An OpenType weight as a fontconfig one, or `-1.0` if negative.
pub fn from_opentype(weight: f64) -> f64 {
    if weight < 0.0 {
        return -1.0;
    }
    let weight = weight.min(MAP[MAP.len() - 1].0);
    let mut i = 1;
    while i < MAP.len() - 1 && weight > MAP[i].0 {
        i += 1;
    }
    if weight == MAP[i].0 {
        return MAP[i].1;
    }
    lerp(weight, MAP[i - 1].0, MAP[i].0, MAP[i - 1].1, MAP[i].1)
}

#[cfg(test)]
mod tests {
    use super::{from_opentype, to_opentype};

    #[test]
    fn the_named_weights_round_trip() {
        for (ot, fc) in [
            (100.0, 0.0),
            (200.0, 40.0),
            (300.0, 50.0),
            (400.0, 80.0),
            (500.0, 100.0),
            (700.0, 200.0),
            (900.0, 210.0),
        ] {
            assert_eq!(from_opentype(ot), fc, "ot {ot} -> fc");
            assert_eq!(to_opentype(fc), ot, "fc {fc} -> ot");
        }
    }

    /// The scale is not linear: regular sits at 80 of 215 but 400 of 1000, so
    /// a proportional conversion would be wrong nearly everywhere.
    #[test]
    fn the_mapping_is_piecewise_not_proportional() {
        assert_ne!(to_opentype(80.0), 80.0 / 215.0 * 1000.0);
        // Between anchors it interpolates: fc 90 is halfway from regular (80,
        // 400) to medium (100, 500).
        assert_eq!(to_opentype(90.0), 450.0);
        assert_eq!(from_opentype(450.0), 90.0);
    }

    #[test]
    fn out_of_range_weights_are_rejected() {
        assert_eq!(to_opentype(-1.0), -1.0);
        assert_eq!(to_opentype(216.0), -1.0);
        assert_eq!(from_opentype(-1.0), -1.0);
        // Above the top anchor it clamps rather than failing.
        assert_eq!(from_opentype(2000.0), 215.0);
    }

    #[test]
    fn conversion_is_monotonic() {
        let mut previous = -1.0;
        for step in 0..=215 {
            let ot = to_opentype(f64::from(step));
            assert!(ot >= previous, "fc {step} gave {ot} after {previous}");
            previous = ot;
        }
    }
}
