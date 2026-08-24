use embedded_hal::delay::DelayNs;
use picosigner_wallet::{TaprootSigningDebug, WalletError};
use usb_device::{class_prelude::UsbBus, device::UsbDevice};
use usbd_serial::SerialPort;

const BEGIN: &[u8] = b"SIGNED_PSBT_HEX_BEGIN\r\n";
const END: &[u8] = b"\r\nSIGNED_PSBT_HEX_END\r\n";
const IDLE_TIMEOUT_MS: usize = 2_000;

pub fn poll<B: UsbBus>(usb: &mut UsbDevice<'_, B>, serial: &mut SerialPort<'_, B>) {
    let _ = usb.poll(&mut [serial]);
}

pub fn log_signed_psbt<B: UsbBus, D: DelayNs>(
    usb: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    delay: &mut D,
    psbt: &[u8],
    taproot_debug: &[TaprootSigningDebug],
) {
    for entry in taproot_debug {
        if !write_all(usb, serial, delay, b"TAPROOT_SIGHASH=")
            || !write_hex(usb, serial, delay, &entry.sighash)
            || !write_all(usb, serial, delay, b"\r\nTAPROOT_OUTPUT_KEY=")
            || !write_hex(usb, serial, delay, &entry.output_key)
            || !write_all(
                usb,
                serial,
                delay,
                if entry.self_verified {
                    b"\r\nTAPROOT_SELF_VERIFY=OK\r\n"
                } else {
                    b"\r\nTAPROOT_SELF_VERIFY=FAILED\r\n"
                },
            )
        {
            return;
        }
    }
    if !write_all(usb, serial, delay, BEGIN) {
        return;
    }

    for chunk in psbt.chunks(32) {
        if !write_hex(usb, serial, delay, chunk) {
            return;
        }
    }
    if !write_all(usb, serial, delay, END) {
        return;
    }

    // Give the final endpoint packet time to leave DPRAM before returning to
    // the animated QR loop, which doesn't otherwise need USB servicing.
    for _ in 0..10 {
        poll(usb, serial);
        delay.delay_ms(1);
    }
}

pub fn log_signing_error<B: UsbBus, D: DelayNs>(
    usb: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    delay: &mut D,
    error: &WalletError,
) {
    let name = match error {
        WalletError::InvalidEntropy => b"InvalidEntropy" as &[u8],
        WalletError::InvalidPsbt => b"InvalidPsbt",
        WalletError::OversizedPsbt => b"OversizedPsbt",
        WalletError::TooManyInputs => b"TooManyInputs",
        WalletError::TooManyOutputs => b"TooManyOutputs",
        WalletError::PsbtV2 => b"PsbtV2",
        WalletError::MissingPrevout => b"MissingPrevout",
        WalletError::UnsafeSighash => b"UnsafeSighash",
        WalletError::NetworkMismatch => b"NetworkMismatch",
        WalletError::ReviewChanged => b"ReviewChanged",
        WalletError::PolicyRejected => b"PolicyRejected",
        WalletError::InvalidTaprootTweak => b"InvalidTaprootTweak",
        WalletError::InvalidSignature => b"InvalidSignature",
        WalletError::InvalidSchnorrKey => b"InvalidSchnorrKey",
        WalletError::SchnorrSelfCheck => b"SchnorrSelfCheck",
        WalletError::InvalidSchnorrEncoding => b"InvalidSchnorrEncoding",
        WalletError::Descriptor => b"Descriptor",
        WalletError::InvalidMnemonic => b"InvalidMnemonic",
        WalletError::InvalidSeedQr => b"InvalidSeedQr",
        WalletError::InvalidFrostBackup => b"InvalidFrostBackup",
    };
    let _ = write_all(usb, serial, delay, b"SIGNING_ERROR=");
    let _ = write_all(usb, serial, delay, name);
    let _ = write_all(usb, serial, delay, b"\r\n");
}

fn write_hex<B: UsbBus, D: DelayNs>(
    usb: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    delay: &mut D,
    bytes: &[u8],
) -> bool {
    let mut encoded = [0u8; 64];
    for chunk in bytes.chunks(32) {
        for (index, byte) in chunk.iter().copied().enumerate() {
            encoded[index * 2] = hex(byte >> 4);
            encoded[index * 2 + 1] = hex(byte & 0x0f);
        }
        if !write_all(usb, serial, delay, &encoded[..chunk.len() * 2]) {
            return false;
        }
    }
    true
}

fn write_all<B: UsbBus, D: DelayNs>(
    usb: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    delay: &mut D,
    mut bytes: &[u8],
) -> bool {
    let mut idle_ms = 0usize;
    while !bytes.is_empty() {
        poll(usb, serial);
        match serial.write(bytes) {
            Ok(written) if written > 0 => {
                bytes = &bytes[written..];
                idle_ms = 0;
            }
            _ => {
                idle_ms += 1;
                if idle_ms >= IDLE_TIMEOUT_MS {
                    return false;
                }
                delay.delay_ms(1);
            }
        }
    }
    true
}

fn hex(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'a' + nibble - 10,
    }
}

/// Per-stage scanner timings, in microseconds.
#[derive(Clone, Copy, Default)]
pub struct ScanTiming {
    pub capture_us: u32,
    pub extract_us: u32,
    pub decode_us: u32,
    pub downscale_us: u32,
    pub lcd_us: u32,
    pub frame_us: u32,
}

/// Emits one `CAP=...` line, best-effort and non-blocking.
///
/// Deliberately does NOT use `write_all`: that blocks for up to
/// `IDLE_TIMEOUT_MS` when nothing is draining the port, which stalls the
/// scanner loop and makes touch feel unresponsive on a debug build with no
/// terminal attached. A dropped timing sample beats a laggy UI.
pub fn log_scan_timing<B: UsbBus>(
    usb: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    timing: &ScanTiming,
) {
    let mut line = heapless::String::<160>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "CAP={}us EXTRACT={}us DECODE={}us QR={}us DOWNSCALE={}us LCD={}us FRAME={}us\r\n",
            timing.capture_us,
            timing.extract_us,
            timing.decode_us,
            timing.extract_us + timing.decode_us,
            timing.downscale_us,
            timing.lcd_us,
            timing.frame_us,
        ),
    );
    poll(usb, serial);
    let _ = serial.write(line.as_bytes());
}

/// Dumps the OV5640 clock and frame-timing registers, with the PCLK and frame
/// period they imply, so what the sensor is *actually* running can be observed
/// instead of inferred from frame timings.
///
/// XCLK is the PWM-generated sensor input clock in Hz (25 MHz on this board).
pub fn log_camera_clocks<B: UsbBus, D: DelayNs>(
    usb: &mut UsbDevice<'_, B>,
    serial: &mut SerialPort<'_, B>,
    delay: &mut D,
    regs: &[u8; 14],
    xclk_hz: u32,
) {
    let mut line = heapless::String::<320>::new();
    let names = [
        "3034", "3035", "3036", "3037", "3108", "3824", "460c", "380c", "380d", "380e", "380f",
        "3a02", "3a03", "3008",
    ];
    let _ = core::fmt::Write::write_str(&mut line, "CAMREG");
    for (name, value) in names.iter().zip(regs.iter()) {
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!(" {}={:02x}", name, value));
    }
    let _ = core::fmt::Write::write_str(&mut line, "\r\n");
    let _ = write_all(usb, serial, delay, line.as_bytes());

    // Decode the PLL chain. Datasheet: 0x3037[3:0] pre-divider, 0x3036 multiplier,
    // 0x3035[7:4] system divider, 0x3037[4] PLL root divider (1 = /2),
    // 0x3108[5:4] PCLK root divider (2^n), 0x3824[4:0] manual DVP divider.
    let pre_div = u32::from(regs[3] & 0x0f).max(1);
    let mult = u32::from(regs[2]);
    let sys_div = u32::from((regs[1] >> 4) & 0x0f).max(1);
    let pll_root = if regs[3] & 0x10 != 0 { 2 } else { 1 };
    let pclk_root = 1u32 << ((regs[4] >> 4) & 0x03);
    let dvp_div = u32::from(regs[5] & 0x1f).max(1);
    let hts = (u32::from(regs[7]) << 8) | u32::from(regs[8]);
    let vts = (u32::from(regs[9]) << 8) | u32::from(regs[10]);

    let vco = xclk_hz / pre_div * mult;
    let pclk = vco / pll_root / sys_div / pclk_root / dvp_div;
    let frame_us = if pclk > 0 {
        (u64::from(hts) * u64::from(vts) * 1_000_000 / u64::from(pclk)) as u32
    } else {
        0
    };

    let mut line2 = heapless::String::<200>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line2,
        format_args!(
            "CAMCLK vco={}kHz pclk={}kHz hts={} vts={} frame={}us manual_dvp={}\r\n",
            vco / 1000,
            pclk / 1000,
            hts,
            vts,
            frame_us,
            (regs[6] >> 1) & 1,
        ),
    );
    let _ = write_all(usb, serial, delay, line2.as_bytes());
}
