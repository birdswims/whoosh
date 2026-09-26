//! Bluetooth advertisements for Quick Share and AirDrop.
//!
//! Android looks for service UUID `FE2C` and the bytes `fc 12 8e`. An iPhone
//! looks for Apple's manufacturer message `05 12` with empty contact hashes,
//! which means Everyone. CoreBluetooth drops both of those fields, so macOS
//! writes them with IOBluetooth's LE advertising commands. The visible name
//! rides in the scan response.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::quickshare::wake_service_data;

pub struct Radio {
    #[cfg(target_os = "macos")]
    worker: Option<Worker>,
}

#[cfg(target_os = "macos")]
struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Radio {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        drop(self.worker.take());
    }
}

impl Radio {
    pub fn start(name: impl Into<String>) -> Self {
        let name = name.into();
        #[cfg(target_os = "macos")]
        {
            return Self {
                worker: Worker::spawn(name),
            };
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = name;
            Self {}
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn quickshare_advertisement(random: &[u8; 10]) -> Vec<u8> {
    let service = wake_service_data(random);
    let mut field = Vec::with_capacity(27);
    field.push(0x16);
    field.extend_from_slice(&crate::quickshare::SERVICE_UUID.to_le_bytes());
    field.extend_from_slice(&service);
    let mut packet = vec![0x02, 0x01, 0x06, field.len() as u8];
    packet.extend_from_slice(&field);
    packet
}

pub fn airdrop_advertisement() -> Vec<u8> {
    let mut body = vec![0x4C, 0x00, 0x05, 0x12];
    body.extend_from_slice(&[0; 8]);
    body.push(0x01);
    body.extend_from_slice(&[0; 8]);
    body.push(0x00);
    let mut packet = vec![0x02, 0x01, 0x06, (1 + body.len()) as u8, 0xFF];
    packet.extend_from_slice(&body);
    packet
}

pub fn name_response(name: &str) -> Vec<u8> {
    let bytes = truncate_utf8(name, 28).as_bytes();
    let mut packet = Vec::with_capacity(2 + bytes.len());
    packet.push((1 + bytes.len()) as u8);
    packet.push(0x09);
    packet.extend_from_slice(&bytes);
    packet
}

fn truncate_utf8(value: &str, max: usize) -> &str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(target_os = "macos")]
impl Worker {
    fn spawn(name: String) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("whoosh-ble".into())
            .spawn(move || broadcast(&name, &flag))
            .ok()?;
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }
}

#[cfg(target_os = "macos")]
fn broadcast(name: &str, stop: &AtomicBool) {
    let Some(controller) = Controller::open() else {
        println!("bluetooth  this Mac has no Bluetooth controller");
        return;
    };
    if let Err(error) = controller.configure() {
        println!("bluetooth  advertisement was not accepted ({error})");
        return;
    }
    println!("bluetooth  broadcasting Quick Share and AirDrop as \"{name}\"");
    let mut tick = 0u8;
    while !stop.load(Ordering::Relaxed) {
        let packet = match tick % 3 {
            0 => {
                let mut random = [0u8; 10];
                rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut random);
                quickshare_advertisement(&random)
            }
            1 => airdrop_advertisement(),
            _ => name_response(name),
        };
        if controller.set_advertisement(&packet).is_err()
            || controller.set_scan_response(&name_response(name)).is_err()
        {
            println!("bluetooth  advertisement stopped");
            break;
        }
        let _ = controller.set_enabled(true);
        tick = tick.wrapping_add(1);
        for _ in 0..4 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = controller.set_enabled(false);
}

#[cfg(target_os = "macos")]
struct Controller {
    object: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
unsafe impl Send for Controller {}

#[cfg(target_os = "macos")]
impl Controller {
    fn open() -> Option<Self> {
        unsafe { open_controller() }
    }

    fn configure(&self) -> std::result::Result<(), i32> {
        unsafe { configure_controller(self.object) }
    }

    fn set_advertisement(&self, packet: &[u8]) -> std::result::Result<(), i32> {
        unsafe {
            set_data(
                self.object,
                b"BluetoothHCILESetAdvertisingData:advertsingData:\0",
                packet,
            )
        }
    }

    fn set_scan_response(&self, packet: &[u8]) -> std::result::Result<(), i32> {
        unsafe {
            set_data(
                self.object,
                b"BluetoothHCILESetScanResponseData:scanResponseData:\0",
                packet,
            )
        }
    }

    fn set_enabled(&self, enabled: bool) -> std::result::Result<(), i32> {
        unsafe { set_enabled(self.object, enabled) }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe fn open_controller() -> Option<Controller> {
    let class_name = b"IOBluetoothHostController\0";
    let class = objc_getClass(class_name.as_ptr().cast());
    if class.is_null() {
        return None;
    }
    let selector = sel_registerName(b"defaultController\0".as_ptr().cast());
    let method = class_getClassMethod(class, selector);
    if method.is_null() {
        return None;
    }
    type Getter =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    let get: Getter = std::mem::transmute(method_getImplementation(method));
    let object = get(class, selector);
    if object.is_null() {
        None
    } else {
        Some(Controller { object })
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe fn configure_controller(object: *mut std::ffi::c_void) -> std::result::Result<(), i32> {
    let class = objc_getClass(b"IOBluetoothHostController\0".as_ptr().cast());
    let selector = sel_registerName(
        b"BluetoothHCILESetAdvertisingParameters:advertisingIntervalMax:advertisingType:ownAddressType:directAddressType:directAddress:advertisingChannelMap:advertisingFilterPolicy:\0"
            .as_ptr()
            .cast(),
    );
    let method = class_getInstanceMethod(class, selector);
    if method.is_null() {
        return Err(-1);
    }
    type SetParams = unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *mut std::ffi::c_void,
        u16,
        u16,
        u8,
        u8,
        u8,
        *const u8,
        u8,
        u8,
    ) -> i32;
    let set: SetParams = std::mem::transmute(method_getImplementation(method));
    let direct = [0u8; 6];
    // 160 * 0.625 ms = 100 ms. Type 2 is scannable so the name in the scan
    // response is delivered. Channels 7 means 37, 38, and 39.
    let status = set(object, selector, 160, 160, 2, 0, 0, direct.as_ptr(), 7, 0);
    if status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe fn set_data(
    object: *mut std::ffi::c_void,
    selector_name: &[u8],
    packet: &[u8],
) -> std::result::Result<(), i32> {
    if packet.is_empty() || packet.len() > 31 {
        return Err(-1);
    }
    let class = objc_getClass(b"IOBluetoothHostController\0".as_ptr().cast());
    let selector = sel_registerName(selector_name.as_ptr().cast());
    let method = class_getInstanceMethod(class, selector);
    if method.is_null() {
        return Err(-1);
    }
    type SetData =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u8, *const u8) -> i32;
    let set: SetData = std::mem::transmute(method_getImplementation(method));
    let status = set(object, selector, packet.len() as u8, packet.as_ptr());
    if status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe fn set_enabled(
    object: *mut std::ffi::c_void,
    enabled: bool,
) -> std::result::Result<(), i32> {
    let class = objc_getClass(b"IOBluetoothHostController\0".as_ptr().cast());
    let selector = sel_registerName(b"BluetoothHCILESetAdvertiseEnable:\0".as_ptr().cast());
    let method = class_getInstanceMethod(class, selector);
    if method.is_null() {
        return Err(-1);
    }
    type SetEnable = unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, u8) -> i32;
    let set: SetEnable = std::mem::transmute(method_getImplementation(method));
    let status = set(object, selector, u8::from(enabled));
    if status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}

#[cfg(target_os = "macos")]
#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn class_getClassMethod(
        class: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn class_getInstanceMethod(
        class: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn method_getImplementation(method: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(target_os = "macos")]
#[link(name = "IOBluetooth", kind = "framework")]
extern "C" {}

#[cfg(test)]
mod tests {
    use super::{airdrop_advertisement, name_response, quickshare_advertisement};

    #[test]
    fn quickshare_advertisement_carries_the_android_prefix() {
        let packet = quickshare_advertisement(&[0xAB; 10]);
        assert!(packet.len() <= 31);
        assert_eq!(&packet[..3], &[0x02, 0x01, 0x06]);
        assert!(packet
            .windows(5)
            .any(|window| window == [0xFC, 0x12, 0x8E, 0x01, 0x42]));
        assert!(packet.windows(2).any(|window| window == [0x2C, 0xFE]));
        assert!(packet.ends_with(&[0xAB; 10]));
    }

    #[test]
    fn airdrop_advertisement_is_the_everyone_beacon() {
        let packet = airdrop_advertisement();
        assert!(packet.len() <= 31);
        assert!(packet
            .windows(4)
            .any(|window| window == [0x4C, 0x00, 0x05, 0x12]));
    }

    #[test]
    fn scan_response_uses_the_whoosh_name() {
        let packet = name_response("Harry's Mac");
        assert_eq!(packet[1], 0x09);
        assert_eq!(&packet[2..], b"Harry's Mac");
        assert!(name_response(&"n".repeat(40)).len() <= 31);
    }
}
