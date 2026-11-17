use crate::lang;
use crate::lang::Key::*;

use libc::{c_char, c_int, c_void};

#[repr(C)]
pub struct HscBattery {
    pub level_percent: c_int,
    pub status: BatteryStatus,
    pub voltage_mv: c_int,
    pub time_to_full_min: c_int,
    pub time_to_empty_min: c_int,
}

#[repr(C)]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum BatteryStatus {
    #[default]
    Unavailable,
    Charging,
    Available,
    HidError,
    Timeout,
}

#[repr(C)]
struct HidDeviceInfo {
    path: *mut c_char,
    vendor_id: u16,
    product_id: u16,
    serial_number: *mut u16,
    release_number: u16,
    manufacturer_string: *mut u16,
    product_string: *mut u16,
    usage_page: u16,
    usage: u16,
    interface_number: c_int,
    next: *mut HidDeviceInfo,
}

unsafe extern "C" {
    unsafe fn hid_init() -> c_int;
    unsafe fn hid_exit() -> c_int;
    unsafe fn hid_enumerate(vendor_id: u16, product_id: u16) -> *mut HidDeviceInfo;
    unsafe fn hid_free_enumeration(devs: *mut HidDeviceInfo);
    unsafe fn hid_open_path(path: *const c_char) -> *mut c_void;
    unsafe fn hid_close(device: *mut c_void);
    unsafe fn hid_write(device: *mut c_void, data: *const u8, length: usize) -> c_int;
    unsafe fn hid_read_timeout(device: *mut c_void, data: *mut u8, length: usize, milliseconds: c_int) -> c_int;
}

const LOGITECH_VENDOR_ID: u16 = 0x046d;
const MOUSE_PRODUCT_IDS: [u16; 6] =
    [0xc09b, 0xc094, 0xc09d, 0xc09f, 0xc547, 0xc54d];

pub fn query_device() -> Option<Device> {
    unsafe {
        if hid_init() != 0 {
            return None;
        }

        for &product_id in &MOUSE_PRODUCT_IDS {
            let devices = hid_enumerate(LOGITECH_VENDOR_ID, product_id);
            if devices.is_null() {
                continue;
            }

            let mut device_info = devices;
            while !device_info.is_null() {
                let path = (*device_info).path;
                if !path.is_null() {
                    let mouse = hid_open_path(path);
                    if !mouse.is_null() {
                        if let Some(battery) = query_battery(mouse) {
                            hid_close(mouse);
                            hid_free_enumeration(devices);
                            hid_exit();
                            return Some(Device {
                                product_name: "Logitech G PRO X SUPERLIGHT 2".to_string(),
                                battery,
                            });
                        }
                        hid_close(mouse);
                    }
                }
                device_info = (*device_info).next;
            }

            hid_free_enumeration(devices);
        }

        hid_exit();
    }

    None
}

unsafe fn query_battery(mouse: *mut c_void) -> Option<HscBattery> {
    // Discover HID++ 2.0 feature 0x1000 (Unified Battery).
    let mut discover = [0u8; 20];
    discover[0] = 0x10;
    discover[1] = 0xff;
    discover[2] = 0x00;
    discover[3] = 0x01;
    discover[4] = 0x10;
    discover[5] = 0x00;

    if hid_write(mouse, discover.as_ptr(), discover.len()) <= 0 {
        return None;
    }

    let mut response = [0u8; 20];
    if hid_read_timeout(mouse, response.as_mut_ptr(), response.len(), 250) <= 0 {
        return None;
    }

    if response.len() < 7 || response[2] != 0x00 || response[4] == 0xff {
        return None;
    }

    let feature_index = response[4];
    let mut request = [0u8; 20];
    request[0] = 0x10;
    request[1] = 0xff;
    request[2] = feature_index;
    request[3] = 0x10;

    if hid_write(mouse, request.as_ptr(), request.len()) <= 0 {
        return None;
    }

    if hid_read_timeout(mouse, response.as_mut_ptr(), response.len(), 250) <= 0 {
        return None;
    }

    let level = response[4].min(100) as c_int;
    if level == 0 && response[4] == 0xff {
        return None;
    }

    let status_code = response[5];
    let status = match status_code {
        3 | 4 => BatteryStatus::Charging,
        1 => BatteryStatus::Available,
        0 | 2 | 7 => BatteryStatus::Unavailable,
        _ => BatteryStatus::Unavailable,
    };

    Some(HscBattery {
        level_percent: level,
        status,
        voltage_mv: -1,
        time_to_full_min: -1,
        time_to_empty_min: -1,
    })
}

pub struct Device {
    pub product_name: String,
    pub battery: HscBattery,
}

impl Device {
    pub fn status_text(&self) -> Option<&'static str> {
        match self.battery.status {
            BatteryStatus::Charging => Some(lang::t(device_charging)),
            BatteryStatus::Available => None,
            BatteryStatus::Unavailable => Some(lang::t(battery_unavailable)),
            _ => Some(lang::t(device_disconnected)),
        }
    }
}

impl std::fmt::Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.battery.level_percent > 0 {
            write!(
                f,
                "{name}: {battery}%",
                name = self.product_name,
                battery = self.battery.level_percent,
            )?;
        } else {
            write!(f, "{}", self.product_name)?;
        }

        if let Some(status) = self.status_text() {
            write!(f, " {status}")?;
        }

        Ok(())
    }
}
