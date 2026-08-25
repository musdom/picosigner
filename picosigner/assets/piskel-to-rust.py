#!/usr/bin/env python3
"""Convert a single-frame Piskel C export to packed Rust u32 rows."""

import re
import sys
from pathlib import Path
from typing import NoReturn


def fail(message: str) -> NoReturn:
    raise SystemExit(f"error: {message}")


def define(source: str, name: str) -> int:
    match = re.search(rf"^\s*#define\s+{name}\s+(\d+)\s*$", source, re.MULTILINE)
    if match is None:
        fail(f"missing {name} definition")
    return int(match.group(1))


def rust_name(path: Path, width: int, height: int) -> str:
    stem = re.sub(r"[^A-Za-z0-9]+", "_", path.stem).strip("_").upper()
    if not stem:
        fail("cannot infer a Rust constant name from the filename")
    if stem[0].isdigit():
        stem = f"ICON_{stem}"
    return f"{stem}_{width}X{height}"


def main() -> None:
    if len(sys.argv) not in (2, 3):
        fail("usage: piskel-to-rust.py SOURCE.c [CONSTANT_NAME]")

    path = Path(sys.argv[1])
    try:
        source = path.read_text()
    except OSError as error:
        fail(str(error))

    frames = define(source, "NEW_PISKEL_FRAME_COUNT")
    width = define(source, "NEW_PISKEL_FRAME_WIDTH")
    height = define(source, "NEW_PISKEL_FRAME_HEIGHT")
    if frames != 1:
        fail(f"expected one frame, found {frames}")
    if not 1 <= width <= 32:
        fail(f"width must fit in a u32 row (1..=32), found {width}")
    if height < 1:
        fail(f"height must be positive, found {height}")

    data_match = re.search(
        r"new_piskel_data\s*\[[^;=]*=\s*\{(?P<data>.*?)\}\s*;",
        source,
        re.DOTALL,
    )
    if data_match is None:
        fail("missing new_piskel_data array")
    pixels = [
        int(value, 16)
        for value in re.findall(r"0x[0-9A-Fa-f]+", data_match.group("data"))
    ]
    expected = width * height
    if len(pixels) != expected:
        fail(f"expected {expected} pixels, found {len(pixels)}")

    name = sys.argv[2] if len(sys.argv) == 3 and sys.argv[2] else rust_name(path, width, height)
    if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) is None:
        fail(f"invalid Rust constant name: {name!r}")

    print(f"const {name}: [u32; {height}] = [")
    for y in range(height):
        row = pixels[y * width : (y + 1) * width]
        visible = [(pixel >> 24) != 0 for pixel in row]
        mask = sum(pixel << x for x, pixel in enumerate(visible))
        preview = "".join("#" if pixel else "." for pixel in visible)
        print(f"    0b{mask:0{width}b}, // {preview}")
    print("];")


if __name__ == "__main__":
    main()
