# quircs no_std fork

This directory is derived from `quircs` 0.10.3 by Friedel Ziegelmayer and
contributors:

- Upstream: <https://github.com/dignifiedquire/quircs>
- Crate: <https://crates.io/crates/quircs/0.10.3>
- License: MIT (see `LICENSE`)

Local changes replace `std` usage with `core` and `alloc`, use `libm` for
floating-point rounding on bare-metal targets, and configure dependencies
without their default `std` features.

The local `identify_with_luma` entry point accepts a repeatable luminance
callback. Histogram and thresholding read it directly, avoiding an intermediate
grayscale allocation for RGB565 camera frames. The existing slice-based
`identify` API delegates to the same path.
