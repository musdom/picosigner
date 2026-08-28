# picosigner

Bare-metal `no_std` Rust firmware turning an RP2350 board into a Bitcoin
hardware signer.

**Read the documentation in [`docs/`](docs/) before making changes.**

| Doc | Read it when |
| --- | --- |
| [docs/architecture.md](docs/architecture.md) | Any change. Crate boundaries, where things live, how to add a screen. |
| [docs/testing.md](docs/testing.md) | Any change. How to verify, and the golden-image workflow. |
| [docs/ui-guide.md](docs/ui-guide.md) | Touching the UI. Footer, geometry, and consistency rules. |
| [docs/security.md](docs/security.md) | Touching the wallet, signing, storage, or anything handling key material. |
| [docs/internals.md](docs/internals.md) | Touching hardware: LCD, camera, QR pipeline, flash, or the emulator. |

## Ground rules

- `just test` must pass before you are done. It is what CI runs: formatting,
  clippy, host tests, golden-image UI tests, and an embedded build.
- Never log or transmit secrets. Entropy, mnemonics, seeds, extended private
  keys, complete PSBTs, and signatures must not reach USB, panic output, or the
  emulator state file.
- Static RAM is the binding constraint. Check `just size` if you change
  buffers, allocations, or add a dependency.
- This project is not audited and is staged; see the roadmap in
  [docs/security.md](docs/security.md). Do not describe unimplemented hardening
  as if it protects anyone.
