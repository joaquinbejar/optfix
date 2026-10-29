//! The error type shared by every optfix function.

use core::fmt;

/// Why an optfix operation failed.
///
/// `#[repr(u8)]` with stable discriminants, so an on-chain program can map it into its own
/// error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PricingError {
    /// An intermediate or final result does not fit the representation.
    Overflow = 0,
    /// Input outside the function's domain: the logarithm of a non-positive number, a zero
    /// divisor, a negative time, or a quantity constructor's range check.
    DomainError = 1,
    /// Implied volatility did not reach the tolerance within the iteration cap.
    NoConvergence = 2,
    /// Target price outside the no-arbitrage bounds, so no implied volatility exists.
    ArbitrageBound = 3,
}

impl fmt::Display for PricingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Overflow => "result does not fit the fixed-point representation",
            Self::DomainError => "input outside the function's domain",
            Self::NoConvergence => "implied volatility did not converge",
            Self::ArbitrageBound => "target price outside the no-arbitrage bounds",
        };
        f.write_str(text)
    }
}

impl core::error::Error for PricingError {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    extern crate std;

    use super::PricingError;
    use std::string::ToString;

    #[test]
    fn test_pricing_error_discriminants_are_stable() {
        assert_eq!(PricingError::Overflow as u8, 0);
        assert_eq!(PricingError::DomainError as u8, 1);
        assert_eq!(PricingError::NoConvergence as u8, 2);
        assert_eq!(PricingError::ArbitrageBound as u8, 3);
    }

    #[test]
    fn test_pricing_error_display_is_not_empty() {
        for e in [
            PricingError::Overflow,
            PricingError::DomainError,
            PricingError::NoConvergence,
            PricingError::ArbitrageBound,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }
}
