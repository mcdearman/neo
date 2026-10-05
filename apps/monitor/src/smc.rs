//! Reads Apple's System Management Controller, for fan speeds and a few
//! temperatures that IOKit's thermal sensors do not cover. No admin rights
//! are needed to read.

use std::ffi::{c_char, c_void};

type MachPort = u32;
type KernReturn = i32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> MachPort;
    fn IOServiceOpen(service: MachPort, owning_task: MachPort, kind: u32, connection: *mut MachPort) -> KernReturn;
    fn IOServiceClose(connection: MachPort) -> KernReturn;
    fn IOObjectRelease(object: MachPort) -> KernReturn;
    /// This process's task port, which `mach_task_self()` reads in C.
    static mach_task_self_: MachPort;
    fn IOConnectCallStructMethod(connection: MachPort, selector: u32, input: *const c_void, input_size: usize, output: *mut c_void, output_size: *mut usize) -> KernReturn;
}

/// `SMCKeyData_t` from Apple's PowerManagement sources: 80 bytes.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KeyData {
    key: u32,
    version: [u8; 6],
    limits: [u32; 4],
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
    /// The key-info fields are a nested struct in C, padded to 12 bytes.
    _pad: [u8; 3],
    result: u8,
    status: u8,
    command: u8,
    data32: u32,
    bytes: [u8; 32],
}

const HANDLE_EVENT: u32 = 2;
const READ_KEY: u8 = 5;
const KEY_INFO: u8 = 9;

fn four(code: &str) -> u32 {
    code.bytes().take(4).fold(0, |acc, b| (acc << 8) | b as u32)
}

/// An open connection to the controller.
pub struct Smc(MachPort);

impl Smc {
    pub fn open() -> Option<Self> {
        // SAFETY: plain IOKit calls; the service object is released once the
        // connection is open, and the connection is closed on drop.
        unsafe {
            let service = IOServiceGetMatchingService(0, IOServiceMatching(c"AppleSMC".as_ptr()));
            if service == 0 {
                return None;
            }
            let mut connection = 0;
            let status = IOServiceOpen(service, mach_task_self_, 0, &mut connection);
            IOObjectRelease(service);
            (status == 0).then_some(Smc(connection))
        }
    }

    fn call(&self, input: &KeyData) -> Option<KeyData> {
        let mut output = KeyData::default();
        let mut size = size_of::<KeyData>();
        // SAFETY: both structs are the 80 bytes the controller expects.
        let status = unsafe { IOConnectCallStructMethod(self.0, HANDLE_EVENT, (input as *const KeyData).cast(), size_of::<KeyData>(), (&raw mut output).cast(), &mut size) };
        (status == 0 && output.result == 0).then_some(output)
    }

    /// Reads a key as a number, whatever numeric type the controller stores it in.
    pub fn number(&self, key: &str) -> Option<f32> {
        let info = self.call(&KeyData { key: four(key), command: KEY_INFO, ..Default::default() })?;
        let value = self.call(&KeyData { key: four(key), data_size: info.data_size, command: READ_KEY, ..Default::default() })?;
        let b = value.bytes;
        let kind = info.data_type.to_be_bytes();
        Some(match (&kind, info.data_size) {
            // Apple Silicon stores floats little-endian.
            (b"flt ", 4) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            // Intel Macs use big-endian fixed point: 14.2 for fans, 8.8 signed for temperatures.
            (b"fpe2", 2) => u16::from_be_bytes([b[0], b[1]]) as f32 / 4.0,
            (b"sp78", 2) => i16::from_be_bytes([b[0], b[1]]) as f32 / 256.0,
            (b"ui8 ", 1) => b[0] as f32,
            (b"ui16", 2) => u16::from_be_bytes([b[0], b[1]]) as f32,
            (b"ui32", 4) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f32,
            _ => return None,
        })
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: the connection was opened by `open` and is closed once.
        unsafe {
            IOServiceClose(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_matches_the_controllers_layout() {
        assert_eq!(size_of::<KeyData>(), 80);
        assert_eq!(std::mem::offset_of!(KeyData, data_size), 28);
        assert_eq!(std::mem::offset_of!(KeyData, command), 42);
        assert_eq!(std::mem::offset_of!(KeyData, bytes), 48);
        assert_eq!(four("FNum"), 0x464E_756D);
    }
}
