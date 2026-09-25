//! Bluetooth LE wake packet that makes Android publish Quick Share on mDNS.
//!
//! The 16-bit service UUID is `0xFE2C`. The service data prefix is fixed;
//! the last 10 bytes are random. macOS does not let ordinary apps set that
//! service data, so the daemon advertises over mDNS directly and this packet
//! is available for a platform that can send it.

pub const SERVICE_UUID: u16 = 0xFE2C;
const PREFIX: [u8; 14] = [
    0xFC, 0x12, 0x8E, 0x01, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

pub fn wake_service_data(random: &[u8; 10]) -> Vec<u8> {
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PREFIX);
    data.extend_from_slice(random);
    data
}

#[cfg(test)]
mod tests {
    use super::wake_service_data;

    #[test]
    fn wake_packet_keeps_the_android_prefix() {
        let data = wake_service_data(&[1; 10]);
        assert_eq!(&data[..5], &[0xFC, 0x12, 0x8E, 0x01, 0x42]);
        assert_eq!(data.len(), 24);
        assert_eq!(&data[14..], &[1; 10]);
    }
}
