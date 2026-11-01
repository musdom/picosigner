# Dependency review — 2026-11-01

Reviewed the workspace manifests, locked registry packages, published crate
sources, and the RustSec advisory database at `2295466c4e9df26647fd67d8fa1c705ac2fc7f81`. The updates focus on fixes affecting wallet keys and
untrusted QR input.

## Updates

| Dependency | Before | After | Reason |
| --- | --- | --- | --- |
| `ur` | 0.4.1 | 0.5.2 | Verify reconstructed fountain-message checksums; reject sequence zero and malformed bytewords; validate UR types and indices. |
| `minicbor` | 0.19.1 | 2.3.1 | Match the CBOR traits used by `ur` 0.5, avoiding incompatible `Decode` implementations and duplicate CBOR versions. |
| `miniscript` | 12.3.7 | 12.3.8 | Match the descriptor path before returning a BIP32 key, reject sibling paths, and strip the key origin before deriving from an account xprv. |
| `bdk_wallet` | 3.1.0 | 3.2.0 | Includes ChangeSet write-once invariant and BIP431 rule 2 fixes. Used only by host helpers. |

Sources: [UR changelog](https://docs.rs/crate/ur/0.5.2/source/CHANGELOG.md),
[Miniscript key lookup implementation and regressions](https://docs.rs/crate/miniscript/12.3.8/source/src/descriptor/key_map.rs),
[BDK changelog](https://docs.rs/crate/bdk_wallet/3.2.0/source/CHANGELOG.md).

PicoSigner uses `ur::fountain::Decoder` directly so it can bound message size
and fragment count before allocation. Consequently the index-to-CBOR checks in
`ur::Decoder::receive` do not automatically apply. The collector now enforces
those checks itself before accepting a frame. Its existing crypto-psbt-only
header check also prevents mixing UR types.

Miniscript stays on the patched 12.x series used by BDK. The firmware uses its
checksum API; the new host regression directly exercises the corrected key
lookup. Bitcoin 0.32.102, schnorr_fun 0.13.0, secp256kfun 0.12.1,
and rp235x-hal 0.4.0 were already current stable releases at review time.

## Audit and remaining maintenance work

`cargo audit` reports **zero known vulnerabilities and zero yanked-package
warnings**, both before and after the update. These bug fixes are not presented
as RustSec findings. Six existing unmaintained-package warnings remain:

- `bare-metal` 0.2.5: through cortex-m and the current RP235x HAL/PAC.
- `paste` 1.0.15 and `proc-macro-error2` 2.0.1: through PIO/HAL build dependencies.
  The latter also produces a future Rust compatibility warning.
- `rusttype` 0.9.3 and `ttf-parser` 0.15.2: the current font renderer and its parser.
- `bincode` 2.0.1: a locked optional dependency; it is absent from the active
  workspace dependency tree even with all workspace features enabled.

Removing these warnings needs upstream changes or a font-rendering migration;
there is no patched version to select for these advisories.

## Validation

- `just test` passed: formatting, required Clippy gates, 138 host/integration
  tests (including golden images), and checks of firmware and board examples.
- New regressions cover forged URI indices without advancing collection,
  corrupted fountain data with a valid outer bytewords CRC, origin-relative key
  derivation, and refusal to return a key for a sibling descriptor path.
- `just size`: release text grew from 838,964 to 839,084 bytes (+120);
  data stayed at 4 bytes and BSS at 153,636 bytes.
- Hardware scanning and timing have not been tested. Static size does not
  measure peak heap use or physical camera performance.
