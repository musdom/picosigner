#![no_std]

//! Small, allocation-free seams for the camera QR path and BBQr transport.

extern crate alloc;

use core::fmt;

pub const MAX_PSBT_BYTES: usize = 32 * 1024;
pub const MAX_FRAGMENTS: usize = 256;
pub const MAX_INPUTS: usize = 64;
pub const MAX_OUTPUTS: usize = 64;
pub const MIN_MODULE_SCALE: usize = 2;
pub const QUIET_ZONE: usize = 4;
pub const DESCRIPTOR_QR_SIDE: usize = 49;
pub const DESCRIPTOR_QR_MODULES: usize = DESCRIPTOR_QR_SIDE * DESCRIPTOR_QR_SIDE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    InvalidHeader,
    InvalidPart,
    MismatchedSession,
    TooLarge,
    TooManyParts,
    Incomplete,
    OutputTooSmall,
    UnsupportedEncoding,
    InvalidDimensions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorQrError {
    TooLarge,
}

/// Encode a Sparrow-compatible BIP-380 descriptor as a QR symbol.
///
/// This is a deliberately bounded byte-mode QR version 8/M encoder. The
/// descriptor export is public data, but it must still be encoded
/// deterministically and without borrowing a heap-backed QR image. Version 8
/// has 154 data codewords, which covers the longest BIP86 descriptor emitted
/// by the wallet (including its checksum).
pub fn encode_descriptor_qr(
    data: &[u8],
    modules: &mut [bool; DESCRIPTOR_QR_MODULES],
) -> Result<(), DescriptorQrError> {
    const DATA_BYTES: usize = 154;
    if data.len() > DATA_BYTES - 2 {
        return Err(DescriptorQrError::TooLarge);
    }

    let mut payload = [0u8; DATA_BYTES];
    let bit_len = 12 + data.len() * 8;
    payload[0] = 0x40 | ((data.len() >> 4) as u8);
    payload[1] = ((data.len() & 0x0f) as u8) << 4;
    let mut bit = 12;
    for &byte in data {
        for shift in (0..8).rev() {
            if byte & (1 << shift) != 0 {
                payload[bit / 8] |= 1 << (7 - bit % 8);
            }
            bit += 1;
        }
    }
    // The byte-mode payload is already zero-filled after the data. Append the
    // terminator and the alternating QR pad bytes to the data codewords.
    if bit_len + 4 <= DATA_BYTES * 8 {
        bit = bit_len;
        bit += 4;
    }
    let mut pad = bit.div_ceil(8);
    let mut toggle = false;
    while pad < DATA_BYTES {
        payload[pad] = if toggle { 0x11 } else { 0xec };
        toggle = !toggle;
        pad += 1;
    }

    let codewords = build_codewords(&payload);

    let (function_colors, reserved_map) = functional_patterns();
    modules.copy_from_slice(&function_colors);
    let reserved = reserved_map;
    place_codewords(modules, &reserved, &codewords);

    // Mask 0 is the QR checkerboard mask. It is valid for every QR symbol;
    // fixed selection keeps the embedded implementation bounded and avoids a
    // second matrix copy for penalty scoring.
    for y in 0..DESCRIPTOR_QR_SIDE {
        for x in 0..DESCRIPTOR_QR_SIDE {
            if !reserved[y * DESCRIPTOR_QR_SIDE + x] && (x + y) % 2 == 0 {
                modules[y * DESCRIPTOR_QR_SIDE + x] = !modules[y * DESCRIPTOR_QR_SIDE + x];
            }
        }
    }
    format_info(modules);
    Ok(())
}

fn build_codewords(payload: &[u8; 154]) -> [u8; 242] {
    let mut codewords = [0u8; 242];
    let mut ec = [[0u8; 22]; 4];
    let block_lengths = [38usize, 38, 39, 39];
    let mut offset = 0;
    for block in 0..4 {
        ec[block] = reed_solomon(&payload[offset..offset + block_lengths[block]]);
        offset += block_lengths[block];
    }
    let mut n = 0;
    for index in 0..39 {
        for block in 0..4 {
            if index < block_lengths[block] {
                let block_offset = match block {
                    0 => 0,
                    1 => 38,
                    2 => 76,
                    _ => 115,
                };
                codewords[n] = payload[block_offset + index];
                n += 1;
            }
        }
    }
    for index in 0..22 {
        for block in 0..4 {
            codewords[n] = ec[block][index];
            n += 1;
        }
    }
    codewords
}

fn functional_patterns() -> ([bool; DESCRIPTOR_QR_MODULES], [bool; DESCRIPTOR_QR_MODULES]) {
    let mut modules = [false; DESCRIPTOR_QR_MODULES];
    let mut reserved = [false; DESCRIPTOR_QR_MODULES];
    finder(&mut modules, &mut reserved, 0, 0);
    finder(&mut modules, &mut reserved, 42, 0);
    finder(&mut modules, &mut reserved, 0, 42);
    for &(x, y) in &[(6, 24), (24, 6), (24, 24), (42, 24), (24, 42), (42, 42)] {
        alignment(&mut modules, &mut reserved, x, y);
    }
    for x in 8..41 {
        set_function(&mut modules, &mut reserved, x, 6, x % 2 == 0);
        set_function(&mut modules, &mut reserved, 6, x, x % 2 == 0);
    }
    reserve_format(&mut reserved);
    set_function(&mut modules, &mut reserved, 8, 41, true);
    version_info(&mut modules, &mut reserved);
    (modules, reserved)
}

fn index(x: usize, y: usize) -> usize {
    y * DESCRIPTOR_QR_SIDE + x
}

fn set_function(
    modules: &mut [bool; DESCRIPTOR_QR_MODULES],
    reserved: &mut [bool; DESCRIPTOR_QR_MODULES],
    x: usize,
    y: usize,
    value: bool,
) {
    modules[index(x, y)] = value;
    reserved[index(x, y)] = true;
}

fn finder(
    modules: &mut [bool; DESCRIPTOR_QR_MODULES],
    reserved: &mut [bool; DESCRIPTOR_QR_MODULES],
    left: usize,
    top: usize,
) {
    for dy in 0usize..9 {
        for dx in 0usize..9 {
            let x = left as i32 + dx as i32 - 1;
            let y = top as i32 + dy as i32 - 1;
            if (0..DESCRIPTOR_QR_SIDE as i32).contains(&x)
                && (0..DESCRIPTOR_QR_SIDE as i32).contains(&y)
            {
                let ix = dx.saturating_sub(1usize);
                let iy = dy.saturating_sub(1usize);
                let inside = (1..8).contains(&dx) && (1..8).contains(&dy);
                let dark = inside
                    && (ix == 0
                        || ix == 6
                        || iy == 0
                        || iy == 6
                        || (2..5).contains(&ix) && (2..5).contains(&iy));
                set_function(modules, reserved, x as usize, y as usize, dark);
            }
        }
    }
}

fn alignment(
    modules: &mut [bool; DESCRIPTOR_QR_MODULES],
    reserved: &mut [bool; DESCRIPTOR_QR_MODULES],
    cx: usize,
    cy: usize,
) {
    if reserved[index(cx, cy)] {
        return;
    }
    for dy in 0..5 {
        for dx in 0..5 {
            set_function(
                modules,
                reserved,
                cx + dx - 2,
                cy + dy - 2,
                dx == 0 || dx == 4 || dy == 0 || dy == 4 || (dx == 2 && dy == 2),
            );
        }
    }
}

fn reserve_format(reserved: &mut [bool; DESCRIPTOR_QR_MODULES]) {
    let coords = [
        (0, 8),
        (1, 8),
        (2, 8),
        (3, 8),
        (4, 8),
        (5, 8),
        (7, 8),
        (8, 8),
        (8, 7),
        (8, 5),
        (8, 4),
        (8, 3),
        (8, 2),
        (8, 1),
        (8, 0),
        (8, 48),
        (8, 47),
        (8, 46),
        (8, 45),
        (8, 44),
        (8, 43),
        (8, 42),
        (41, 8),
        (42, 8),
        (43, 8),
        (44, 8),
        (45, 8),
        (46, 8),
        (47, 8),
        (48, 8),
    ];
    for (x, y) in coords {
        reserved[index(x, y)] = true;
    }
}

fn version_info(
    modules: &mut [bool; DESCRIPTOR_QR_MODULES],
    reserved: &mut [bool; DESCRIPTOR_QR_MODULES],
) {
    // Version 8 information word from the QR standard.
    let value = 0x085bcu32;
    for i in 0..18 {
        let bit = (value >> (17 - i)) & 1 != 0;
        let x1 = 5 - (i / 3);
        let offset = i % 3;
        set_function(modules, reserved, x1, 40 - offset, bit);
        set_function(modules, reserved, 40 - offset, 5 - (i / 3), bit);
    }
}

fn format_info(modules: &mut [bool; DESCRIPTOR_QR_MODULES]) {
    let value = 0x5412u16; // error correction M, mask 0
    let coords = [
        (0, 8),
        (1, 8),
        (2, 8),
        (3, 8),
        (4, 8),
        (5, 8),
        (7, 8),
        (8, 8),
        (8, 7),
        (8, 5),
        (8, 4),
        (8, 3),
        (8, 2),
        (8, 1),
        (8, 0),
        (8, 48),
        (8, 47),
        (8, 46),
        (8, 45),
        (8, 44),
        (8, 43),
        (8, 42),
        (41, 8),
        (42, 8),
        (43, 8),
        (44, 8),
        (45, 8),
        (46, 8),
        (47, 8),
        (48, 8),
    ];
    for i in 0..15 {
        let bit = (value >> (14 - i)) & 1 != 0;
        let (x, y) = coords[i];
        modules[index(x, y)] = bit;
        let (x, y) = coords[i + 15];
        modules[index(x, y)] = bit;
    }
}

fn place_codewords(
    modules: &mut [bool; DESCRIPTOR_QR_MODULES],
    reserved: &[bool; DESCRIPTOR_QR_MODULES],
    codewords: &[u8; 242],
) {
    let mut x = 48i32;
    let mut y = 48i32;
    let mut bit = 0;
    loop {
        let adjusted = if x <= 6 { x + 1 } else { x };
        if adjusted <= 0 || bit >= codewords.len() * 8 {
            return;
        }
        if !reserved[index(x as usize, y as usize)] {
            let value = (codewords[bit / 8] >> (7 - bit % 8)) & 1 != 0;
            modules[index(x as usize, y as usize)] = value;
            bit += 1;
        }
        let column_type = (49 - adjusted) % 4;
        match column_type {
            2 if y > 0 => {
                y -= 1;
                x += 1;
            }
            0 if y < 48 => {
                y += 1;
                x += 1;
            }
            0 | 2 if x == 7 => {
                x -= 2;
            }
            _ => {
                x -= 1;
            }
        }
    }
}

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut result = 0;
    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        let high = a & 0x80;
        a <<= 1;
        if high != 0 {
            a ^= 0x1d;
        }
        b >>= 1;
    }
    result
}

fn gf_pow(mut value: u8, mut power: usize) -> u8 {
    let mut result = 1;
    while power != 0 {
        if power & 1 != 0 {
            result = gf_mul(result, value);
        }
        value = gf_mul(value, value);
        power >>= 1;
    }
    result
}

fn reed_solomon(data: &[u8]) -> [u8; 22] {
    let mut generator = [0u8; 23];
    generator[0] = 1;
    for i in 0..22 {
        let root = gf_pow(2, i);
        let mut next = [0u8; 23];
        for j in 0..=i {
            next[j] ^= generator[j];
            next[j + 1] ^= gf_mul(generator[j], root);
        }
        generator = next;
    }
    let mut remainder = [0u8; 22];
    for &byte in data {
        let factor = byte ^ remainder[0];
        for j in 0..21 {
            remainder[j] = remainder[j + 1] ^ gf_mul(factor, generator[j + 1]);
        }
        remainder[21] = gf_mul(factor, generator[22]);
    }
    remainder
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BbqrEncoding {
    Hex,
    Base32,
    ZlibBase32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BbqrHeader {
    pub encoding: BbqrEncoding,
    pub file_type: u8,
    pub parts: usize,
    pub index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QrPayload<'a>(pub &'a [u8]);

pub trait QrDecoder {
    type Error;
    fn decode(&mut self, grayscale: &[u8]) -> Result<Option<QrPayload<'_>>, Self::Error>;
}

#[cfg(feature = "embedded")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuircError {
    InvalidDimensions,
    PayloadTooLarge,
}

#[cfg(feature = "embedded")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuircTiming {
    pub identify_us: u32,
    pub decode_us: u32,
}

/// The production quircs adapter. It owns only quircs' reusable work state
/// and a bounded payload copy; callers never depend on the vendored decoder's
/// types or allocation behavior.
#[cfg(feature = "embedded")]
pub struct QuircDecoder<const W: usize, const H: usize, const P: usize> {
    decoder: quircs::Quirc,
    payload: heapless::Vec<u8, P>,
}

#[cfg(feature = "embedded")]
impl<const W: usize, const H: usize, const P: usize> QuircDecoder<W, H, P> {
    pub fn new() -> Self {
        let mut decoder = quircs::Quirc::new();
        decoder.resize(W, H);
        Self {
            decoder,
            payload: heapless::Vec::new(),
        }
    }

    /// Decode one grayscale frame and retain the identify/decode timings used
    /// by the QR diagnostic. The clock callback is deliberately supplied by
    /// the board binary so this crate stays hardware-independent.
    pub fn decode_with_timing<F>(
        &mut self,
        grayscale: &[u8],
        mut clock_micros: F,
    ) -> Result<(Option<QrPayload<'_>>, QuircTiming), QuircError>
    where
        F: FnMut() -> u32,
    {
        if grayscale.len() != W.checked_mul(H).unwrap_or(usize::MAX) {
            return Err(QuircError::InvalidDimensions);
        }

        let identify_started = clock_micros();
        let codes = self.decoder.identify(W, H, grayscale);
        let identify_us = clock_micros().wrapping_sub(identify_started);

        let decode_started = clock_micros();
        let mut decoded = None;
        for mut code in codes.flatten() {
            let result = code.decode().or_else(|_| {
                code.flip();
                code.decode()
            });
            if let Ok(data) = result {
                decoded = Some(data.payload);
                break;
            }
        }
        let decode_us = clock_micros().wrapping_sub(decode_started);

        self.payload.clear();
        if let Some(decoded) = decoded {
            if decoded.len() > P {
                return Err(QuircError::PayloadTooLarge);
            }
            self.payload
                .extend_from_slice(&decoded)
                .map_err(|_| QuircError::PayloadTooLarge)?;
        }

        let payload = (!self.payload.is_empty()).then(|| QrPayload(self.payload.as_slice()));
        Ok((
            payload,
            QuircTiming {
                identify_us,
                decode_us,
            },
        ))
    }
}

#[cfg(feature = "embedded")]
impl<const W: usize, const H: usize, const P: usize> Default for QuircDecoder<W, H, P> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "embedded")]
impl<const W: usize, const H: usize, const P: usize> QrDecoder for QuircDecoder<W, H, P> {
    type Error = QuircError;

    fn decode(&mut self, grayscale: &[u8]) -> Result<Option<QrPayload<'_>>, Self::Error> {
        self.decode_with_timing(grayscale, || 0)
            .map(|(payload, _)| payload)
    }
}

pub trait QrRenderer {
    type Error;
    fn render(&mut self, payload: &[u8]) -> Result<(), Self::Error>;
}

/// A camera-sized RGB565 to grayscale conversion without an intermediate image.
pub fn extract_rgb565(
    src: &[u16],
    width: usize,
    height: usize,
    zoom: bool,
    dst: &mut [u8],
) -> Result<(), TransportError> {
    let out_w = width / 2;
    let out_h = height / 2;
    if width < 2
        || height < 2
        || width.checked_mul(height).is_none_or(|n| src.len() < n)
        || dst.len() < out_w * out_h
    {
        return Err(TransportError::InvalidDimensions);
    }
    for y in 0..out_h {
        for x in 0..out_w {
            let (sx, sy) = if zoom {
                ((width - out_w) / 2 + x, (height - out_h) / 2 + y)
            } else {
                (x * 2, y * 2)
            };
            let pixel = src[sy * width + sx];
            dst[y * out_w + x] = green_luma(pixel);
        }
    }
    Ok(())
}

/// Volatile variant for a DMA-owned camera buffer.
///
/// # Safety
/// `src` must be non-null and point to at least `width * height` readable
/// `u16` values for the duration of this call. The caller must ensure the DMA
/// producer is stopped or otherwise synchronized while the values are read.
pub unsafe fn extract_rgb565_volatile(
    src: *const u16,
    width: usize,
    height: usize,
    zoom: bool,
    dst: &mut [u8],
) -> Result<(), TransportError> {
    let out_w = width / 2;
    let out_h = height / 2;
    if width < 2 || height < 2 || dst.len() < out_w * out_h || src.is_null() {
        return Err(TransportError::InvalidDimensions);
    }
    for y in 0..out_h {
        for x in 0..out_w {
            let (sx, sy) = if zoom {
                ((width - out_w) / 2 + x, (height - out_h) / 2 + y)
            } else {
                (x * 2, y * 2)
            };
            dst[y * out_w + x] = green_luma(core::ptr::read_volatile(src.add(sy * width + sx)));
        }
    }
    Ok(())
}

#[inline]
pub fn green_luma(pixel: u16) -> u8 {
    let green = ((pixel >> 5) & 0x3f) as u8;
    (green << 2) | (green >> 4)
}

/// Draws a QR module matrix through the board/UI-owned surface.
pub trait ModuleSurface {
    type Error;
    fn module(&mut self, x: usize, y: usize) -> Result<(), Self::Error>;
}
pub fn render_modules<S: ModuleSurface>(
    surface: &mut S,
    modules: &[bool],
    side: usize,
    scale: usize,
) -> Result<(), RenderError<S::Error>> {
    if scale < MIN_MODULE_SCALE || side.checked_mul(side) != Some(modules.len()) {
        return Err(RenderError::InvalidDimensions);
    }
    for y in 0..side {
        for x in 0..side {
            if modules[y * side + x] {
                for dy in 0..scale {
                    for dx in 0..scale {
                        surface
                            .module(
                                QUIET_ZONE * scale + x * scale + dx,
                                QUIET_ZONE * scale + y * scale + dy,
                            )
                            .map_err(RenderError::Surface)?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenderError<E> {
    InvalidDimensions,
    Surface(E),
}

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn b36(n: usize) -> Option<u8> {
    if n < 10 {
        Some(b'0' + n as u8)
    } else if n < 36 {
        Some(b'A' + (n as u8 - 10))
    } else {
        None
    }
}
fn from_b36(c: u8) -> Option<usize> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as usize),
        b'A'..=b'Z' => Some((c - b'A' + 10) as usize),
        _ => None,
    }
}

pub fn parse_header(part: &[u8]) -> Result<(BbqrHeader, &[u8]), TransportError> {
    if part.len() < 8 || &part[..2] != b"B$" {
        return Err(TransportError::InvalidHeader);
    }
    let encoding = match part[2] {
        b'H' => BbqrEncoding::Hex,
        b'2' => BbqrEncoding::Base32,
        b'Z' => BbqrEncoding::ZlibBase32,
        _ => return Err(TransportError::UnsupportedEncoding),
    };
    let parts = from_b36(part[4]).ok_or(TransportError::InvalidHeader)? * 36
        + from_b36(part[5]).ok_or(TransportError::InvalidHeader)?;
    let index = from_b36(part[6]).ok_or(TransportError::InvalidHeader)? * 36
        + from_b36(part[7]).ok_or(TransportError::InvalidHeader)?;
    if parts == 0 || parts > MAX_FRAGMENTS || index >= parts {
        return Err(TransportError::TooManyParts);
    }
    Ok((
        BbqrHeader {
            encoding,
            file_type: part[3],
            parts,
            index,
        },
        &part[8..],
    ))
}

fn decode_hex(input: &[u8], out: &mut [u8]) -> Result<usize, TransportError> {
    if input.len() % 2 != 0 || out.len() < input.len() / 2 {
        return Err(TransportError::InvalidPart);
    }
    for (i, pair) in input.chunks_exact(2).enumerate() {
        let hi = HEX
            .iter()
            .position(|&x| x == pair[0])
            .ok_or(TransportError::InvalidPart)?;
        let lo = HEX
            .iter()
            .position(|&x| x == pair[1])
            .ok_or(TransportError::InvalidPart)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Ok(input.len() / 2)
}
fn decode_b32(input: &[u8], out: &mut [u8]) -> Result<usize, TransportError> {
    if input.len() % 8 == 1 {
        return Err(TransportError::InvalidPart);
    }
    let mut acc = 0u32;
    let mut bits = 0usize;
    let mut n = 0;
    for &c in input {
        let v = B32
            .iter()
            .position(|&x| x == c)
            .ok_or(TransportError::InvalidPart)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if n >= out.len() {
                return Err(TransportError::TooLarge);
            }
            out[n] = (acc >> bits) as u8;
            n += 1;
            // Keep only the not-yet-emitted tail. Without this mask, a
            // large fragment eventually overflows the u32 accumulator even
            // though the decoder is otherwise correctly bounded.
            acc = if bits == 0 {
                0
            } else {
                acc & ((1u32 << bits) - 1)
            };
        }
    }
    if bits != 0 && (acc & ((1 << bits) - 1)) != 0 {
        return Err(TransportError::InvalidPart);
    }
    Ok(n)
}

#[derive(Clone, Copy)]
struct Part {
    seen: bool,
    offset: usize,
    len: usize,
}
impl Part {
    const EMPTY: Self = Self {
        seen: false,
        offset: 0,
        len: 0,
    };
}

pub struct BbqrCollector<const N: usize = MAX_PSBT_BYTES> {
    data: [u8; N],
    parts: [Part; MAX_FRAGMENTS],
    header: Option<BbqrHeader>,
    chunk: usize,
    seen: usize,
}
impl<const N: usize> BbqrCollector<N> {
    pub const fn new() -> Self {
        Self {
            data: [0; N],
            parts: [Part::EMPTY; MAX_FRAGMENTS],
            header: None,
            chunk: 0,
            seen: 0,
        }
    }
    pub fn push(&mut self, input: &[u8]) -> Result<bool, TransportError> {
        let (h, body) = parse_header(input)?;
        if let Some(old) = self.header {
            if old.encoding != h.encoding || old.file_type != h.file_type || old.parts != h.parts {
                return Err(TransportError::MismatchedSession);
            }
        }
        if self.parts[h.index].seen {
            // A retransmitted index is intentionally ignored. This also
            // avoids allowing a malformed duplicate to disturb a valid
            // session that already recorded this fragment.
            return Ok(self.is_complete());
        }
        let len = match h.encoding {
            BbqrEncoding::Hex => {
                if body.len() % 2 != 0 {
                    return Err(TransportError::InvalidPart);
                }
                body.len() / 2
            }
            BbqrEncoding::Base32 | BbqrEncoding::ZlibBase32 => {
                if body.len() % 8 == 1 {
                    return Err(TransportError::InvalidPart);
                }
                body.len().checked_mul(5).ok_or(TransportError::TooLarge)? / 8
            }
        };
        if len == 0 {
            return Err(TransportError::InvalidPart);
        }
        let capacity = core::cmp::min(N, MAX_PSBT_BYTES);
        if len > capacity {
            return Err(TransportError::TooLarge);
        }
        if h.index + 1 != h.parts {
            if self.chunk == 0 {
                self.chunk = len;
            } else if self.chunk != len {
                return Err(TransportError::InvalidPart);
            }
        }
        // Do not infer the common chunk size from a runt received first.
        // It is established by the first non-final fragment.
        // A final runt may arrive first. It is held at offset zero until a
        // regular part reveals the common chunk size, then moved into place.
        if h.index + 1 != h.parts && self.seen > 0 {
            let final_index = h.parts - 1;
            let final_part = self.parts[final_index];
            if final_part.seen && final_part.offset == 0 {
                if final_part.len > self.chunk {
                    return Err(TransportError::InvalidPart);
                }
                let final_offset = final_index
                    .checked_mul(self.chunk)
                    .ok_or(TransportError::TooLarge)?;
                if final_offset
                    .checked_add(final_part.len)
                    .ok_or(TransportError::TooLarge)?
                    > capacity
                {
                    return Err(TransportError::TooLarge);
                }
                self.data.copy_within(0..final_part.len, final_offset);
                self.parts[final_index].offset = final_offset;
            }
        }
        let offset = h
            .index
            .checked_mul(self.chunk)
            .ok_or(TransportError::TooLarge)?;
        if offset.checked_add(len).ok_or(TransportError::TooLarge)? > capacity {
            return Err(TransportError::TooLarge);
        }
        let destination = &mut self.data[offset..offset + len];
        match h.encoding {
            BbqrEncoding::Hex => decode_hex(body, destination)?,
            BbqrEncoding::Base32 | BbqrEncoding::ZlibBase32 => decode_b32(body, destination)?,
        };
        if self.header.is_none() {
            self.header = Some(h);
        }
        self.parts[h.index] = Part {
            seen: true,
            offset,
            len,
        };
        self.seen += 1;
        Ok(self.is_complete())
    }
    fn is_complete(&self) -> bool {
        self.header.map_or(false, |h| self.seen == h.parts)
    }
    pub fn is_complete_session(&self) -> bool {
        self.is_complete()
    }
    pub fn finish(&mut self) -> Result<&[u8], TransportError> {
        let h = self.header.ok_or(TransportError::Incomplete)?;
        if self.seen != h.parts {
            return Err(TransportError::Incomplete);
        }
        let mut end = 0;
        for i in 0..h.parts {
            let p = self.parts[i];
            if !p.seen {
                return Err(TransportError::Incomplete);
            };
            end = end.max(p.offset + p.len);
        }
        if h.encoding == BbqrEncoding::ZlibBase32 {
            let limit = core::cmp::min(N, MAX_PSBT_BYTES);
            let v = miniz_oxide::inflate::decompress_to_vec_with_limit(&self.data[..end], limit)
                .map_err(|_| TransportError::InvalidPart)?;
            let n = v.len();
            self.data[..n].copy_from_slice(&v);
            Ok(&self.data[..n])
        } else {
            Ok(&self.data[..end])
        }
    }
    pub fn as_bytes(&self) -> Result<&[u8], TransportError> {
        if self
            .header
            .map_or(false, |h| h.encoding == BbqrEncoding::ZlibBase32)
        {
            Err(TransportError::Incomplete)
        } else if self.is_complete() {
            let end = self
                .parts
                .iter()
                .take(self.header.unwrap().parts)
                .map(|p| p.offset + p.len)
                .max()
                .unwrap_or(0);
            Ok(&self.data[..end])
        } else {
            Err(TransportError::Incomplete)
        }
    }
}

pub struct BbqrEncoder;
impl BbqrEncoder {
    pub fn encode<const M: usize>(
        data: &[u8],
        file_type: u8,
        encoding: BbqrEncoding,
        chunk_bytes: usize,
        out: &mut [heapless::Vec<u8, M>],
    ) -> Result<usize, TransportError> {
        if data.len() > MAX_PSBT_BYTES || chunk_bytes == 0 {
            return Err(TransportError::TooLarge);
        }
        let compressed = if encoding == BbqrEncoding::ZlibBase32 {
            Some(miniz_oxide::deflate::compress_to_vec(data, 9))
        } else {
            None
        };
        let source = compressed.as_deref().unwrap_or(data);
        if source.len() > MAX_PSBT_BYTES {
            return Err(TransportError::TooLarge);
        }
        let parts = source
            .len()
            .checked_add(chunk_bytes - 1)
            .ok_or(TransportError::TooLarge)?
            / chunk_bytes;
        if parts == 0 || parts > MAX_FRAGMENTS || parts > out.len() {
            return Err(TransportError::TooManyParts);
        }
        for i in 0..parts {
            let body =
                &source[i * chunk_bytes..core::cmp::min(source.len(), (i + 1) * chunk_bytes)];
            let mut encoded = heapless::Vec::<u8, M>::new();
            let code = match encoding {
                BbqrEncoding::Hex => b'H',
                BbqrEncoding::Base32 | BbqrEncoding::ZlibBase32 => {
                    if encoding == BbqrEncoding::ZlibBase32 {
                        b'Z'
                    } else {
                        b'2'
                    }
                }
            };
            for &c in body {
                if encoding == BbqrEncoding::Hex {
                    encoded
                        .extend_from_slice(&[HEX[(c >> 4) as usize], HEX[(c & 15) as usize]])
                        .map_err(|_| TransportError::OutputTooSmall)?;
                }
            }
            if encoding != BbqrEncoding::Hex {
                let mut acc = 0u32;
                let mut bits = 0;
                for &c in body {
                    acc = (acc << 8) | c as u32;
                    bits += 8;
                    while bits >= 5 {
                        bits -= 5;
                        encoded
                            .push(B32[((acc >> bits) & 31) as usize])
                            .map_err(|_| TransportError::OutputTooSmall)?;
                    }
                }
                if bits > 0 {
                    encoded
                        .push(B32[((acc << (5 - bits)) & 31) as usize])
                        .map_err(|_| TransportError::OutputTooSmall)?;
                }
            }
            let mut header = [0u8; 8];
            header[..4].copy_from_slice(&[b'B', b'$', code, file_type]);
            header[4] = b36(parts / 36).ok_or(TransportError::TooManyParts)?;
            header[5] = b36(parts % 36).unwrap();
            header[6] = b36(i / 36).unwrap();
            header[7] = b36(i % 36).unwrap();
            out[i].clear();
            out[i]
                .extend_from_slice(&header)
                .map_err(|_| TransportError::OutputTooSmall)?;
            out[i]
                .extend_from_slice(&encoded)
                .map_err(|_| TransportError::OutputTooSmall)?;
        }
        Ok(parts)
    }
}

impl fmt::Debug for BbqrCollector<MAX_PSBT_BYTES> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BbqrCollector")
            .field("seen", &self.seen)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shuffled_duplicates_and_hex() {
        let input = b"0123456789abcdef";
        let mut out = [
            heapless::Vec::<u8, 64>::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
        ];
        let n = BbqrEncoder::encode(input, b'P', BbqrEncoding::Hex, 4, &mut out).unwrap();
        let mut c = BbqrCollector::<64>::new();
        assert!(!c.push(&out[2]).unwrap());
        assert!(!c.push(&out[0]).unwrap());
        assert!(!c.push(&out[0]).unwrap());
        assert!(!c.push(&out[1]).unwrap());
        assert!(c.push(&out[3]).unwrap());
        assert_eq!(c.finish().unwrap(), input);
        assert_eq!(n, 4);
    }
    #[test]
    fn base32_roundtrip() {
        let input = b"hello BBQr";
        let mut out = [
            heapless::Vec::<u8, 64>::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
        ];
        BbqrEncoder::encode(input, b'P', BbqrEncoding::Base32, 4, &mut out).unwrap();
        let mut c = BbqrCollector::<64>::new();
        for p in &out {
            if !p.is_empty() {
                c.push(p).unwrap();
            }
        }
        assert_eq!(c.finish().unwrap(), input);
    }

    #[test]
    fn zlib_roundtrip_and_limit() {
        let input = b"repeated PSBT-like data repeated PSBT-like data repeated PSBT-like data";
        let mut out = [
            heapless::Vec::<u8, 128>::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
            heapless::Vec::new(),
        ];
        let n = BbqrEncoder::encode(input, b'P', BbqrEncoding::ZlibBase32, 8, &mut out).unwrap();
        let mut c = BbqrCollector::<128>::new();
        for part in out.iter().take(n).rev() {
            c.push(part).unwrap();
        }
        assert_eq!(c.finish().unwrap(), input);
    }

    #[test]
    fn extraction_is_bounded_and_uses_green_channel() {
        let src = [0xf800, 0x07e0, 0x001f, 0xffff];
        let mut dst = [0; 1];
        extract_rgb565(&src, 2, 2, false, &mut dst).unwrap();
        assert_eq!(dst[0], green_luma(0xf800));
    }
    #[test]
    fn malformed_is_rejected() {
        let mut c = BbqrCollector::<64>::new();
        assert_eq!(c.push(b"B$HP0100A"), Err(TransportError::InvalidPart));
        assert_eq!(c.push(b"B$HP0000"), Err(TransportError::TooManyParts));
        let mut c = BbqrCollector::<64>::new();
        assert_eq!(c.push(b"B$2P0100A"), Err(TransportError::InvalidPart));
        let mut c = BbqrCollector::<64>::new();
        assert_eq!(
            c.push(b"B$XP010000"),
            Err(TransportError::UnsupportedEncoding)
        );
    }

    #[test]
    fn limits_reject_oversized_sessions_and_part_counts() {
        assert_eq!(parse_header(b"B$HP7S00"), Err(TransportError::TooManyParts));

        let input = b"abcdefgh";
        let mut out = [heapless::Vec::<u8, 32>::new(), heapless::Vec::new()];
        BbqrEncoder::encode(input, b'P', BbqrEncoding::Hex, 4, &mut out).unwrap();
        let mut collector = BbqrCollector::<6>::new();
        assert!(!collector.push(&out[0]).unwrap());
        assert_eq!(collector.push(&out[1]), Err(TransportError::TooLarge));
    }

    #[test]
    fn long_base32_fragment_stays_bounded() {
        let input = [0xa5u8; 1024];
        let mut out = [heapless::Vec::<u8, 1700>::new()];
        BbqrEncoder::encode(&input, b'P', BbqrEncoding::Base32, 1024, &mut out).unwrap();
        let mut collector = BbqrCollector::<1024>::new();
        assert!(collector.push(&out[0]).unwrap());
        assert_eq!(collector.finish().unwrap(), input);
    }

    #[test]
    fn descriptor_qr_matches_reference_encoder_for_mask_zero() {
        let input = b"tr([deadbeef/86h/1h/0h]xpub6CUGRUonJvQ2h9m1z2H3q4R5s6T7u8V9w0X1Y2Z3a4b5c6d7e8f9g0h1i2j3k4l5m6n7o8p9q0r1s2t3u4v5w6x7y8z<0;1>/*)#checksum";
        let mut actual = [false; DESCRIPTOR_QR_MODULES];
        encode_descriptor_qr(input, &mut actual).unwrap();

        let version = qrcode::Version::Normal(8);
        let level = qrcode::EcLevel::M;
        let mut bits = qrcode::bits::Bits::new(version);
        bits.push_byte_data(input).unwrap();
        bits.push_terminator(level).unwrap();
        let raw = bits.into_bytes();
        let mut ours_raw = [0u8; 154];
        ours_raw[0] = 0x40 | ((input.len() >> 4) as u8);
        ours_raw[1] = ((input.len() & 0x0f) as u8) << 4;
        let mut raw_bit = 12;
        for &byte in input {
            for shift in (0..8).rev() {
                if byte & (1 << shift) != 0 {
                    ours_raw[raw_bit / 8] |= 1 << (7 - raw_bit % 8);
                }
                raw_bit += 1;
            }
        }
        let mut raw_pad = (12 + input.len() * 8 + 4).div_ceil(8);
        let mut raw_toggle = false;
        while raw_pad < 154 {
            ours_raw[raw_pad] = if raw_toggle { 0x11 } else { 0xec };
            raw_toggle = !raw_toggle;
            raw_pad += 1;
        }
        assert_eq!(&ours_raw[..], &raw[..]);
        let reference_ec = qrcode::ec::create_error_correction_code(&raw[..38], 22);
        assert_eq!(&reed_solomon(&raw[..38])[..], &reference_ec[..]);
        let (data, ec) = qrcode::ec::construct_codewords(&raw, version, level).unwrap();
        let ours_codewords = build_codewords(&raw.try_into().unwrap());
        let mut reference_codewords = [0u8; 242];
        reference_codewords[..154].copy_from_slice(&data);
        reference_codewords[154..].copy_from_slice(&ec);
        assert_eq!(&ours_codewords[..], &reference_codewords[..]);
        let (function_colors, reserved) = functional_patterns();
        let mut zero_modules = function_colors;
        place_codewords(&mut zero_modules, &reserved, &[0u8; 242]);
        let mut zero_canvas = qrcode::canvas::Canvas::new(version, level);
        zero_canvas.draw_all_functional_patterns();
        zero_canvas.draw_data(&[0u8; 154], &[0u8; 88]);
        for (index, color) in zero_canvas.clone().into_colors().into_iter().enumerate() {
            assert_eq!(
                zero_modules[index],
                color == qrcode::Color::Dark,
                "zero placement module {index}"
            );
        }
        let (_, reserved) = functional_patterns();
        for y in 0..DESCRIPTOR_QR_SIDE {
            for x in 0..DESCRIPTOR_QR_SIDE {
                if (38..=40).contains(&x) && y <= 5 || x <= 5 && (38..=42).contains(&y) {
                    continue;
                }
                assert_eq!(
                    reserved[index(x, y)],
                    qrcode::canvas::is_functional(version, 49, x as i16, y as i16),
                    "functional module {x},{y}"
                );
            }
        }
        let mut canvas = qrcode::canvas::Canvas::new(version, level);
        canvas.draw_all_functional_patterns();
        canvas.draw_data(&data, &ec);
        canvas.apply_mask(qrcode::canvas::MaskPattern::Checkerboard);
        let expected = canvas.into_colors();
        for (index, color) in expected.into_iter().enumerate() {
            assert_eq!(
                actual[index],
                color == qrcode::Color::Dark,
                "module {index}"
            );
        }
    }
}
