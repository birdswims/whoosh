//! Android Quick Share (Nearby Share) over Wi-Fi LAN.
//!
//! A phone on the same network discovers `_FC9F5ED42C8A._tcp`, connects with
//! UKEY2, then sends photos and videos as encrypted payload frames. Bluetooth
//! wake-up is optional and not required while the Quick Share sheet is open.

mod ble;
mod endpoint;
mod secure;
mod session;
mod ukey2;
mod wire;

pub use ble::{wake_service_data, SERVICE_UUID};
pub use endpoint::{
    decode_b64, parse_instance_name, random_endpoint_id, service_instance_name, EndpointInfo,
    DEVICE_LAPTOP, SERVICE_TYPE,
};
pub use session::{accept_one, send_paths, serve, QuickshareConfig, TransferDone};
