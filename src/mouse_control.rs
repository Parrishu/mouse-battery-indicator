use crate::lang;
use crate::lang::Key::*;

use libc::{c_char, c_int, c_void};
use std::ffi::{CStr, CString};
use std::ptr::NonNull;
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
const HIDPP_SW_ID: u8 = 0x0b;
const HIDPP_DIRECT_INDEX: u8 = 0xff;
const HIDPP_FEATURE_DEVICE_NAME: u16 = 0x0005;
const HIDPP_FEATURE_BATTERY_STATUS: u16 = 0x1000;
const HIDPP_FEATURE_BATTERY_VOLTAGE: u16 = 0x1001;
const HIDPP_FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
const HIDPP_FEATURE_ADC_MEASUREMENT: u16 = 0x1f20;
const HIDPP_REPLY_TIMEOUT: Duration = Duration::from_millis(1500);
static HIDAPI_INITIALIZED: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Copy)]
struct BatteryFeature {
    feature_id: u16,
    feature_index: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HidppError {
    DeviceUnavailable,
    Rejected,
    Timeout,
    Transport,
}

struct HidDevice(NonNull<c_void>);

impl HidDevice {
    unsafe fn open_path(path: *const c_char) -> Option<Self> {
        NonNull::new(unsafe { hid_open_path(path) }).map(Self)
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

impl Drop for HidDevice {
    fn drop(&mut self) {
        unsafe { hid_close(self.0.as_ptr()) };
    }
}

struct HidEnumeration(NonNull<HidDeviceInfo>);

impl HidEnumeration {
    fn new(vendor_id: u16, product_id: u16) -> Option<Self> {
        NonNull::new(unsafe { hid_enumerate(vendor_id, product_id) }).map(Self)
    }

    fn as_ptr(&self) -> *mut HidDeviceInfo {
        self.0.as_ptr()
    }
}

impl Drop for HidEnumeration {
    fn drop(&mut self) {
        unsafe { hid_free_enumeration(self.0.as_ptr()) };
    }
}

pub struct CachedMouse {
    path: CString,
    device_index: u8,
    battery_feature: BatteryFeature,
    product_name: String,
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
            if let Some(mouse) = HidDevice::open_path(cached.path.as_ptr()) {
                let battery = read_battery(
                    mouse.as_ptr(),
                    cached.device_index,
                    Some(cached.battery_feature),
                );
                if let Some((battery, _)) = battery {
                    return Some(Device {
                        product_name: cached.product_name.clone(),
                        battery,
                    });
                }
            }
            *cache = None;
        }

        let Some(devices) = HidEnumeration::new(LOGITECH_VENDOR_ID, 0) else {
            return None;
        };

        let mut device_info = devices.as_ptr();
        let mut result = None;
        while !device_info.is_null() && result.is_none() {
            let info = &*device_info;
            let is_receiver = is_receiver_product(info.product_id);

            if info.vendor_id == LOGITECH_VENDOR_ID
                && info.usage_page == HIDPP_USAGE_PAGE
                && !info.path.is_null()
            {
                log::trace!(
                    "Probing Logitech HID++ interface (PID 0x{:04x}, usage 0x{:04x})",
                    info.product_id,
                    info.usage
                );
                if let Some(mouse) = HidDevice::open_path(info.path) {
                    if is_receiver {
                        result = probe_receiver_slots(mouse.as_ptr(), info.path, cache);
                    } else {
                        result = cache_probe(
                            probe_device(mouse.as_ptr(), HIDPP_DIRECT_INDEX),
                            info.path,
                            HIDPP_DIRECT_INDEX,
                            cache,
                        );
                    }

                    if result.is_some() {
                        log::debug!(
                            "Found Logitech HID++ battery device on PID 0x{:04x}",
                            info.product_id
                        );
                    }
                }
            }
            device_info = (*device_info).next;
        }

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
            product_name: device.product_name.clone(),
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
        if let Some(probe) = unsafe { probe_device(mouse, device_index) } {
            return unsafe { cache_probe(Some(probe), path, device_index, cache) };
        }
    }
    None
}

unsafe fn probe_device(
    mouse: *mut c_void,
    device_index: u8,
) -> Option<(Device, BatteryFeature)> {
    let (battery, battery_feature) = unsafe { read_battery(mouse, device_index, None)? };
    let product_name =
        product_name_or_fallback(unsafe { read_firmware_name(mouse, device_index) });

    Some((
        Device {
            product_name,
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

        for feature_id in [
            HIDPP_FEATURE_UNIFIED_BATTERY,
            HIDPP_FEATURE_BATTERY_STATUS,
            HIDPP_FEATURE_BATTERY_VOLTAGE,
            HIDPP_FEATURE_ADC_MEASUREMENT,
        ] {
            let feature_index = match feature_index(mouse, device_index, feature_id) {
                Ok(Some(feature_index)) => feature_index,
                Ok(None) | Err(HidppError::Rejected) => continue,
                Err(
                    HidppError::DeviceUnavailable | HidppError::Timeout | HidppError::Transport,
                ) => return None,
            };
            let feature = BatteryFeature {
                feature_id,
                feature_index,
            };

            if feature_id == HIDPP_FEATURE_UNIFIED_BATTERY
                && let Ok(capabilities) =
                    hidpp_request(mouse, device_index, feature_index, 0x00, 0, 0)
                && capabilities[5] & 0x02 == 0
            {
                continue;
            }

            if let Some(battery) = battery_for_feature(mouse, device_index, feature) {
                return Some((battery, feature));
            }
        }
        None
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
        )
        .ok()?
    };
    battery_from_report(feature.feature_id, &status)
}

unsafe fn feature_index(
    mouse: *mut c_void,
    device_index: u8,
    feature_id: u16,
) -> Result<Option<u8>, HidppError> {
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
    Ok((response[4] != 0xff).then_some(response[4]))
}

unsafe fn read_firmware_name(mouse: *mut c_void, device_index: u8) -> Option<String> {
    let feature = unsafe { feature_index(mouse, device_index, HIDPP_FEATURE_DEVICE_NAME) }
        .ok()??;
    let count = unsafe { hidpp_request(mouse, device_index, feature, 0x00, 0, 0) }.ok()?;
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
            )
            .ok()?
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
) -> Result<[u8; HIDPP_LONG_REPORT_LEN], HidppError> {
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
        return Err(HidppError::Transport);
    }

    let deadline = Instant::now() + HIDPP_REPLY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            log::trace!(
                "HID++ response timed out for device index 0x{device_index:02x}, feature 0x{feature:02x}, function 0x{function:x}"
            );
            return Err(HidppError::Timeout);
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
            return Err(HidppError::Timeout);
        }
        if bytes as usize == response.len()
            && response[0] == HIDPP_LONG_REPORT
            && response[1] == device_index
            && response[2] == feature
            && response[3] == ((function << 4) | HIDPP_SW_ID)
        {
            return Ok(response);
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
            return Err(if response[2] == 0x8f {
                HidppError::DeviceUnavailable
            } else {
                HidppError::Rejected
            });
        }
    }
}

fn battery_from_report(feature_id: u16, report: &[u8; HIDPP_LONG_REPORT_LEN]) -> Option<HscBattery> {
    let voltage_mv;
    let mut level;
    let status = match feature_id {
        HIDPP_FEATURE_UNIFIED_BATTERY | HIDPP_FEATURE_BATTERY_STATUS => {
            level = c_int::from(report[4]);
            if level > 100 {
                return None;
            }
            voltage_mv = -1;
            if feature_id == HIDPP_FEATURE_BATTERY_STATUS
                && report[6] == 3
                && level == 0
            {
                level = 100;
            }
            match (feature_id, report[6]) {
                (HIDPP_FEATURE_UNIFIED_BATTERY, 1 | 2)
                | (HIDPP_FEATURE_BATTERY_STATUS, 1 | 2 | 4) => BatteryStatus::Charging,
                (HIDPP_FEATURE_UNIFIED_BATTERY, 0 | 3)
                | (HIDPP_FEATURE_BATTERY_STATUS, 0 | 3) => BatteryStatus::Available,
                _ => BatteryStatus::Unavailable,
            }
        }
        HIDPP_FEATURE_BATTERY_VOLTAGE | HIDPP_FEATURE_ADC_MEASUREMENT => {
            voltage_mv = (c_int::from(report[4]) << 8) | c_int::from(report[5]);
            if !(1000..=5000).contains(&voltage_mv) {
                return None;
            }
            level = percentage_from_voltage(voltage_mv);
            let status = if feature_id == HIDPP_FEATURE_BATTERY_VOLTAGE {
                if report[6] & 0x80 == 0 {
                    BatteryStatus::Available
                } else {
                    match report[6] & 0x07 {
                        0 => BatteryStatus::Charging,
                        1 => BatteryStatus::Available,
                        7 => BatteryStatus::Unavailable,
                        _ => BatteryStatus::Available,
                    }
                }
            } else {
                match report[6] & 0x0e {
                    0x02 => BatteryStatus::Charging,
                    0x04 => BatteryStatus::Available,
                    0x08 => BatteryStatus::Unavailable,
                    _ => BatteryStatus::Available,
                }
            };
            status
        }
        _ => return None,
    };

    Some(HscBattery {
        level_percent: level.clamp(0, 100),
        status,
        voltage_mv,
        time_to_full_min: -1,
        time_to_empty_min: -1,
    })
}

fn product_name_or_fallback(name: Option<String>) -> String {
    name.filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Logitech HID++ device".to_string())
}

fn percentage_from_voltage(voltage_mv: c_int) -> c_int {
    const CURVE: [(c_int, c_int); 13] = [
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3811, 50),
        (3778, 40),
        (3751, 30),
        (3717, 20),
        (3671, 10),
        (3646, 5),
        (3579, 2),
        (3500, 0),
    ];

    if voltage_mv >= CURVE[0].0 {
        return 100;
    }

    for pair in CURVE.windows(2) {
        let (upper_voltage, upper_percent) = pair[0];
        let (lower_voltage, lower_percent) = pair[1];
        if voltage_mv >= lower_voltage {
            let voltage_delta = upper_voltage - lower_voltage;
            let percent_delta = upper_percent - lower_percent;
            return lower_percent
                + ((voltage_mv - lower_voltage) * percent_delta + voltage_delta / 2)
                    / voltage_delta;
        }
    }

    0
}

fn is_receiver_product(product_id: u16) -> bool {
    (0xc500..=0xc5ff).contains(&product_id)
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
    fn battery_status_report_decodes_charge_state() {
        let mut report = [0; HIDPP_LONG_REPORT_LEN];
        report[4] = 45;
        report[6] = 4;

        let battery = battery_from_report(HIDPP_FEATURE_BATTERY_STATUS, &report).unwrap();
        assert_eq!(battery.level_percent, 45);
        assert_eq!(battery.status, BatteryStatus::Charging);
    }

    #[test]
    fn voltage_battery_report_converts_millivolts_to_percentage() {
        let mut report = [0; HIDPP_LONG_REPORT_LEN];
        report[4] = 0x0f;
        report[5] = 0x0e;

        let battery = battery_from_report(HIDPP_FEATURE_BATTERY_VOLTAGE, &report).unwrap();
        assert_eq!(battery.voltage_mv, 3854);
        assert!((50..=60).contains(&battery.level_percent));
    }

    #[test]
    fn receiver_product_family_is_not_limited_to_known_models() {
        assert!(is_receiver_product(0xc547));
        assert!(is_receiver_product(0xc548));
        assert!(!is_receiver_product(0xc09b));
    }

    #[test]
    fn any_hidpp_firmware_name_is_displayed() {
        assert_eq!(
            product_name_or_fallback(Some("G502 X".to_string())),
            "G502 X"
        );
        assert_eq!(product_name_or_fallback(None), "Logitech HID++ device");
    }

    #[test]
    fn voltage_curve_has_stable_endpoints_and_interpolates() {
        assert_eq!(percentage_from_voltage(4200), 100);
        assert_eq!(percentage_from_voltage(3500), 0);
        assert_eq!(percentage_from_voltage(3420), 0);
        assert_eq!(percentage_from_voltage(3811), 50);
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
