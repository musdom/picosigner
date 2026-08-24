# PicoSigner assets

## UI font

`subset-font.ttf` is embedded directly into the firmware by
`picosigner/src/ui.rs`. It contains printable ASCII (`U+0020` through
`U+007E`). This covers PicoSigner's static UI strings and its dynamic BIP39
words, hexadecimal fingerprints, derivation paths, transaction amounts, and
error details. The current source font is `Quicksand-Regular.ttf`.

The subset retains TrueType hinting for the 14 px and 20 px UI sizes, along
with the font's names and license metadata. OpenType layout features are
disabled so optional ligatures do not pull extra glyphs into the firmware.

### Requirements

- Bash
- Python 3
- [FontTools](https://fonttools.readthedocs.io/), which supplies `pyftsubset`

Install FontTools if needed:

```sh
python3 -m pip install --user fonttools
```

### Regenerate the checked-in font

From the repository root, run:

```sh
./picosigner/assets/subset-font.sh picosigner/assets/Quicksand-Regular.ttf
```

The default output is `picosigner/assets/subset-font.ttf`. To inspect the
result without replacing the checked-in asset, pass a second path:

```sh
./picosigner/assets/subset-font.sh \
  picosigner/assets/Quicksand-Regular.ttf \
  /tmp/subset-font.ttf
```

FontTools output can vary slightly between releases. For reproducible binary
output, use the same FontTools version; the current asset was produced with
the 4.x series. Independently of byte-for-byte differences, confirm that the
result only exposes printable ASCII:

```sh
fc-query --format '%{charset}\n' picosigner/assets/subset-font.ttf
```

The expected output is `20-7e`. Then validate both host rendering and the
embedded build:

```sh
cargo test -p picosigner-emulator --target x86_64-unknown-linux-gnu
cargo check -p picosigner --target thumbv8m.main-none-eabihf
```

Quicksand is Copyright 2011 The Quicksand Project Authors and is licensed
under the SIL Open Font License 1.1. The upstream font and generated subset
contain the complete license metadata.

## Bitcoin Icons

The outline glyphs used by the menu renderer are adapted from
[Bitcoin Icons](https://bitcoinicons.com/). Attribution and the MIT license are
recorded in [`bitcoin-icons/README.md`](bitcoin-icons/README.md).
