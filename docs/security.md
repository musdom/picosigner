# Security

> **⚠️ Not audited.** picosigner is in development. Secure boot and the
> anti-glitch features are not enabled, seed storage is plaintext, and several
> hardening items below are unimplemented. Do not use it to hold real bitcoin.

## Status of hardening work

picosigner is being built in stages (see the roadmap in `README.md`). Stage 2 —
wallet features — is implemented; **Stage 3, security hardening, is not**. The
following are known gaps, not oversights:

| Gap | Where | Status |
| --- | --- | --- |
| Seed entropy stored in plaintext | `StoredSeed`, `picosigner-board/src/lib.rs` | Stage 3 |
| `StoredSeed` is `Copy`/`Debug` with no `Drop`/`Zeroize`, so raw entropy propagates unzeroized | `picosigner-board/src/lib.rs:137` | Stage 3 |
| No PIN or attempt policy | — | Stage 3 |
| Secure boot and glitch detectors not enabled | RP2350 fuses | Stage 3 |
| Anti-exfil ([Dark Skippy](https://darkskippy.com/)) | signing path | Stage 3 |
| Reproducible builds | release process | Stage 3 |

## USB policy and future wallet connectivity

USB data access defaults off in both normal and debug firmware. The device
setting is enabled only through Home > Settings > Enable USB and a long-press
confirmation, persisted independently of wallets, and applied by restarting.
When disabled the application leaves the USB peripheral in reset. The physical
BOOTSEL recovery path remains available. This is a firmware policy, not a
hardware data-line disconnect or protection against replacing the firmware.

When enabled, the current development firmware permits a connected computer
to request an unauthenticated reboot into the ROM bootloader for flashing;
debug builds also expose their serial interface. The preference is checksummed
for accidental corruption, not authenticated. It is not secure boot or an
authenticated update mechanism.

Future hardened usage must retain a USB-disabled option. Optional USB wallet
connectivity for Sparrow or Liana should be designed for integration with
[bhwi](https://github.com/wizardsardine/bhwi). Bhwi is a host interface with
per-device protocols, so that work requires choosing a device wire protocol
and adding the corresponding integration; it is not implemented here. Treat
wallet communication and development bootloader access as separate permissions
when designing that support. Enabling wallet connectivity should not implicitly
grant unauthenticated firmware update access in a hardened design.

## Reviewing the signing path

The audit-critical code lives in two small modules, each with a header
explaining what to check:

| File | Holds |
| --- | --- |
| `picosigner-wallet/src/review.rs` | `review` — parses the PSBT within fixed bounds and builds what the user is shown, including which outputs are genuinely change. |
| `picosigner-wallet/src/sign.rs` | `sign` — re-checks the reviewed digest, applies the signing policy, and signs only owned inputs. |
| `picosigner-wallet/src/descriptor.rs` | Descriptor construction, parsing, and the cache tag that binds a cached descriptor to its wallet. |

`picosigner-wallet/src/lib.rs` holds the shared types (`WalletSecret`,
`WalletSession`, the error enum) and the entropy mixer.

Bounds live at the top of `lib.rs`: PSBTs are capped at 32 KiB with at most 64
inputs and 64 outputs.

The single most important property: **the signature covers what the user
approved.** `sign` re-hashes the exact bytes it is given and compares that
against the digest recorded during review, before parsing anything.

## Taproot signing review, 2026-08-23

The review below was performed against the tree of 2026-08-23. Its findings
still apply unless marked otherwise; line numbers in the original text refer to
that tree and have drifted. It is reproduced here because a review nobody can
read protects nobody.

Reviewed 2026-08-23. Scope: BIP341/342/340 signing path — `WalletSession::review`, `sign`, and `sign_with_rust_bitcoin` in `picosigner-wallet/src/lib.rs`, plus the firmware entry points in `picosigner/src/app.rs`. Line numbers refer to the tree at time of review.

## Verdict

No path to fund loss was found. The amount/script commitments inherent to `SIGHASH_DEFAULT`/`SIGHASH_ALL` (all prevout amounts and all outputs are hashed), the requirement that every prevout be present before signing (`lib.rs:523-540`), and the digest binding between review and sign (`lib.rs:428`) close the known theft routes: output substitution, fee deception via declared amounts, and PSBT mutation after approval all yield transactions that fail on-chain rather than ones that move funds unexpectedly.

The realistic risks are interoperability breakage (legitimate spends failing to sign), silent failure modes that surface only on-chain, and untested critical paths.

## Strengths

- **TOCTOU guard**: `sign()` re-checks the SHA256 digest of the exact bytes against the reviewed digest immediately before signing (`sign` at `lib.rs:428`; firmware stores immutable pending bytes in `app.rs:585`).
- **Double-enforced sighash policy**: unsafe sighash types are warned during review (`lib.rs:366-372`) and hard-rejected by `StrictSigningPolicy` at sign time (`app.rs:638`). The BDK oracle path also sets `allow_all_sighashes = false` (`lib.rs:478`).
- **Tweak defense in depth**: tweaked secret is computed explicitly with even-Y normalization, then the resulting output key is cross-checked against an independent `xonly.tap_tweak()` (`lib.rs:593-613`) and again inside schnorr_fun (`lib.rs:627-629`).
- **Mandatory self-verification** of each signature before it is exported (`lib.rs:632-643`), mitigating fault-injected signatures.
- **Ownership gating**: keys are signed only when fingerprint, BIP86-style path, and re-derived x-only key all match (`lib.rs:548-570`); foreign inputs are skipped.
- **Tests independently verify** recomputed sighashes, tweaks, and Schnorr verification (`lib.rs:1095-1177`).

## Findings

### 1. Key-path signing refused when coordinator lists leaf hashes (interop break)

`lib.rs:580` requires `leaves.is_empty()` before key-path signing. Per BIP341, `tap_merkle_root = None` means no script tree exists, so key-path signing is safe and correct regardless of any leaf hashes listed in `tap_key_origins`. A coordinator that populates leaf hashes while producing pure key-path outputs causes the wallet to skip the key signature and emit script signatures bound to leaves that are not committed anywhere. The input goes out unsigned and the spend fails downstream.

**Recommendation:** allow key-path signing whenever `updated.tap_merkle_root.is_none()`, independent of the listed leaves.

### 2. No binding between derived output key and the UTXO being spent (silent failure)

The signer never verifies that `witness_utxo.script_pubkey == ScriptBuf::new_p2tr(output_key)` for owned inputs, nor that `non_witness_utxo.txid()` matches the input's previous outpoint (`lib.rs:523-540`). This is fail-closed by consensus — the taproot sighash commits to all declared amounts and script pubkeys, so any coordinator lie produces a signature that fails on-chain — but consequences include:

- Malformed or hostile PSBTs are only rejected implicitly; the device happily signs garbage and fails late.
- The review-time fee is computed from the same declared prevouts (`psbt.fee()` at `lib.rs:335`), so fake foreign-input amounts can distort the displayed fee. Theft is impossible (our signature commits to the declared amounts, making the transaction unmineable if they lie), but the display can mislead up to the point where the signature is rejected on-chain.

**Recommendation:** in `review()` or at signing time, verify owned inputs' witness UTXO script pubkey equals the P2TR commitment of the derived output key, and check `non_witness_utxo` txid/vout consistency. This converts silent garbage into a clear rejection and removes fee-display ambiguity.

### 3. Review warnings are dead code (opaque failures)

`ReviewWarning::UnknownInput`, `UnverifiedChange`, and `NetworkMismatch` are defined (`lib.rs:219-223`) but never produced anywhere. Nothing in the firmware renders `review.warnings`; `StrictSigningPolicy` simply rejects at hold time with a generic error (`app.rs:627-647`). Fail-closed, but users receive no explanation during review, including for outputs whose addresses do not parse on the active network (`address: None`).

**Recommendation:** either surface warnings on the review screen or delete the unused variants so the warning set matches reality.

### 4. Script-path signing authorizes unverifiable semantics

The script-path loop (`lib.rs:653-678`) signs whatever leaf hashes the coordinator supplies. The device neither parses the tapscripts nor validates merkle inclusion of those leaves under the tweak root. Signatures still commit to the user-reviewed outputs, so redirection/theft is not possible, but the device blesses arbitrary script semantics it never displayed to the user.

Additionally, this loop currently has **zero test coverage** — all fixtures use empty leaf lists.

**Recommendation:** add tests covering script-path signing; consider displaying (or at minimum counting) script-path inputs during review.

### 5. Odd-Y normalization path is untested

The even-Y secret normalization before the BIP341 tweak (`lib.rs:589-599`) works around an RP2350 libsecp quirk (in-place keypair x-only tweak yielding an invalid secret half for odd-Y internal keys). No test fixture uses an odd-parity internal key, so the workaround's happy path is never exercised end-to-end. Related subtlety: script-path signing deliberately uses the *un-normalized* child keypair (correct per BIP342, since libsecp normalizes internally), which is easy to regress. The dual cross-checks (`lib.rs:607-613`, `:627-629`) would catch most regressions, but not consistently-wrong pairs.

**Recommendation:** add a fixture with an odd-parity internal key asserting end-to-end signature validity against the tweaked output key.

### 6. Deterministic nonces without auxiliary randomness (hardening)

Key-path signatures use schnorr_fun deterministic nonces (`lib.rs:622`); script-path uses `sign_schnorr_no_aux_rand` (`lib.rs:670`). Deterministic nonces are replay-safe (identical message + key reproduces an identical signature), but on a glitch-prone microcontroller, deterministic-nonce schemes combined with fault injection are a known key-extraction vector. Self-verification protects against faulty *signatures* being exported; it does not protect the nonce computation itself.

**Recommendation:** mix hardware TRNG entropy into nonce generation (schnorr_fun's `NonceGen` supports additional entropy) while retaining determinism as a fallback property.

### 7. Minor issues

- Misleading error mapping: `derive_priv` failure maps to `PolicyRejected` (`lib.rs:564`); sighash computation failures map to `MissingPrevout` (`lib.rs:588`, `:668`). Cosmetic but obscures diagnostics.
- Secret hygiene: only the raw seed is zeroized (`lib.rs:521`). Derived `Xpriv`, child private keys, and the tweaked secret remain in RAM after signing (rust-bitcoin/secp256k1 types lack zeroize-on-drop). Acceptable for current threat model; worth tracking.
- Host-only BDK fallback maps every BDK failure to `PolicyRejected` (`lib.rs:484`); dead code behind `#[cfg(feature = "host")]`.
- PSBT parser accepts v0 only, ≤64 inputs, ≤32 KiB (`lib.rs:796-811`) — reasonable DoS bounds; v0-only is an interop limitation, not a defect.
