//! Quantity newtypes: each wraps a [`Fixed`] and proves its own range.
//!
//! The ranges are the supported domain of `docs/PLAN.md` (section "Supported domain and error
//! envelope"), which is also exactly what the tests cover. Bounds that Q64.64 cannot hold
//! exactly (`10^-6`, `0.005`, `−0.1`) are rounded outward to the next representable value, so
//! either rounding of a boundary constant is accepted; the domain grows by under one ulp. There are no conversions between
//! the newtypes, so the compiler rejects a `Volatility` passed as a `Strike`.

use crate::{Fixed, PricingError, Rounding};

/// Seconds in the 365-day year that [`Years::from_seconds`] uses.
pub const SECONDS_PER_YEAR: i64 = 31_536_000;

/// Defines a newtype over `Fixed` whose constructor checks `MIN <= value <= MAX`.
macro_rules! bounded_quantity {
    (
        $(#[$doc:meta])*
        $name:ident, min_raw: $min:expr, max_raw: $max:expr, range: $range:literal
    ) => {
        $(#[$doc])*
        #[doc = ""]
        #[doc = concat!("Range: ", $range, ".")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(transparent)]
        pub struct $name(Fixed);

        impl $name {
            /// The smallest accepted value.
            pub const MIN: Self = Self(Fixed::from_raw($min));
            /// The largest accepted value.
            pub const MAX: Self = Self(Fixed::from_raw($max));

            /// Checks the range and wraps `value`. Values read from storage must come back
            /// through this constructor, never a raw cast.
            ///
            /// # Errors
            ///
            #[doc = concat!("`DomainError` if `value` is outside ", $range, ".")]
            #[must_use = "the result carries a possible error"]
            #[inline]
            pub fn try_new(value: Fixed) -> Result<Self, PricingError> {
                if value < Self::MIN.0 || value > Self::MAX.0 {
                    return Err(PricingError::DomainError);
                }
                Ok(Self(value))
            }

            /// The wrapped value.
            #[must_use]
            #[inline]
            pub const fn get(self) -> Fixed {
                self.0
            }
        }
    };
}

bounded_quantity!(
    /// Price of the underlying, in quote units.
    Spot,
    // floor(10^-6 × 2^64), 10^9 × 2^64
    min_raw: 18_446_744_073_709,
    max_raw: 18_446_744_073_709_551_616_000_000_000,
    range: "`[10^-6, 10^9]`"
);

bounded_quantity!(
    /// Strike price, in the same unit as [`Spot`]. The two-input constraint
    /// `S/K in [0.05, 20]` is checked by the pricing functions, not here.
    Strike,
    // floor(10^-6 × 2^64), 10^9 × 2^64
    min_raw: 18_446_744_073_709,
    max_raw: 18_446_744_073_709_551_616_000_000_000,
    range: "`[10^-6, 10^9]`"
);

bounded_quantity!(
    /// Option premium, in the same unit as [`Spot`].
    Price,
    min_raw: 0,
    max_raw: i128::MAX,
    range: "`[0, Fixed::MAX]`"
);

bounded_quantity!(
    /// Annualized volatility sigma (`0.20` is 20%).
    Volatility,
    // floor(0.005 × 2^64), 5 × 2^64
    min_raw: 92_233_720_368_547_758,
    max_raw: 92_233_720_368_547_758_080,
    range: "`[0.005, 5]`"
);

bounded_quantity!(
    /// Continuously compounded annual rate: the risk-free rate, or the carry (continuous
    /// yield `q`).
    Rate,
    // floor(−0.1 × 2^64), 0.5 × 2^64
    min_raw: -1_844_674_407_370_955_162,
    max_raw: 9_223_372_036_854_775_808,
    range: "`[-0.1, 0.5]`"
);

bounded_quantity!(
    /// Time to expiry in years: time remaining, not an absolute timestamp.
    Years,
    // 30 × 2^64
    min_raw: 0,
    max_raw: 553_402_322_211_286_548_480,
    range: "`[0, 30]`"
);

impl Years {
    /// `seconds / 31_536_000` (365-day year), rounded in direction `r`.
    ///
    /// The library does not know which side is conservative: rounding down understates time
    /// value, which is not conservative for a writer's margin, so the caller chooses.
    ///
    /// # Errors
    ///
    /// `DomainError` if `seconds < 0` (expired options are priced by intrinsic value, not by
    /// Black-Scholes) or if the result exceeds 30 years.
    #[must_use = "the result carries a possible error"]
    pub fn from_seconds(seconds: i64, r: Rounding) -> Result<Self, PricingError> {
        if seconds < 0 {
            return Err(PricingError::DomainError);
        }
        let years = Fixed::from_ratio(i128::from(seconds), i128::from(SECONDS_PER_YEAR), r)?;
        Self::try_new(years)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]
mod tests {
    use super::{Price, Rate, Spot, Strike, Volatility, Years, SECONDS_PER_YEAR};
    use crate::{Fixed, PricingError, Rounding};

    /// `num / den` rounded down: the outward rounding of a lower bound.
    fn ratio(num: i128, den: i128) -> Fixed {
        Fixed::from_ratio(num, den, Rounding::Down).unwrap()
    }

    #[test]
    fn test_bounds_match_domain_table() {
        assert_eq!(Spot::MIN.get(), ratio(1, 1_000_000));
        assert_eq!(Spot::MAX.get(), Fixed::from_int(1_000_000_000));
        assert_eq!(Strike::MIN.get(), Spot::MIN.get());
        assert_eq!(Strike::MAX.get(), Spot::MAX.get());
        assert_eq!(Volatility::MIN.get(), ratio(5, 1000));
        assert_eq!(Volatility::MAX.get(), Fixed::from_int(5));
        assert_eq!(Rate::MIN.get(), ratio(-1, 10));
        assert_eq!(Rate::MAX.get(), ratio(1, 2));
        assert_eq!(Years::MIN.get(), Fixed::ZERO);
        assert_eq!(Years::MAX.get(), Fixed::from_int(30));
        assert_eq!(Price::MIN.get(), Fixed::ZERO);
        assert_eq!(Price::MAX.get(), Fixed::MAX);
    }

    #[test]
    fn test_try_new_accepts_bounds_rejects_one_ulp_outside() {
        let ulp = Fixed::from_raw(1);
        macro_rules! check {
            ($t:ident) => {
                assert_eq!($t::try_new($t::MIN.get()).unwrap(), $t::MIN);
                assert_eq!($t::try_new($t::MAX.get()).unwrap(), $t::MAX);
                if let Ok(below) = $t::MIN.get().checked_sub(ulp) {
                    assert_eq!($t::try_new(below), Err(PricingError::DomainError));
                }
                if let Ok(above) = $t::MAX.get().checked_add(ulp) {
                    assert_eq!($t::try_new(above), Err(PricingError::DomainError));
                }
            };
        }
        check!(Spot);
        check!(Strike);
        check!(Price);
        check!(Volatility);
        check!(Rate);
        check!(Years);
    }

    #[test]
    fn test_years_from_seconds_one_year_is_one() {
        assert_eq!(
            Years::from_seconds(SECONDS_PER_YEAR, Rounding::Down)
                .unwrap()
                .get(),
            Fixed::ONE
        );
        assert_eq!(Years::from_seconds(0, Rounding::Up).unwrap(), Years::MIN);
    }

    #[test]
    fn test_years_from_seconds_rounding_brackets_one_second() {
        let down = Years::from_seconds(1, Rounding::Down).unwrap().get();
        let up = Years::from_seconds(1, Rounding::Up).unwrap().get();
        // 2^64 / 31_536_000 = 584942417355.072...
        assert_eq!(down.raw(), 584_942_417_355);
        assert_eq!(up.raw(), 584_942_417_356);
    }

    #[test]
    fn test_years_from_seconds_negative_is_domain_error() {
        assert_eq!(
            Years::from_seconds(-1, Rounding::Down),
            Err(PricingError::DomainError)
        );
    }

    #[test]
    fn test_years_from_seconds_above_thirty_years_is_domain_error() {
        let thirty = 30 * SECONDS_PER_YEAR;
        assert!(Years::from_seconds(thirty, Rounding::Up).is_ok());
        assert_eq!(
            Years::from_seconds(thirty + 1, Rounding::Down),
            Err(PricingError::DomainError)
        );
        assert_eq!(
            Years::from_seconds(i64::MAX, Rounding::Down),
            Err(PricingError::DomainError)
        );
    }
}
