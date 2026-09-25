//! Protobuf messages for Nearby Connections and Nearby Share / Quick Share.
//!
//! Field numbers match the Apache-2.0 protos published with Google's Nearby and
//! UKEY2 projects. Unknown fields from newer phones are ignored on decode.

use prost::Message;

pub const MAX_FRAME: usize = 8 * 1024 * 1024;

pub mod conn {
    use super::Message;

    pub const CONNECTION_REQUEST: i32 = 1;
    pub const CONNECTION_RESPONSE: i32 = 2;
    pub const PAYLOAD_TRANSFER: i32 = 3;
    pub const KEEP_ALIVE: i32 = 5;
    pub const DISCONNECTION: i32 = 6;

    pub const WIFI_LAN: i32 = 5;
    pub const ACCEPT: i32 = 1;
    pub const STATUS_OK: i32 = 0;

    pub const PACKET_DATA: i32 = 1;
    pub const PACKET_ACK: i32 = 3;
    pub const BYTES: i32 = 1;
    pub const FILE: i32 = 2;
    pub const LAST_CHUNK: i32 = 1;

    #[derive(Clone, PartialEq, Message)]
    pub struct OfflineFrame {
        #[prost(int32, optional, tag = "1")]
        pub version: Option<i32>,
        #[prost(message, optional, tag = "2")]
        pub v1: Option<V1Frame>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct V1Frame {
        #[prost(int32, optional, tag = "1")]
        pub frame_type: Option<i32>,
        #[prost(message, optional, tag = "2")]
        pub connection_request: Option<ConnectionRequestFrame>,
        #[prost(message, optional, tag = "3")]
        pub connection_response: Option<ConnectionResponseFrame>,
        #[prost(message, optional, tag = "4")]
        pub payload_transfer: Option<PayloadTransferFrame>,
        #[prost(message, optional, tag = "6")]
        pub keep_alive: Option<KeepAliveFrame>,
        #[prost(message, optional, tag = "7")]
        pub disconnection: Option<DisconnectionFrame>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ConnectionRequestFrame {
        #[prost(string, optional, tag = "1")]
        pub endpoint_id: Option<String>,
        #[prost(string, optional, tag = "2")]
        pub endpoint_name: Option<String>,
        #[prost(int32, optional, tag = "4")]
        pub nonce: Option<i32>,
        #[prost(int32, repeated, packed = "false", tag = "5")]
        pub mediums: Vec<i32>,
        #[prost(bytes = "vec", optional, tag = "6")]
        pub endpoint_info: Option<Vec<u8>>,
        #[prost(int32, optional, tag = "8")]
        pub keep_alive_interval_millis: Option<i32>,
        #[prost(int32, optional, tag = "9")]
        pub keep_alive_timeout_millis: Option<i32>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ConnectionResponseFrame {
        /// Deprecated integer status. `0` is STATUS_OK.
        #[prost(int32, optional, tag = "1")]
        pub status: Option<i32>,
        #[prost(int32, optional, tag = "3")]
        pub response: Option<i32>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct PayloadHeader {
        #[prost(int64, optional, tag = "1")]
        pub id: Option<i64>,
        #[prost(int32, optional, tag = "2")]
        pub payload_type: Option<i32>,
        #[prost(int64, optional, tag = "3")]
        pub total_size: Option<i64>,
        #[prost(string, optional, tag = "5")]
        pub file_name: Option<String>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct PayloadChunk {
        #[prost(int32, optional, tag = "1")]
        pub flags: Option<i32>,
        #[prost(int64, optional, tag = "2")]
        pub offset: Option<i64>,
        #[prost(bytes = "vec", optional, tag = "3")]
        pub body: Option<Vec<u8>>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct PayloadTransferFrame {
        #[prost(int32, optional, tag = "1")]
        pub packet_type: Option<i32>,
        #[prost(message, optional, tag = "2")]
        pub payload_header: Option<PayloadHeader>,
        #[prost(message, optional, tag = "3")]
        pub payload_chunk: Option<PayloadChunk>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct KeepAliveFrame {}

    #[derive(Clone, PartialEq, Message)]
    pub struct DisconnectionFrame {}

    impl OfflineFrame {
        pub fn new(frame_type: i32, v1: V1Frame) -> Self {
            Self {
                version: Some(1),
                v1: Some(V1Frame {
                    frame_type: Some(frame_type),
                    ..v1
                }),
            }
        }

        pub fn connection_response() -> Self {
            Self::new(
                CONNECTION_RESPONSE,
                V1Frame {
                    connection_response: Some(ConnectionResponseFrame {
                        status: Some(STATUS_OK),
                        response: Some(ACCEPT),
                    }),
                    ..V1Frame::empty()
                },
            )
        }

        pub fn keep_alive() -> Self {
            Self::new(
                KEEP_ALIVE,
                V1Frame {
                    keep_alive: Some(KeepAliveFrame {}),
                    ..V1Frame::empty()
                },
            )
        }

        pub fn disconnect() -> Self {
            Self::new(
                DISCONNECTION,
                V1Frame {
                    disconnection: Some(DisconnectionFrame {}),
                    ..V1Frame::empty()
                },
            )
        }

        pub fn payload(transfer: PayloadTransferFrame) -> Self {
            Self::new(
                PAYLOAD_TRANSFER,
                V1Frame {
                    payload_transfer: Some(transfer),
                    ..V1Frame::empty()
                },
            )
        }
    }

    impl V1Frame {
        pub(crate) fn empty() -> Self {
            Self {
                frame_type: None,
                connection_request: None,
                connection_response: None,
                payload_transfer: None,
                keep_alive: None,
                disconnection: None,
            }
        }
    }

    pub fn bytes_payload(id: i64, data: &[u8]) -> [OfflineFrame; 2] {
        let total = data.len() as i64;
        let first = data_frame(id, BYTES, total, 0, Some(data.to_vec()), 0, None);
        let second = data_frame(id, BYTES, total, total, Some(Vec::new()), LAST_CHUNK, None);
        [first, second]
    }

    pub fn file_chunk(
        id: i64,
        total: i64,
        offset: i64,
        body: &[u8],
        last: bool,
        name: &str,
    ) -> OfflineFrame {
        data_frame(
            id,
            FILE,
            total,
            offset,
            Some(body.to_vec()),
            if last { LAST_CHUNK } else { 0 },
            Some(name.to_string()),
        )
    }

    pub fn payload_ack(id: i64, payload_type: i32, total: i64) -> OfflineFrame {
        OfflineFrame::payload(PayloadTransferFrame {
            packet_type: Some(PACKET_ACK),
            payload_header: Some(PayloadHeader {
                id: Some(id),
                payload_type: Some(payload_type),
                total_size: Some(total),
                file_name: None,
            }),
            payload_chunk: None,
        })
    }

    fn data_frame(
        id: i64,
        payload_type: i32,
        total: i64,
        offset: i64,
        body: Option<Vec<u8>>,
        flags: i32,
        name: Option<String>,
    ) -> OfflineFrame {
        OfflineFrame::payload(PayloadTransferFrame {
            packet_type: Some(PACKET_DATA),
            payload_header: Some(PayloadHeader {
                id: Some(id),
                payload_type: Some(payload_type),
                total_size: Some(total),
                file_name: name,
            }),
            payload_chunk: Some(PayloadChunk {
                flags: Some(flags),
                offset: Some(offset),
                body,
            }),
        })
    }
}

pub mod share {
    use super::Message;

    pub const INTRODUCTION: i32 = 1;
    pub const RESPONSE: i32 = 2;
    pub const PAIRED_KEY_ENCRYPTION: i32 = 3;
    pub const PAIRED_KEY_RESULT: i32 = 4;
    pub const CANCEL: i32 = 6;

    pub const ACCEPT: i32 = 1;
    pub const REJECT: i32 = 2;
    pub const NOT_ENOUGH_SPACE: i32 = 3;
    pub const UNABLE: i32 = 3;

    #[derive(Clone, PartialEq, Message)]
    pub struct Frame {
        #[prost(int32, optional, tag = "1")]
        pub version: Option<i32>,
        #[prost(message, optional, tag = "2")]
        pub v1: Option<V1Frame>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct V1Frame {
        #[prost(int32, optional, tag = "1")]
        pub frame_type: Option<i32>,
        #[prost(message, optional, tag = "2")]
        pub introduction: Option<IntroductionFrame>,
        #[prost(message, optional, tag = "3")]
        pub connection_response: Option<ConnectionResponseFrame>,
        #[prost(message, optional, tag = "4")]
        pub paired_key_encryption: Option<PairedKeyEncryptionFrame>,
        #[prost(message, optional, tag = "5")]
        pub paired_key_result: Option<PairedKeyResultFrame>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct FileMetadata {
        #[prost(string, optional, tag = "1")]
        pub name: Option<String>,
        #[prost(int32, optional, tag = "2")]
        pub file_type: Option<i32>,
        #[prost(int64, optional, tag = "3")]
        pub payload_id: Option<i64>,
        #[prost(int64, optional, tag = "4")]
        pub size: Option<i64>,
        #[prost(string, optional, tag = "5")]
        pub mime_type: Option<String>,
        #[prost(int64, optional, tag = "6")]
        pub id: Option<i64>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct TextMetadata {
        #[prost(string, optional, tag = "2")]
        pub text_title: Option<String>,
        #[prost(int32, optional, tag = "3")]
        pub text_type: Option<i32>,
        #[prost(int64, optional, tag = "4")]
        pub payload_id: Option<i64>,
        #[prost(int64, optional, tag = "5")]
        pub size: Option<i64>,
        #[prost(int64, optional, tag = "6")]
        pub id: Option<i64>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct IntroductionFrame {
        #[prost(message, repeated, tag = "1")]
        pub file_metadata: Vec<FileMetadata>,
        #[prost(message, repeated, tag = "2")]
        pub text_metadata: Vec<TextMetadata>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct ConnectionResponseFrame {
        #[prost(int32, optional, tag = "1")]
        pub status: Option<i32>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct PairedKeyEncryptionFrame {
        #[prost(bytes = "vec", optional, tag = "1")]
        pub signed_data: Option<Vec<u8>>,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub secret_id_hash: Option<Vec<u8>>,
    }

    #[derive(Clone, PartialEq, Message)]
    pub struct PairedKeyResultFrame {
        #[prost(int32, optional, tag = "1")]
        pub status: Option<i32>,
    }

    impl Frame {
        pub fn new(frame_type: i32, v1: V1Frame) -> Self {
            Self {
                version: Some(1),
                v1: Some(V1Frame {
                    frame_type: Some(frame_type),
                    ..v1
                }),
            }
        }
    }

    impl V1Frame {
        pub(crate) fn empty() -> Self {
            Self {
                frame_type: None,
                introduction: None,
                connection_response: None,
                paired_key_encryption: None,
                paired_key_result: None,
            }
        }
    }

    impl Frame {
        pub fn is_control(&self) -> bool {
            let Some(v1) = &self.v1 else {
                return false;
            };
            matches!(
                v1.frame_type,
                Some(INTRODUCTION | RESPONSE | PAIRED_KEY_ENCRYPTION | PAIRED_KEY_RESULT | CANCEL)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::conn::{self, OfflineFrame};
    use super::share;
    use prost::Message;

    #[test]
    fn connection_response_roundtrips() {
        let frame = OfflineFrame::connection_response();
        let bytes = frame.encode_to_vec();
        let decoded = OfflineFrame::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded.version, Some(1));
        assert_eq!(
            decoded.v1.unwrap().frame_type,
            Some(conn::CONNECTION_RESPONSE)
        );
    }

    #[test]
    fn bytes_payload_marks_the_second_frame_last() {
        let frames = conn::bytes_payload(7, b"hello");
        let chunk = frames[1]
            .v1
            .as_ref()
            .unwrap()
            .payload_transfer
            .as_ref()
            .unwrap();
        assert_eq!(
            chunk.payload_chunk.as_ref().unwrap().flags,
            Some(conn::LAST_CHUNK)
        );
        assert_eq!(chunk.payload_chunk.as_ref().unwrap().offset, Some(5));
    }

    #[test]
    fn introduction_keeps_file_metadata() {
        let frame = share::Frame::new(
            share::INTRODUCTION,
            share::V1Frame {
                introduction: Some(share::IntroductionFrame {
                    file_metadata: vec![share::FileMetadata {
                        name: Some("a.jpg".into()),
                        file_type: Some(1),
                        payload_id: Some(4),
                        size: Some(3),
                        mime_type: Some("image/jpeg".into()),
                        id: Some(4),
                    }],
                    text_metadata: Vec::new(),
                }),
                ..share::V1Frame::empty()
            },
        );
        let decoded = share::Frame::decode(frame.encode_to_vec().as_slice()).unwrap();
        let file = &decoded.v1.unwrap().introduction.unwrap().file_metadata[0];
        assert_eq!(file.name.as_deref(), Some("a.jpg"));
        assert_eq!(file.payload_id, Some(4));
    }
}
