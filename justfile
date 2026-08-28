host := "x86_64-unknown-linux-gnu"
embedded := "thumbv8m.main-none-eabihf"

# Library crates that build for the host. The `picosigner` binaries are
# embedded-only, so host runs go through the library and the emulator.
host_libs := "-p picosigner -p picosigner-ui -p picosigner-qr -p picosigner-wallet -p picosigner-board -p quircs-nostd -p embedded-ttf"

flash mode="":
    cargo run --release --bin picosigner {{ if mode == "debug" { "--features debug" } else { "" } }}

emulate *args:
    cargo run --target {{ host }} -p picosigner-emulator -- {{ args }}

# Everything CI runs. Use this before pushing.
default: test

test: fmt-check lint test-host check-embedded

# Host unit tests, plus the emulator's integration and golden-image tests.
# `--lib` keeps this to library code: the firmware binary and the hardware
# diagnostics in picosigner-board/examples only build for the board, and
# `check-embedded` covers those.
test-host:
    cargo test --target {{ host }} --no-default-features --features host {{ host_libs }} --lib
    cargo test --target {{ host }} -p picosigner-emulator

# Re-render every golden baseline. Review the diff before committing.
bless:
    UPDATE_GOLDEN=1 cargo test --target {{ host }} -p picosigner-emulator --test golden

# Render the UI to PNGs for offline review.
screenshots out="screenshots":
    cargo run --target {{ host }} -p picosigner-emulator --example screenshots -- "{{ out }}"

# The firmware and every hardware diagnostic must still build for the board.
check-embedded:
    cargo check --target {{ embedded }} --bins
    cargo check --target {{ embedded }} -p picosigner-board --examples

# Run a hardware diagnostic on a board in BOOTSEL mode.
# Examples: camera, lcd_rgb, qr, usb.
diagnostic name:
    cargo run --release -p picosigner-board --example {{ name }}

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# Clippy gate.
#
# Enforced (`-D warnings`) on the crates this refactor is adding to, so new
# code cannot regress. The rest carry a pre-existing backlog of style lints and
# are reported without failing: `picosigner/src/ui.rs` is slated for deletion in
# stage 3, and the wallet/QR lints sit in audit-critical code (the hand-rolled
# encoders and the signing path) where a drive-by fix costs a reviewer more
# than it saves. Clearing that backlog is its own reviewable change.
lint:
    cargo clippy --no-deps --target {{ host }} -p picosigner-emulator -- -D warnings
    cargo clippy --no-deps --target {{ host }} --no-default-features --features host -p picosigner-ui -- -D warnings
    -cargo clippy --no-deps --target {{ host }} --no-default-features --features host -p picosigner -p picosigner-wallet -p picosigner-qr -p picosigner-board -p embedded-ttf

# Static RAM and flash use of the release firmware, against the budget in
# conteng/stage2-refactor.md. `bss` must not grow.
size:
    cargo build --release --bin picosigner
    @size target/{{ embedded }}/release/picosigner

# Convert a single-frame Piskel C export into packed Rust u32 icon rows.
# Pass an explicit Rust constant name as the optional second argument.
icon-array source constant="":
    @python3 picosigner/assets/piskel-to-rust.py "{{ source }}" "{{ constant }}"
