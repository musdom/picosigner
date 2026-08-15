flash mode="":
    cargo run --release --bin picosigner {{ if mode == "debug" { "--features debug" } else { "" } }}

emulate *args:
    cargo run --target x86_64-unknown-linux-gnu -p picosigner-emulator -- {{ args }}
