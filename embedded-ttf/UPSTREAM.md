# Upstream provenance

This directory is derived from `embedded-ttf` 0.2.2, published from
<https://github.com/peckpeck/embedded-ttf> under the MIT license.

PicoSigner changes:

- Clamp `rusttype` rasterizer coverage to `0.0..=1.0` before converting it to
  an 8-bit alpha value.
- Blend RGB channels with `u32` intermediates so debug builds cannot overflow
  while adding the foreground and background contributions.

The local package version is `0.2.2-picosigner.1`.
