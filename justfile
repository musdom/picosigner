flash mode="":
    cargo run --release --bin picosigner {{ if mode == "debug" { "--features debug" } else { "" } }}
