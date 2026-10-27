//! USB discovery, the GP-GL plan encoder, the cutting driver and the transport interface.
//! Discovery never opens or claims a device and sends nothing; nothing here sends to real
//! hardware yet: the driver runs against the Cameo simulator until the USB transport exists.
//!
//! Firmware/status queries await confirmation of the actual cutter and protocol.
//! [`transport::Transport`] is the interface the driver will talk through; the Cameo simulator
//! (`cutcraft-sim`) implements it today. [`gpgl::encode`] turns a cut plan into the bytes a
//! sender would write.

pub mod driver;
pub mod gpgl;
pub mod transport;
pub mod usb;

use nusb::MaybeFuture;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct InterfaceDescriptor {
    pub number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

/// OS-reported USB identity, not proof of protocol compatibility or cut readiness.
#[derive(Debug, Serialize)]
pub struct DeviceDescriptor {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub interfaces: Vec<InterfaceDescriptor>,
}

/// Enumerate candidate Graphtec/Silhouette devices without sending cutter commands.
///
/// Later-generation devices are reported for identification, not enabled for motion.
pub fn discover() -> Result<Vec<DeviceDescriptor>, nusb::Error> {
    Ok(nusb::list_devices()
        .wait()?
        .filter(|device| matches!(device.vendor_id(), 0x0b4d | 0x3844))
        .map(|device| DeviceDescriptor {
            vendor_id: device.vendor_id(),
            product_id: device.product_id(),
            manufacturer: device.manufacturer_string().map(str::to_owned),
            product: device.product_string().map(str::to_owned),
            interfaces: device
                .interfaces()
                .map(|interface| InterfaceDescriptor {
                    number: interface.interface_number(),
                    class: interface.class(),
                    subclass: interface.subclass(),
                    protocol: interface.protocol(),
                })
                .collect(),
        })
        .collect())
}
