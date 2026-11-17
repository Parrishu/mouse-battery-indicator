use crate::lang;
use crate::lang::Key::*;

use libc::{c_char, c_int, c_void};
use std::ffi::{CStr, CString};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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
    unsafe fn hid_enumerate(vendor_id: u16, product_id: u16) -> *mut HidDeviceInfo;
    unsafe fn hid_free_enumeration(devs: *mut HidDeviceInfo);
    unsafe fn hid_open_path(path: *const c_char) -> *mut c_void;
    unsafe fn hid_close(device: *mut c_void);
    unsafe fn hid_write(device: *mut c_void, data: *const u8, length: usize) -> c_int;
    unsafe fn hid_read_timeout(
        device: *mut c_void,
        data: *mut u8,
        length: usize,
        milliseconds: c_int,
    ) -> c_int;
}

const LOGITECH_VENDOR_ID: u16 = 0x046d;
const HIDPP_USAGE_PAGE: u16 = 0xff00;
const HIDPP_LONG_REPORT: u8 = 0x11;
const HIDPP_LONG_REPORT_LEN: usize = 20;
const HIDPP_SW_ID: u8 = 0x01;
const HIDPP_DIRECT_INDEX: u8 = 0xff;
const HIDPP_FEATURE_DEVICE_NAME: u16 = 0x0005;
const HIDPP_FEATURE_BATTERY_STATUS: u16 = 0x1000;
const HIDPP_FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
const HIDPP_REPLY_TIMEOUT: Duration = Duration::from_millis(500);
const RECEIVER_PRODUCT_IDS: [u16; 2] = [0xc547, 0xc54d];
const DIRECT_MOUSE_PRODUCT_IDS: [u16; 2] = [0xc09b, 0xc09d];
static HIDAPI_INITIALIZED: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Copy)]
struct BatteryFeature {
    feature_id: u16,
    feature_index: u8,
}

pub struct CachedMouse {
    path: CString,
    device_index: u8,
    battery_feature: BatteryFeature,
}

pub fn query_device(cache: &mut Option<CachedMouse>) -> Option<Device> {
    let initialized = *HIDAPI_INITIALIZED.get_or_init(|| unsafe {
        let initialized = hid_init() == 0;
        if !initialized {
            log::error!("Failed to initialize HIDAPI");
        }
        initialized
    });
    if !initialized {
        return None;
    }

    unsafe {
        if let Some(cached) = cache.as_ref() {
            let mouse = hid_open_path(cached.path.as_ptr());
            if !mouse.is_null() {
                let battery = read_battery(
                    mouse,
                    cached.device_index,
                    Some(cached.battery_feature),
                );
                hid_close(mouse);
                if let Some((battery, _)) = battery {
                    return Some(Device {
                        product_name: "Logitech G PRO X SUPERLIGHT 2".to_string(),
                        battery,
                    });
                }
            }
            *cache = None;
        }

        let devices = hid_enumerate(LOGITECH_VENDOR_ID, 0);
        if devices.is_null() {
            return None;
        }

        let mut device_info = devices;
        let mut result = None;
        while !device_info.is_null() && result.is_none() {
            let info = &*device_info;
            let is_direct_mouse = DIRECT_MOUSE_PRODUCT_IDS.contains(&info.product_id);
            let is_receiver = RECEIVER_PRODUCT_IDS.contains(&info.product_id);

            if info.vendor_id == LOGITECH_VENDOR_ID
                && info.usage_page == HIDPP_USAGE_PAGE
                && !info.path.is_null()
            {
                log::trace!(
                    "Probing Logitech HID++ interface (PID 0x{:04x}, usage 0x{:04x})",
                    info.product_id,
                    info.usage
                );
                let mouse = hid_open_path(info.path);
                if !mouse.is_null() {
                    if is_direct_mouse {
                        result = cache_probe(
                            probe_device(mouse, HIDPP_DIRECT_INDEX, true),
                            info.path,
                            HIDPP_DIRECT_INDEX,
                            cache,
                        );
                    } else {
                        if !is_receiver {
                            result = cache_probe(
                                probe_device(mouse, HIDPP_DIRECT_INDEX, false),
                                info.path,
                                HIDPP_DIRECT_INDEX,
                                cache,
                            );
                        }
                        if result.is_none() {
                            result = probe_receiver_slots(mouse, info.path, cache);
                        } else {
                            log::debug!(
                                "Found Superlight 2 on Logitech HID++ PID 0x{:04x}",
                                info.product_id
                            );
                        }
                    }

                    hid_close(mouse);
                }
            }
            device_info = (*device_info).next;
        }

        hid_free_enumeration(devices);
        result
    }
}

unsafe fn cache_probe(
    probe: Option<(Device, BatteryFeature)>,
    path: *const c_char,
    device_index: u8,
    cache: &mut Option<CachedMouse>,
) -> Option<Device> {
    probe.map(|(device, battery_feature)| {
        *cache = Some(CachedMouse {
            path: unsafe { CStr::from_ptr(path) }.to_owned(),
            device_index,
            battery_feature,
        });
        device
    })
}

unsafe fn probe_receiver_slots(
    mouse: *mut c_void,
    path: *const c_char,
    cache: &mut Option<CachedMouse>,
) -> Option<Device> {
    for device_index in 1..=6 {
        if let Some((device, battery_feature)) =
            unsafe { probe_device(mouse, device_index, false) }
        {
            *cache = Some(CachedMouse {
                path: unsafe { CStr::from_ptr(path) }.to_owned(),
                device_index,
                battery_feature,
            });
            return Some(device);
        }
    }
    None
}

unsafe fn probe_device(
    mouse: *mut c_void,
    device_index: u8,
    direct_mouse: bool,
) -> Option<(Device, BatteryFeature)> {
    let (battery, battery_feature) = unsafe { read_battery(mouse, device_index, None)? };
    let name = unsafe { read_firmware_name(mouse, device_index) };
    if let Some(name) = name.as_deref() {
        if !is_superlight_2(name) {
            log::trace!("HID++ device index 0x{device_index:02x} is {name:?}, not the target mouse");
            return None;
        }
    } else if !direct_mouse {
        log::trace!("Could not read the firmware name for HID++ device index 0x{device_index:02x}");
        return None;
    }

    Some((
        Device {
            product_name: "Logitech G PRO X SUPERLIGHT 2".to_string(),
            battery,
        },
        battery_feature,
    ))
}

fn read_battery(
    mouse: *mut c_void,
    device_index: u8,
    cached_feature: Option<BatteryFeature>,
) -> Option<(HscBattery, BatteryFeature)> {
    unsafe {
        if let Some(feature) = cached_feature
            && let Some(battery) = battery_for_feature(mouse, device_index, feature)
        {
            return Some((battery, feature));
        }

        if let Some(feature_index) =
            feature_index(mouse, device_index, HIDPP_FEATURE_UNIFIED_BATTERY)
        {
            let feature = BatteryFeature {
                feature_id: HIDPP_FEATURE_UNIFIED_BATTERY,
                feature_index,
            };
            if let Some(capabilities) =
                hidpp_request(mouse, device_index, feature_index, 0x00, 0, 0)
                && capabilities[5] & 0x02 != 0
                && let Some(battery) = battery_for_feature(mouse, device_index, feature)
            {
                return Some((battery, feature));
            }
        }

        let feature_index =
            feature_index(mouse, device_index, HIDPP_FEATURE_BATTERY_STATUS)?;
        let feature = BatteryFeature {
            feature_id: HIDPP_FEATURE_BATTERY_STATUS,
            feature_index,
        };
        let battery = battery_for_feature(mouse, device_index, feature)?;
        Some((battery, feature))
    }
}

unsafe fn battery_for_feature(
    mouse: *mut c_void,
    device_index: u8,
    feature: BatteryFeature,
) -> Option<HscBattery> {
    let function = if feature.feature_id == HIDPP_FEATURE_UNIFIED_BATTERY {
        0x01
    } else {
        0x00
    };
    let status = unsafe {
        hidpp_request(
            mouse,
            device_index,
            feature.feature_index,
            function,
            0,
            0,
        )?
    };
    battery_from_report(feature.feature_id, &status)
}

unsafe fn feature_index(
    mouse: *mut c_void,
    device_index: u8,
    feature_id: u16,
) -> Option<u8> {
    let response = unsafe {
        hidpp_request(
            mouse,
            device_index,
            0x00,
            0x00,
            (feature_id >> 8) as u8,
            feature_id as u8,
        )?
    };
    (response[4] != 0xff).then_some(response[4])
}

unsafe fn read_firmware_name(mouse: *mut c_void, device_index: u8) -> Option<String> {
    let feature = unsafe { feature_index(mouse, device_index, HIDPP_FEATURE_DEVICE_NAME)? };
    let count = unsafe { hidpp_request(mouse, device_index, feature, 0x00, 0, 0)? };
    let length = usize::from(count[4]);
    if length == 0 || length > 64 {
        return None;
    }

    let mut name = Vec::with_capacity(length);
    while name.len() < length {
        let chunk = unsafe {
            hidpp_request(
                mouse,
                device_index,
                feature,
                0x01,
                name.len() as u8,
                0,
            )?
        };
        let remaining = length - name.len();
        let previous_len = name.len();
        for &byte in chunk[4..].iter().take(remaining) {
            if byte == 0 {
                return None;
            }
            name.push(byte);
        }
        if name.len() == previous_len {
            return None;
        }
    }

    Some(String::from_utf8_lossy(&name).into_owned())
}

unsafe fn hidpp_request(
    mouse: *mut c_void,
    device_index: u8,
    feature: u8,
    function: u8,
    param0: u8,
    param1: u8,
) -> Option<[u8; HIDPP_LONG_REPORT_LEN]> {
    let mut request = [0u8; HIDPP_LONG_REPORT_LEN];
    request[0] = HIDPP_LONG_REPORT;
    request[1] = device_index;
    request[2] = feature;
    request[3] = (function << 4) | HIDPP_SW_ID;
    request[4] = param0;
    request[5] = param1;

    let written = unsafe { hid_write(mouse, request.as_ptr(), request.len()) };
    if written != request.len() as c_int {
        log::trace!(
            "HID++ write failed for device index 0x{device_index:02x}, feature 0x{feature:02x}, function 0x{function:x} (wrote {written} bytes)"
        );
        return None;
    }

    let deadline = Instant::now() + HIDPP_REPLY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            log::trace!(
                "HID++ response timed out for device index 0x{device_index:02x}, feature 0x{feature:02x}, function 0x{function:x}"
            );
            return None;
        }
        let timeout_ms = remaining.as_millis().max(1).min(c_int::MAX as u128) as c_int;
        let mut response = [0u8; HIDPP_LONG_REPORT_LEN];
        let bytes = unsafe {
            hid_read_timeout(
                mouse,
                response.as_mut_ptr(),
                response.len(),
                timeout_ms,
            )
        };
        if bytes <= 0 {
            log::trace!(
                "HID++ response timed out for device index 0x{device_index:02x}, feature 0x{feature:02x}, function 0x{function:x}"
            );
            return None;
        }
        if bytes as usize == response.len()
            && response[0] == HIDPP_LONG_REPORT
            && response[1] == device_index
            && response[2] == feature
            && response[3] == ((function << 4) | HIDPP_SW_ID)
        {
            return Some(response);
        }
        if bytes >= 7
            && response[1] == device_index
            && ((response[2] == 0xff && response[3] == feature)
                || (response[2] == 0x8f && response[3] == feature))
            && response[4] == request[3]
        {
            log::trace!(
                "HID++ error response for device index 0x{device_index:02x}, feature 0x{feature:02x}, function 0x{function:x}"
            );
            return None;
        }
    }
}

fn battery_from_report(feature_id: u16, report: &[u8; HIDPP_LONG_REPORT_LEN]) -> Option<HscBattery> {
    let level = report[4];
    if level > 100 {
        return None;
    }

    let charging = if feature_id == HIDPP_FEATURE_UNIFIED_BATTERY {
        matches!(report[6], 1 | 2)
    } else {
        matches!(report[6], 1 | 2 | 4)
    };
    let valid_status = if feature_id == HIDPP_FEATURE_UNIFIED_BATTERY {
        matches!(report[6], 0..=3)
    } else {
        matches!(report[6], 0..=4)
    };

    Some(HscBattery {
        level_percent: c_int::from(level),
        status: if !valid_status {
            BatteryStatus::Unavailable
        } else if charging {
            BatteryStatus::Charging
        } else {
            BatteryStatus::Available
        },
        voltage_mv: -1,
        time_to_full_min: -1,
        time_to_empty_min: -1,
    })
}

fn is_superlight_2(name: &str) -> bool {
    let normalized: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "proxsuperlight2"
            | "gproxsuperlight2"
            | "logitechproxsuperlight2"
            | "logitechgproxsuperlight2"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_battery_report_decodes_percent_and_charge_state() {
        let mut report = [0; HIDPP_LONG_REPORT_LEN];
        report[4] = 72;
        report[6] = 2;

        let battery = battery_from_report(HIDPP_FEATURE_UNIFIED_BATTERY, &report).unwrap();
        assert_eq!(battery.level_percent, 72);
        assert_eq!(battery.status, BatteryStatus::Charging);
    }

    #[test]
    fn battery_report_rejects_non_percentage_values() {
        let mut report = [0; HIDPP_LONG_REPORT_LEN];
        report[4] = 0xff;

        assert!(battery_from_report(HIDPP_FEATURE_UNIFIED_BATTERY, &report).is_none());
    }

    #[test]
    fn device_name_match_is_limited_to_superlight_two() {
        assert!(is_superlight_2("PRO X SUPERLIGHT 2"));
        assert!(!is_superlight_2("PRO X SUPERLIGHT"));
        assert!(!is_superlight_2("PRO X SUPERLIGHT 2 DEX"));
    }
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
