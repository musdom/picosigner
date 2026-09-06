//! Pico SDK reset interface, discovered by `picotool load -f` / `reboot -f`.
//! Protocol: https://github.com/raspberrypi/pico-sdk/blob/master/src/common/pico_usb_reset_interface_headers/include/pico/usb_reset_interface.h

use rp235x_hal::reboot::{reboot, RebootArch, RebootKind};
use usb_device::{
    class_prelude::*,
    control::{Recipient, RequestType},
    device::UsbDevice,
};

/// Match the ROM / Pico SDK serial so picotool can track the board across reboot.
pub fn serial_number() -> heapless::String<16> {
    use core::fmt::Write;
    let chip = rp235x_hal::rom_data::sys_info_api::chip_info()
        .expect("chip info unavailable")
        .expect("chip info unsupported");
    let mut serial = heapless::String::new();
    write!(serial, "{:08X}{:08X}", chip.wafer_id, chip.device_id).unwrap();
    serial
}

pub struct ResetInterface {
    interface: InterfaceNumber,
}

impl ResetInterface {
    pub fn new<B: UsbBus>(bus: &UsbBusAllocator<B>) -> Self {
        Self {
            interface: bus.interface(),
        }
    }
}

impl<B: UsbBus> UsbClass<B> for ResetInterface {
    fn get_configuration_descriptors(
        &self,
        writer: &mut DescriptorWriter,
    ) -> usb_device::Result<()> {
        writer.interface(self.interface, 0xff, 0x00, 0x01)
    }

    fn control_out(&mut self, transfer: ControlOut<B>) {
        let request = transfer.request();
        if request.request_type != RequestType::Class
            || request.recipient != Recipient::Interface
            || request.index != u16::from(u8::from(self.interface))
            || request.length != 0
        {
            return;
        }
        let kind = match request.request {
            1 => RebootKind::BootSel {
                // Only the interface-disable bits are used; no host-selected GPIO.
                msd_disabled: request.value & 1 != 0,
                picoboot_disabled: request.value & 2 != 0,
            },
            2 if request.value == 0 => RebootKind::Normal,
            _ => return,
        };
        if transfer.accept().is_ok() {
            // The ROM schedules the reset with a 500 ms delay.
            reboot(kind, RebootArch::Normal);
        }
    }
}

/// Keeps the reset interface in every USB poll, including debug writes.
pub struct UsbControl<'a, B: UsbBus> {
    active: Option<(UsbDevice<'a, B>, ResetInterface)>,
}

impl<'a, B: UsbBus> UsbControl<'a, B> {
    pub fn new(active: Option<(UsbDevice<'a, B>, ResetInterface)>) -> Self {
        Self { active }
    }

    #[cfg(feature = "debug")]
    pub fn poll(&mut self, serial: &mut Option<usbd_serial::SerialPort<'_, B>>) {
        if let (Some((device, reset)), Some(serial)) = (&mut self.active, serial) {
            device.poll(&mut [serial, reset]);
        }
    }

    #[cfg(not(feature = "debug"))]
    pub fn poll(&mut self) {
        if let Some((device, reset)) = &mut self.active {
            device.poll(&mut [reset]);
        }
    }
}
