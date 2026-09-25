//! Device-to-device secure messages layered on a UKEY2 next-protocol secret.
//!
//! Sequence numbers start at 0 and are incremented before each encrypt and
//! before each decrypt check, so the first message on a direction is 1.
//! `DeviceToDeviceMessage.message` is field 1 and `sequence_number` is field 2.

use aes::Aes256;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use prost::Message;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;
type HmacSha256 = Hmac<Sha256>;

const D2D_SALT: [u8; 32] = [
    0x82, 0xAA, 0x55, 0xA0, 0xD3, 0x97, 0xF8, 0x83, 0x46, 0xCA, 0x1C, 0xEE, 0x8D, 0x39, 0x09, 0xB9,
    0x5F, 0x13, 0xFA, 0x7D, 0xEB, 0x1D, 0x4A, 0xB3, 0x83, 0x76, 0xB8, 0x25, 0x6D, 0xA8, 0x55, 0x10,
];

const SIG_SCHEME: i32 = 1;
const ENC_SCHEME: i32 = 2;
const D2D_TYPE: i32 = 13;
const D2D_VERSION: i32 = 1;

#[derive(Clone, PartialEq, Message)]
struct SecureMessage {
    #[prost(bytes = "vec", optional, tag = "1")]
    header_and_body: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    signature: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct Header {
    #[prost(int32, optional, tag = "1")]
    signature_scheme: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    encryption_scheme: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "5")]
    iv: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "6")]
    public_metadata: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct HeaderAndBody {
    #[prost(bytes = "vec", optional, tag = "1")]
    header: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    body: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct GcmMetadata {
    #[prost(int32, optional, tag = "1")]
    metadata_type: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    version: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct DeviceToDeviceMessage {
    #[prost(bytes = "vec", optional, tag = "1")]
    message: Option<Vec<u8>>,
    #[prost(int32, optional, tag = "2")]
    sequence_number: Option<i32>,
}

#[derive(Clone)]
struct Direction {
    enc: [u8; 32],
    sig: [u8; 32],
}

pub struct SecureChannel {
    encode: Direction,
    decode: Direction,
    encode_seq: i32,
    decode_seq: i32,
    metadata: Vec<u8>,
}

impl SecureChannel {
    pub fn from_next_secret(next_secret: &[u8; 32], client: bool) -> Result<Self> {
        let client_key = purpose_key(next_secret, &D2D_SALT, b"client")?;
        let server_key = purpose_key(next_secret, &D2D_SALT, b"server")?;
        let (encode_master, decode_master) = if client {
            (client_key, server_key)
        } else {
            (server_key, client_key)
        };
        let secure_salt = sha256(b"SecureMessage");
        debug_assert_eq!(
            secure_salt,
            [
                0xBF, 0x9D, 0x2A, 0x53, 0xC6, 0x36, 0x16, 0xD7, 0x5D, 0xB0, 0xA7, 0x16, 0x5B, 0x91,
                0xC1, 0xEF, 0x73, 0xE5, 0x37, 0xF2, 0x42, 0x74, 0x05, 0xFA, 0x23, 0x61, 0x0A, 0x4B,
                0xE6, 0x57, 0x64, 0x2E,
            ]
        );
        Ok(Self {
            encode: Direction {
                enc: purpose_key(&encode_master, &secure_salt, b"ENC:2")?,
                sig: purpose_key(&encode_master, &secure_salt, b"SIG:1")?,
            },
            decode: Direction {
                enc: purpose_key(&decode_master, &secure_salt, b"ENC:2")?,
                sig: purpose_key(&decode_master, &secure_salt, b"SIG:1")?,
            },
            encode_seq: 0,
            decode_seq: 0,
            metadata: GcmMetadata {
                metadata_type: Some(D2D_TYPE),
                version: Some(D2D_VERSION),
            }
            .encode_to_vec(),
        })
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.encode_seq = self
            .encode_seq
            .checked_add(1)
            .ok_or_else(|| Error::crypto("sequence overflow"))?;
        let body = DeviceToDeviceMessage {
            message: Some(plaintext.to_vec()),
            sequence_number: Some(self.encode_seq),
        }
        .encode_to_vec();
        let mut iv = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut iv);
        let ciphertext = Aes256CbcEnc::new_from_slices(&self.encode.enc, &iv)
            .map_err(|error| Error::crypto(error.to_string()))?
            .encrypt_padded_vec_mut::<Pkcs7>(&body);
        let header = Header {
            signature_scheme: Some(SIG_SCHEME),
            encryption_scheme: Some(ENC_SCHEME),
            iv: Some(iv.to_vec()),
            public_metadata: Some(self.metadata.clone()),
        }
        .encode_to_vec();
        let header_and_body = HeaderAndBody {
            header: Some(header),
            body: Some(ciphertext),
        }
        .encode_to_vec();
        let signature = hmac(&self.encode.sig, &header_and_body)?;
        Ok(SecureMessage {
            header_and_body: Some(header_and_body),
            signature: Some(signature),
        }
        .encode_to_vec())
    }

    pub fn decrypt(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        let secure =
            SecureMessage::decode(bytes).map_err(|error| Error::crypto(error.to_string()))?;
        let header_and_body = secure
            .header_and_body
            .ok_or_else(|| Error::crypto("missing header"))?;
        let signature = secure
            .signature
            .ok_or_else(|| Error::crypto("missing signature"))?;
        verify_hmac(&self.decode.sig, &header_and_body, &signature)?;
        let wrapped = HeaderAndBody::decode(header_and_body.as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        let header = Header::decode(wrapped.header.unwrap_or_default().as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        if header.signature_scheme != Some(SIG_SCHEME)
            || header.encryption_scheme != Some(ENC_SCHEME)
        {
            return Err(Error::crypto("unexpected secure message scheme"));
        }
        let metadata = GcmMetadata::decode(header.public_metadata.unwrap_or_default().as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        if metadata.metadata_type != Some(D2D_TYPE) {
            return Err(Error::crypto("unexpected secure metadata"));
        }
        let iv = header.iv.unwrap_or_default();
        if iv.len() != 16 {
            return Err(Error::crypto("bad iv"));
        }
        let plain = Aes256CbcDec::new_from_slices(&self.decode.enc, &iv)
            .map_err(|error| Error::crypto(error.to_string()))?
            .decrypt_padded_vec_mut::<Pkcs7>(&wrapped.body.unwrap_or_default())
            .map_err(|_| Error::crypto("decrypt"))?;
        let message = DeviceToDeviceMessage::decode(plain.as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        self.decode_seq = self
            .decode_seq
            .checked_add(1)
            .ok_or_else(|| Error::crypto("sequence overflow"))?;
        if message.sequence_number != Some(self.decode_seq) {
            return Err(Error::crypto("bad sequence number"));
        }
        message
            .message
            .ok_or_else(|| Error::crypto("empty device message"))
    }
}

fn purpose_key(ikm: &[u8], salt: &[u8], info: &[u8]) -> Result<[u8; 32]> {
    let expander = hkdf::Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    expander
        .expand(info, &mut out)
        .map_err(|_| Error::crypto("hkdf"))?;
    Ok(out)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    sha2::Sha256::digest(data).into()
}

fn hmac(key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|error| Error::crypto(error.to_string()))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn verify_hmac(key: &[u8], data: &[u8], signature: &[u8]) -> Result<()> {
    let mut mac =
        HmacSha256::new_from_slice(key).map_err(|error| Error::crypto(error.to_string()))?;
    mac.update(data);
    mac.verify_slice(signature)
        .map_err(|_| Error::crypto("bad signature"))
}

#[cfg(test)]
mod tests {
    use super::SecureChannel;

    #[test]
    fn rejects_a_tampered_ciphertext_and_a_replay() {
        let mut sender = SecureChannel::from_next_secret(&[9u8; 32], true).unwrap();
        let mut receiver = SecureChannel::from_next_secret(&[9u8; 32], false).unwrap();
        let mut message = sender.encrypt(b"file").unwrap();
        assert_eq!(receiver.decrypt(&message).unwrap(), b"file");
        message[20] ^= 0x5A;
        assert!(receiver.decrypt(&message).is_err());
        let again = sender.encrypt(b"file").unwrap();
        assert_eq!(receiver.decrypt(&again).unwrap(), b"file");
        assert!(receiver.decrypt(&again).is_err());
    }
}
