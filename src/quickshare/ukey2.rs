//! UKEY2 (P-256, SHA-512 commitment) as used by Nearby Share.
//!
//! The handshake hash for the commitment is SHA-512. HKDF is SHA-256, which is
//! what the UKEY2 reference implementation uses for `AES_256_CBC-HMAC_SHA256`
//! even though the cipher is named P256_SHA512. The next-protocol secret is
//! turned into client and server keys with the D2D salt from that same library.

use p256::ecdh::EphemeralSecret;
use p256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use p256::EncodedPoint;
use prost::Message;
use rand::RngCore;
use sha2::{Digest, Sha256, Sha512};

use crate::error::{Error, Result};
use crate::quickshare::secure::SecureChannel;

const CLIENT_INIT: i32 = 2;
const SERVER_INIT: i32 = 3;
const CLIENT_FINISH: i32 = 4;
const P256_SHA512: i32 = 100;
const EC_P256: i32 = 1;
const VERSION: i32 = 1;
pub const NEXT_PROTOCOL: &str = "AES_256_CBC-HMAC_SHA256";

#[derive(Clone, PartialEq, Message)]
struct Ukey2Message {
    #[prost(int32, optional, tag = "1")]
    message_type: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    message_data: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct CipherCommitment {
    #[prost(int32, optional, tag = "1")]
    handshake_cipher: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    commitment: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct ClientInit {
    #[prost(int32, optional, tag = "1")]
    version: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    random: Option<Vec<u8>>,
    #[prost(message, repeated, tag = "3")]
    cipher_commitments: Vec<CipherCommitment>,
    #[prost(string, optional, tag = "4")]
    next_protocol: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct ServerInit {
    #[prost(int32, optional, tag = "1")]
    version: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    random: Option<Vec<u8>>,
    #[prost(int32, optional, tag = "3")]
    handshake_cipher: Option<i32>,
    #[prost(bytes = "vec", optional, tag = "4")]
    public_key: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct ClientFinished {
    #[prost(bytes = "vec", optional, tag = "1")]
    public_key: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct EcPoint {
    #[prost(bytes = "vec", optional, tag = "1")]
    x: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    y: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct GenericPublicKey {
    #[prost(int32, optional, tag = "1")]
    key_type: Option<i32>,
    #[prost(message, optional, tag = "2")]
    ec_p256_public_key: Option<EcPoint>,
}

pub struct ClientHandshake {
    secret: EphemeralSecret,
    client_init: Vec<u8>,
    client_finish: Vec<u8>,
}

pub struct ServerHandshake {
    secret: EphemeralSecret,
    client_init: Vec<u8>,
    server_init: Vec<u8>,
    commitment: Vec<u8>,
}

pub struct SharedSecrets {
    pub auth_string: [u8; 32],
    pub next_secret: [u8; 32],
}

impl ClientHandshake {
    pub fn start() -> Result<Self> {
        let secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
        let public_key = encode_public_key(&secret.public_key())?;
        let client_finish = wrap(
            CLIENT_FINISH,
            &ClientFinished {
                public_key: Some(public_key),
            }
            .encode_to_vec(),
        );
        let commitment = Sha512::digest(&client_finish).to_vec();
        let mut random = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let init = ClientInit {
            version: Some(VERSION),
            random: Some(random),
            cipher_commitments: vec![CipherCommitment {
                handshake_cipher: Some(P256_SHA512),
                commitment: Some(commitment),
            }],
            next_protocol: Some(NEXT_PROTOCOL.to_string()),
        };
        Ok(Self {
            secret,
            client_init: wrap(CLIENT_INIT, &init.encode_to_vec()),
            client_finish,
        })
    }

    pub fn client_init(&self) -> &[u8] {
        &self.client_init
    }

    pub fn client_finish(&self) -> &[u8] {
        &self.client_finish
    }

    pub fn finish(self, server_init: &[u8]) -> Result<SharedSecrets> {
        let message = parse_message(server_init, SERVER_INIT)?;
        let init = ServerInit::decode(message.as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        if init.version != Some(VERSION) || init.handshake_cipher != Some(P256_SHA512) {
            return Err(Error::crypto("unsupported server init"));
        }
        let random = init.random.unwrap_or_default();
        if random.len() != 32 {
            return Err(Error::crypto("server random"));
        }
        let peer = decode_public_key(&init.public_key.unwrap_or_default())?;
        let shared = self.secret.diffie_hellman(&peer);
        derive(
            shared.raw_secret_bytes().as_slice(),
            &self.client_init,
            server_init,
        )
    }
}

impl ServerHandshake {
    pub fn start(client_init: &[u8]) -> Result<Self> {
        let message = parse_message(client_init, CLIENT_INIT)?;
        let init = ClientInit::decode(message.as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        if init.version != Some(VERSION) {
            return Err(Error::crypto("unsupported ukey2 version"));
        }
        if init.random.as_ref().map(Vec::len) != Some(32) {
            return Err(Error::crypto("client random"));
        }
        if init.next_protocol.as_deref() != Some(NEXT_PROTOCOL) {
            return Err(Error::crypto("unsupported next protocol"));
        }
        let commitment = init
            .cipher_commitments
            .iter()
            .find(|item| item.handshake_cipher == Some(P256_SHA512))
            .and_then(|item| item.commitment.clone())
            .ok_or_else(|| Error::crypto("no p256 commitment"))?;
        if commitment.len() != 64 {
            return Err(Error::crypto("commitment length"));
        }
        let secret = EphemeralSecret::random(&mut rand::rngs::OsRng);
        let public_key = encode_public_key(&secret.public_key())?;
        let mut random = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let server_init = wrap(
            SERVER_INIT,
            &ServerInit {
                version: Some(VERSION),
                random: Some(random),
                handshake_cipher: Some(P256_SHA512),
                public_key: Some(public_key),
            }
            .encode_to_vec(),
        );
        Ok(Self {
            secret,
            client_init: client_init.to_vec(),
            server_init,
            commitment,
        })
    }

    pub fn server_init(&self) -> &[u8] {
        &self.server_init
    }

    pub fn finish(self, client_finish: &[u8]) -> Result<SharedSecrets> {
        let actual = Sha512::digest(client_finish);
        if actual.as_slice() != self.commitment {
            return Err(Error::crypto("ukey2 commitment mismatch"));
        }
        let message = parse_message(client_finish, CLIENT_FINISH)?;
        let finished = ClientFinished::decode(message.as_slice())
            .map_err(|error| Error::crypto(error.to_string()))?;
        let peer = decode_public_key(&finished.public_key.unwrap_or_default())?;
        let shared = self.secret.diffie_hellman(&peer);
        derive(
            shared.raw_secret_bytes().as_slice(),
            &self.client_init,
            &self.server_init,
        )
    }
}

impl SharedSecrets {
    /// Four digit code shown by Quick Share so both people can compare it.
    pub fn pin(&self) -> String {
        pin_from_auth(&self.auth_string)
    }

    pub fn client_channel(&self) -> Result<SecureChannel> {
        SecureChannel::from_next_secret(&self.next_secret, true)
    }

    pub fn server_channel(&self) -> Result<SecureChannel> {
        SecureChannel::from_next_secret(&self.next_secret, false)
    }
}

pub fn pin_from_auth(token: &[u8]) -> String {
    let mut hash = 0u32;
    let mut multiplier = 1u32;
    for &byte in token {
        hash = (hash + u32::from(byte) * multiplier) % 9973;
        multiplier = (multiplier * 31) % 9973;
    }
    format!("{hash:04}")
}

fn derive(shared_x: &[u8], client_init: &[u8], server_init: &[u8]) -> Result<SharedSecrets> {
    if shared_x.len() != 32 {
        return Err(Error::crypto("unexpected shared secret length"));
    }
    let mut info = Vec::with_capacity(client_init.len() + server_init.len());
    info.extend_from_slice(client_init);
    info.extend_from_slice(server_init);
    let auth = hkdf(shared_x, b"UKEY2 v1 auth", &info)?;
    let next = hkdf(shared_x, b"UKEY2 v1 next", &info)?;
    Ok(SharedSecrets {
        auth_string: auth,
        next_secret: next,
    })
}

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8]) -> Result<[u8; 32]> {
    let expander = hkdf::Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    expander
        .expand(info, &mut out)
        .map_err(|_| Error::crypto("hkdf expand"))?;
    Ok(out)
}

fn wrap(message_type: i32, data: &[u8]) -> Vec<u8> {
    Ukey2Message {
        message_type: Some(message_type),
        message_data: Some(data.to_vec()),
    }
    .encode_to_vec()
}

fn parse_message(bytes: &[u8], expect: i32) -> Result<Vec<u8>> {
    let message = Ukey2Message::decode(bytes).map_err(|error| Error::crypto(error.to_string()))?;
    if message.message_type != Some(expect) {
        return Err(Error::crypto("unexpected ukey2 message"));
    }
    message
        .message_data
        .ok_or_else(|| Error::crypto("empty ukey2 message"))
}

fn encode_public_key(key: &p256::PublicKey) -> Result<Vec<u8>> {
    let point = key.to_encoded_point(false);
    let x = point.x().ok_or_else(|| Error::crypto("missing x"))?;
    let y = point.y().ok_or_else(|| Error::crypto("missing y"))?;
    let mut x_bytes = [0u8; 32];
    let mut y_bytes = [0u8; 32];
    x_bytes.copy_from_slice(x);
    y_bytes.copy_from_slice(y);
    Ok(GenericPublicKey {
        key_type: Some(EC_P256),
        ec_p256_public_key: Some(EcPoint {
            x: Some(twos_complement(&x_bytes)),
            y: Some(twos_complement(&y_bytes)),
        }),
    }
    .encode_to_vec())
}

fn decode_public_key(bytes: &[u8]) -> Result<p256::PublicKey> {
    let key = GenericPublicKey::decode(bytes).map_err(|error| Error::crypto(error.to_string()))?;
    if key.key_type != Some(EC_P256) {
        return Err(Error::crypto("public key is not p256"));
    }
    let point = key
        .ec_p256_public_key
        .ok_or_else(|| Error::crypto("missing p256 key"))?;
    let x = parse_twos(&point.x.unwrap_or_default())?;
    let y = parse_twos(&point.y.unwrap_or_default())?;
    let encoded = EncodedPoint::from_affine_coordinates(
        &p256::FieldBytes::from(x),
        &p256::FieldBytes::from(y),
        false,
    );
    p256::PublicKey::from_encoded_point(&encoded)
        .into_option()
        .ok_or_else(|| Error::crypto("public key is not on the curve"))
}

fn twos_complement(coord: &[u8; 32]) -> Vec<u8> {
    let mut start = 0;
    while start < 31 && coord[start] == 0 {
        start += 1;
    }
    if coord[start] & 0x80 != 0 {
        let mut out = Vec::with_capacity(33 - start);
        out.push(0);
        out.extend_from_slice(&coord[start..]);
        out
    } else {
        coord[start..].to_vec()
    }
}

fn parse_twos(bytes: &[u8]) -> Result<[u8; 32]> {
    if bytes.is_empty() || bytes.len() > 33 || bytes[0] & 0x80 != 0 {
        return Err(Error::crypto("bad coordinate"));
    }
    if bytes.len() == 33 && bytes[0] != 0 {
        return Err(Error::crypto("bad coordinate"));
    }
    let mut out = [0u8; 32];
    if bytes.len() == 33 {
        out.copy_from_slice(&bytes[1..]);
    } else {
        out[32 - bytes.len()..].copy_from_slice(bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{pin_from_auth, twos_complement, ClientHandshake, ServerHandshake};

    #[test]
    fn both_sides_derive_the_same_secret_and_can_exchange() {
        let client = ClientHandshake::start().unwrap();
        let init = client.client_init().to_vec();
        let client_finish = client.client_finish().to_vec();
        let server = ServerHandshake::start(&init).unwrap();
        let server_init = server.server_init().to_vec();
        let client_secret = client.finish(&server_init).unwrap();
        let server_secret = server.finish(&client_finish).unwrap();
        assert_eq!(client_secret.auth_string, server_secret.auth_string);
        assert_eq!(client_secret.next_secret, server_secret.next_secret);
        assert_eq!(client_secret.pin().len(), 4);
        let mut client_channel = client_secret.client_channel().unwrap();
        let mut server_channel = server_secret.server_channel().unwrap();
        let encrypted = client_channel.encrypt(b"intro").unwrap();
        assert_eq!(server_channel.decrypt(&encrypted).unwrap(), b"intro");
        let reply = server_channel.encrypt(b"ok").unwrap();
        assert_eq!(client_channel.decrypt(&reply).unwrap(), b"ok");
    }

    #[test]
    fn tampered_client_finish_fails_the_commitment() {
        let client = ClientHandshake::start().unwrap();
        let server = ServerHandshake::start(client.client_init()).unwrap();
        let mut finish = client.client_finish().to_vec();
        let last = finish.len() - 1;
        finish[last] ^= 0x01;
        assert!(server.finish(&finish).is_err());
    }

    #[test]
    fn pin_is_four_digits() {
        assert_eq!(pin_from_auth(&[0, 1, 2, 3]).len(), 4);
        assert_eq!(pin_from_auth(&[0, 1, 2, 3]), pin_from_auth(&[0, 1, 2, 3]));
    }

    #[test]
    fn high_bit_coordinates_gain_a_sign_byte() {
        let mut coord = [0u8; 32];
        coord[0] = 0x80;
        let encoded = twos_complement(&coord);
        assert_eq!(encoded[0], 0);
        assert_eq!(encoded[1], 0x80);
        assert_eq!(encoded.len(), 33);
    }
}
