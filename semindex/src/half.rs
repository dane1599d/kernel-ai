//! IEEE 754 half precision, as the index keeps a vector (ADR 0013 section 4: 256
//! dimensions as f16, 512 bytes a chunk). Narrowing rounds to nearest, ties to
//! even, as the engine's f16 KV cache does; widening is exact.

/// `x` as f16 bits: rounded to nearest, ties to even; too large is infinity,
/// too small is a signed zero, NaN stays NaN.
pub fn narrow(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let man = bits & 0x007f_ffff;
    if exp == 0xff {
        // Infinity, or a quiet NaN.
        return sign | 0x7c00 | if man != 0 { 0x0200 } else { 0 };
    }
    // The exponent as f16's, biased by 15.
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // A subnormal, or zero: the implicit bit joins the mantissa, shifted
        // down by how far below f16's least normal exponent it is.
        if e < -10 {
            return sign;
        }
        let man = man | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = man >> shift;
        let rest = man & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let up = rest > halfway || (rest == halfway && half & 1 == 1);
        return sign | (half + u32::from(up)) as u16;
    }
    let half = ((e as u32) << 10) | (man >> 13);
    let rest = man & 0x1fff;
    let up = rest > 0x1000 || (rest == 0x1000 && half & 1 == 1);
    // A carry out of the mantissa raises the exponent, up to infinity.
    sign | (half + u32::from(up)) as u16
}

/// f16 bits `h` as f32, exactly.
pub fn widen(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1f);
    let man = u32::from(h & 0x03ff);
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, _) => {
            // A subnormal: normalise it.
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x0400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x03ff) << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, _) => sign | 0x7fc0_0000 | (man << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}
