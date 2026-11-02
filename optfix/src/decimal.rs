//! Exact decimal text for [`Fixed`] (`docs/PLAN-improvements.md`, milestone 17): parsing with an
//! explicit rounding, and printing the exact value. No allocation.
//!
//! A Q64.64 value is a multiple of `2^-64`, so its decimal expansion ends after at most 64
//! fractional digits and printing is exact. Parsing reads the integer part, the first 64
//! fractional digits exactly and only whether any later digit is nonzero: for a fraction `x`
//! cut to `x_64` after 64 digits, `2^64 x_64` has a fractional part that is a multiple of
//! `2^64 / 10^64` (its numerator `D × 2^64 mod 10^64` is a multiple of `2^64`) and so at most
//! `1 − 2^64 / 10^64`, while the cut adds less than `2^64 / 10^64`: the floor of `2^64 x` is
//! the floor of `2^64 x_64`, and only the exactness depends on the rest.

use core::fmt;

use crate::{Fixed, PricingError, Rounding};

/// Fractional digits read exactly; any later nonzero digit only makes the value inexact.
const EXACT_DIGITS: usize = 64;

impl Fixed {
    /// Parses a decimal number, rounded in direction `r`: an optional sign (`+` or `-`), one or
    /// more digits, and optionally a `.` followed by one or more digits. No exponent,
    /// separators or whitespace; any number of fractional digits.
    ///
    /// # Errors
    ///
    /// `DomainError` if `s` is not of that form; `Overflow` if the rounded value does not fit.
    pub fn from_decimal_str(s: &str, r: Rounding) -> Result<Self, PricingError> {
        let bytes = s.as_bytes();
        let (negative, rest) = match bytes.split_first() {
            Some((b'-', rest)) => (true, rest),
            Some((b'+', rest)) => (false, rest),
            _ => (false, bytes),
        };
        let (int_digits, frac_digits) = match rest.iter().position(|&c| c == b'.') {
            Some(dot) => {
                let (int_digits, with_dot) = rest.split_at(dot);
                (
                    int_digits,
                    with_dot.get(1..).ok_or(PricingError::DomainError)?,
                )
            }
            None => (rest, &[][..]),
        };
        let has_dot = int_digits.len() != rest.len();
        // The whole text is checked before any value: malformed text is a `DomainError` even
        // where its digits would also overflow.
        if int_digits.is_empty()
            || (has_dot && frac_digits.is_empty())
            || !int_digits.iter().chain(frac_digits).all(u8::is_ascii_digit)
        {
            return Err(PricingError::DomainError);
        }

        // Integer part: below 2^64, or the value cannot fit.
        let mut int: u64 = 0;
        for &c in int_digits {
            let d = digit(c)?;
            int = int
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(d)))
                .ok_or(PricingError::Overflow)?;
        }

        // Fraction: the first EXACT_DIGITS digits, doubled 64 times; each carry out of the
        // integer position is the next binary digit.
        let mut buf = [0u8; EXACT_DIGITS];
        let mut inexact = false;
        for (i, &c) in frac_digits.iter().enumerate() {
            let d = digit(c)?;
            match buf.get_mut(i) {
                Some(slot) => *slot = d,
                None => inexact |= d != 0,
            }
        }
        let mut frac: u64 = 0;
        for _ in 0..64 {
            let mut carry = 0u8;
            for slot in buf.iter_mut().rev() {
                // slot <= 9, so 2 slot + carry <= 19.
                let v = slot.wrapping_mul(2).wrapping_add(carry);
                carry = u8::from(v >= 10);
                *slot = if v >= 10 { v.wrapping_sub(10) } else { v };
            }
            frac = (frac << 1) | u64::from(carry);
        }
        inexact |= buf.iter().any(|&d| d != 0);

        let magnitude = (u128::from(int) << 64) | u128::from(frac);
        // Round the magnitude away from zero when the signed result must move that way.
        let away = inexact && (negative == (r == Rounding::Down));
        let magnitude = if away {
            magnitude.checked_add(1).ok_or(PricingError::Overflow)?
        } else {
            magnitude
        };
        let raw = if negative {
            0i128
                .checked_sub_unsigned(magnitude)
                .ok_or(PricingError::Overflow)?
        } else {
            i128::try_from(magnitude).map_err(|_| PricingError::Overflow)?
        };
        Ok(Self::from_raw(raw))
    }

    /// Writes the exact decimal value: a `-` for a negative value, the integer part, and, if
    /// the value is not an integer, a `.` and every fractional digit up to the last nonzero one
    /// (at most 64). [`fmt::Display`] prints the same.
    ///
    /// # Errors
    ///
    /// Only the writer's.
    pub fn write_decimal<W: fmt::Write>(&self, w: &mut W) -> fmt::Result {
        let raw = self.raw();
        if raw < 0 {
            w.write_char('-')?;
        }
        let magnitude = raw.unsigned_abs();
        let int = u64::try_from(magnitude >> 64).map_err(|_| fmt::Error)?;
        let mut frac = u64::try_from(magnitude & u128::from(u64::MAX)).map_err(|_| fmt::Error)?;
        write_u64(w, int)?;
        if frac != 0 {
            w.write_char('.')?;
            while frac != 0 {
                // frac × 10 < 10 × 2^64: the high word is the next digit.
                let v = u128::from(frac).wrapping_mul(10);
                let d = u8::try_from(v >> 64).map_err(|_| fmt::Error)?;
                w.write_char(char::from(b'0'.wrapping_add(d)))?;
                frac = u64::try_from(v & u128::from(u64::MAX)).map_err(|_| fmt::Error)?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write_decimal(f)
    }
}

/// The value of an ASCII digit.
fn digit(c: u8) -> Result<u8, PricingError> {
    if c.is_ascii_digit() {
        Ok(c.wrapping_sub(b'0'))
    } else {
        Err(PricingError::DomainError)
    }
}

/// Writes `n` in decimal, without allocation.
fn write_u64<W: fmt::Write>(w: &mut W, n: u64) -> fmt::Result {
    let mut buf = [0u8; 20];
    let mut start = buf.len();
    let mut n = n;
    loop {
        let d = u8::try_from(n.checked_rem(10).ok_or(fmt::Error)?).map_err(|_| fmt::Error)?;
        start = start.checked_sub(1).ok_or(fmt::Error)?;
        *buf.get_mut(start).ok_or(fmt::Error)? = b'0'.wrapping_add(d);
        n = n.checked_div(10).ok_or(fmt::Error)?;
        if n == 0 {
            break;
        }
    }
    let text = buf.get(start..).ok_or(fmt::Error)?;
    w.write_str(core::str::from_utf8(text).map_err(|_| fmt::Error)?)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::disallowed_macros,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    fn parse(s: &str, r: Rounding) -> Result<i128, PricingError> {
        Fixed::from_decimal_str(s, r).map(Fixed::raw)
    }

    fn show(raw: i128) -> std::string::String {
        std::format!("{}", Fixed::from_raw(raw))
    }

    #[test]
    fn test_exact_values() {
        for (s, raw) in [
            ("0", 0),
            ("-0", 0),
            ("+1", 1i128 << 64),
            ("1.5", 3i128 << 63),
            ("-2.25", -(9i128 << 62)),
            ("0.0000000000000000000542101086242752217003726400434970855712890625", 1),
            ("9223372036854775807.9999999999999999999457898913757247782996273599565029144287109375", i128::MAX),
            ("-9223372036854775808", i128::MIN),
        ] {
            assert_eq!(parse(s, Rounding::Down), Ok(raw), "{s}");
            assert_eq!(parse(s, Rounding::Up), Ok(raw), "{s}");
            if s != "-0" && !s.starts_with('+') {
                assert_eq!(show(raw), s);
            }
        }
    }

    #[test]
    fn test_rounding_directions() {
        // 0.1 is not a multiple of 2^-64.
        let down = parse("0.1", Rounding::Down).unwrap();
        assert_eq!(parse("0.1", Rounding::Up), Ok(down + 1));
        assert_eq!(parse("-0.1", Rounding::Down), Ok(-down - 1));
        assert_eq!(parse("-0.1", Rounding::Up), Ok(-down));
        // A nonzero digit past the 64th only makes it inexact.
        let tail = std::format!("1.{}1", "0".repeat(70));
        assert_eq!(parse(&tail, Rounding::Down), Ok(1i128 << 64));
        assert_eq!(parse(&tail, Rounding::Up), Ok((1i128 << 64) + 1));
        let zeros = std::format!("1.{}", "0".repeat(200));
        assert_eq!(parse(&zeros, Rounding::Up), Ok(1i128 << 64));
    }

    #[test]
    fn test_errors() {
        for s in [
            "", "-", "+", ".", "1.", ".5", "1e3", " 1", "1 ", "1_0", "--1", "0x10", "1.2.3",
        ] {
            assert_eq!(
                parse(s, Rounding::Down),
                Err(PricingError::DomainError),
                "{s:?}"
            );
        }
        for s in [
            "9223372036854775808",
            "-9223372036854775808.0000000000000000000000000001",
            "18446744073709551616",
        ] {
            assert_eq!(
                parse(s, Rounding::Down),
                Err(PricingError::Overflow),
                "{s:?}"
            );
        }
        // Rounds up to 2^63: does not fit.
        let s = std::format!("9223372036854775807.{}", "9".repeat(80));
        assert_eq!(parse(&s, Rounding::Down), Ok(i128::MAX));
        assert_eq!(parse(&s, Rounding::Up), Err(PricingError::Overflow));
    }
}
