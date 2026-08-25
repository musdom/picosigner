flash mode="":
    cargo run --release --bin picosigner {{ if mode == "debug" { "--features debug" } else { "" } }}

emulate *args:
    cargo run --target x86_64-unknown-linux-gnu -p picosigner-emulator -- {{ args }}

# Convert a single-frame Piskel C export into packed Rust u32 icon rows.
# Pass an explicit Rust constant name as the optional second argument.
icon-array source constant="":
    @python3 picosigner/assets/piskel-to-rust.py "{{ source }}" "{{ constant }}"
