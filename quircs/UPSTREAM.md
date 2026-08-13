# quircs no_std fork

This directory is derived from `quircs` 0.10.3 by Friedel Ziegelmayer and
contributors:

- Upstream: <https://github.com/dignifiedquire/quircs>
- Crate: <https://crates.io/crates/quircs/0.10.3>
- License: MIT (see `LICENSE`)

Local changes replace `std` usage with `core` and `alloc`, use `libm` for
floating-point rounding on bare-metal targets, and configure dependencies
without their default `std` features.

