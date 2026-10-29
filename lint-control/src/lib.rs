//! Failing control for the workspace lint set.
//!
//! Normal builds compile nothing here. `make lint-control` sets `--cfg lint_control` and
//! requires clippy to reject every macro below in library code, so a change to `clippy.toml`
//! or `[workspace.lints]` that silently stops enforcing the panic ban fails `make pre-push`.

#![no_std]

#[cfg(lint_control)]
extern crate std;

/// Each panicking macro the rules ban, in a public library function.
#[cfg(lint_control)]
pub fn banned_in_core(x: u8) -> u8 {
    assert!(x > 0);
    assert_eq!(x, 1);
    assert_ne!(x, 2);
    debug_assert!(x > 0);
    debug_assert_eq!(x, 1);
    debug_assert_ne!(x, 2);
    if x == 3 {
        panic!("banned");
    }
    if x == 4 {
        unreachable!();
    }
    x
}

/// The same macros through their `std` paths.
#[cfg(lint_control)]
pub fn banned_in_std(x: u8) -> u8 {
    std::assert!(x > 0);
    std::debug_assert!(x > 0);
    if x == 3 {
        std::panic!("banned");
    }
    if x == 4 {
        std::unreachable!();
    }
    x
}
