# Native protocol

Whoosh peers use QUIC with ALPN `whoosh/1` and the TLS server name `whoosh`. The certificate is self-signed. Callers pin its SHA-256 fingerprint.

The client opens one bidirectional stream and writes:

1. The 8 bytes `WHOOSH01`.
2. Length-prefixed control messages. The length is a little-endian `u32` and does not include itself.

The server writes length-prefixed control messages on that stream and does not repeat the magic.

| Tag | Name | Body |
| --- | --- | --- |
| 1 | Hello | `u16` name length, UTF-8 name, 16-byte device id, flags (`bit 0` means a pin is required) |
| 2 | Offer | 16-byte transfer id, `u16` pin length, pin, `u32` file count, then each file |
| 3 | Accept | 16-byte transfer id |
| 4 | Reject | 16-byte transfer id, `u16` reason length, UTF-8 reason |
| 5 | Done | 16-byte transfer id |
| 6 | ClipboardPull | 16-byte request id |
| 7 | Clipboard | 16-byte request id, `u32` text length, UTF-8 text, `u32` PNG length, PNG bytes |

ClipboardPull asks for the receiver's current clipboard instead of offering files. The receiver answers with Clipboard, or Reject with the reason `not a trusted device`, when the client certificate's SHA-256 fingerprint is not one it has saved. Text is at most 1 MiB. A non-empty image is a PNG of at most 8 MiB and must begin with the PNG signature. The receiver keeps that snapshot in memory and does not write it to disk. File sends still use Offer and do not present a client certificate. A clipboard pull presents the caller's device certificate. The receiver accepts it only when that fingerprint is already in `known-peers`, from any earlier address. The caller also pins the receiver's fingerprint. Both computers have to trust each other.

Each offered file is `u32` id, `u64` size, `u16` name length, name, `u16` MIME length, MIME. Strings are UTF-8 and the length prefixes are little-endian. File-offer strings use a `u16` length. Clipboard text and PNG bytes use a `u32` length.

After Accept, the client opens one unidirectional stream per file:

```text
u32 file id | u64 size | file bytes | 32-byte BLAKE3
```

Up to four of those streams are active at once. Reads use 1 MiB buffers. The QUIC connection uses BBR, an 8 MiB stream window, and a 32 MiB connection window. BLAKE3 is updated while the bytes are read, so a video is not loaded into memory and is not read twice.

The receiver writes `.<name>.partial`, checks the hash, then renames the file into place. A mismatch deletes the partial file.

mDNS service type: `_whoosh._udp.local.` TXT keys are `n` (display name), `v` (`1`), and `fp` (fingerprint hex).

On macOS the daemon registers through `dns-sd`. AirDrop uses `-includeAWDL` so the AWDL address stays in the answer. Quick Share and the native service are registered on the LAN interface only, so a phone is not offered a tunnel address or a proxy hostname. Other operating systems advertise with userspace mDNS and pin that same address for Quick Share and native.

AirDrop TCP listeners also opt into inbound traffic from restricted interfaces using `SOL_SOCKET/SO_RECV_ANYIF` (`0x1104`) before bind/listen. The option is inherited by accepted sockets. Binding a scoped IPv6 address on `awdl0` and using `-includeAWDL` for DNS-SD do not set this socket option. Its value is defined in [Apple's XNU socket header](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/socket_private.h). The macOS regression test reads the option back from both the listener and an accepted socket; an IPv6 HTTPS test separately verifies the receiver's Discover response. These local tests do not verify visibility in a physical iPhone's picker.

The native macOS receiver uses Companion Link and application-service pairing records. It can also publish `_airdrop._tcp` on port 8770 when its legacy path is active, as observed during live validation; its absence at idle does not mean legacy AirDrop is unsupported. The system receiver is separate from Whoosh. A fabricated companion-link record is insufficient: `SDRapportBrowser` can refuse a node when `accountID` is nil. Whoosh does not publish one, and it does not copy the Mac's `rpBA` or `rpAD`.

The iPhone still browses `_airdrop._tcp`. In `sharingd`, that browser drops a node whose flags lack bit 14 (`0x4000`, the nearby-sharing flag) and drops a node with bit 16 (`0x10000`) because that bit means PIN pair and the device must be discovered over Rapport. Whoosh advertises `16524` (`0x408C`: discover, mixed types, pipelining, and nearby sharing) on one `_airdrop._tcp` instance, including AWDL. The instance is a 12-hex id; the Discover response carries `--name`. Extra named instances and `_airdrop-alt._tcp` registrations were removed after an iOS 27 device displayed the receiver three times. The computer name is not changed, and the macOS row still saves into Downloads. Contacts-only mode needs an Apple-signed identity, which this project does not ship.

Live validation on macOS 27 / iOS 27 confirmed IPv6 TCP connections over `awdl0`, successful TLS and `/Discover` responses, and a visible Whoosh name after enabling AWDL reception and stopping an older concurrent receiver. The capture also contained modern pairing and QUIC service queries; those do not imply that the phone has stopped supporting the legacy HTTPS discovery path. Picker visibility is separate from a completed file-transfer test.

The legacy HTTPS sender’s outgoing AirDrop sockets also enable `SO_RECV_ANYIF`, including sockets bound
to a scoped AWDL address. The interoperable TLS sender presents a self-signed
RSA client identity when requested. This is not an Apple account identity or
an implementation of the newer AirDrop code-pairing protocol.

Legacy sender activity follows preparation, TCP connection, TLS, Discover,
Ask, and Upload separately. Connecting and TLS/Discover each have an 8-second
deadline; Ask has 60 seconds and Upload has 10 minutes. An unanswered Ask
does not establish that the receiver showed an acceptance prompt. The app
no longer tells the user to accept before the request has been attempted.
Socket-option, mutual-TLS, silent-peer timeout, and progress-order tests cover
these behaviors locally.

The macOS GUI sends through `SFOperation` in Sharing.framework, targeting the
exact Bonjour instance, hostname, port, and flags discovered by the engine.
The selected device is passed directly to the transfer service; it does not
invoke a sharing sheet or use the entitlement-restricted system browser.
The OS performs sender authentication and transfer negotiation using the Mac's
AirDrop identity. Whoosh neither extracts account credentials nor changes
entitlements. Framework symbols are resolved at runtime, with an explicit error
if they are absent. These are private APIs and need checking on future macOS
releases.

The sender resumes the system operation after event 2 (preparation), displays
request/transfer progress for events 3/5/6/7, and marks success only on event 9.
Events 4 and 10 end the transfer as cancelled or failed. An acceptance or even
100% byte progress is not reported as delivery. Callback context retention,
cancellation, and a 90-second preparation/acceptance deadline plus a 120-second
transfer inactivity deadline prevent an abandoned operation from holding Send
indefinitely. Discovery updates carry the target metadata together with their
chosen address, preserving the AWDL target when a LAN advertisement arrives.

Live validation on macOS 27 / iPadOS 27.0 confirmed the system direct-send path:
Ask and Upload both returned HTTP 200, macOS reported completion, and the user
confirmed receipt of the test file. An opt-in XCTest then exercised the
integrated sender against the same iPad, confirmed all 32 file bytes and the
completion callback, and verified that the sender released its active state.
The legacy sender reached Discover with
`IsAirDropable=true` but got no Ask response, even with a client certificate.
This is why the GUI uses the system transfer service. The precise missing
legacy-handshake requirement remains undetermined; it is not necessary to
reimplement it for the macOS GUI path.

AirDrop uploads can contain either CPIO newc (`070701`) or POSIX odc (`070707`)
archives. Odc uses octal fields in a 76-byte header and has no newc-style
four-byte padding. Directory entries, including the archive root, are skipped;
regular files retain the existing name validation and size limits. DVZip blocks
use a big-endian 32-bit header whose high bit marks uncompressed data and whose
remaining bits give the block length. Compressed blocks are inflated within the
remaining size budget, and framing/decoding errors are returned to the sender
and printed in the terminal. The test fixture comes from bsdtar, independently
of Whoosh's own archive writer; an HTTPS test exercises chunked Ask and Upload
requests with `Expect: 100-continue`.

AirDrop offers preserve the optional `FileSize` field from each Ask file entry,
including Samsung Quick Share transfers using AirDrop compatibility. A known
zero-byte file is displayed as empty; missing or invalid sizes remain unknown.
Successful uploads print the saved file count and byte count.

Quick Share UKEY2 derives its authentication and next-protocol secrets from
`SHA256(ECDH shared x)`, followed by HKDF-SHA256 with the UKEY2 salts and the
serialized ClientInit + ServerInit transcript. This matches Google's
[KeyAgreementSha256](https://github.com/google/ukey2/blob/master/src/securemessage/src/securemessage/crypto_ops.cc)
and [UKEY2 handshake](https://github.com/google/ukey2/blob/master/src/main/cpp/src/securegcm/ukey2_handshake.cc).
Passing the raw ECDH value to HKDF produces incompatible encryption keys even
though the handshake reaches PIN generation. A fixed derivation vector computed
with Python hashlib/hmac tests this independently of Whoosh-to-Whoosh round trips.
Quick Share session failures are logged at warning level, and successful
receives print the saved file count and byte count.

Windows Quick Share can send non-UTF-8 data in connection-request field 2
(`endpoint_name`), despite its upstream proto declaration as a string. Whoosh
decodes that length-delimited field as bytes so it does not reject the request
before UKEY2. Display names prefer `endpoint_info`, then a decodable endpoint
record or valid text in the legacy field, otherwise `Quick Share device`.
A synthetic binary-field request is tested through the encrypted handshake;
this is separate from verification of a complete transfer from a Windows PC.

Quick Share retains partially read frame prefixes and bodies across `select!`
cancellation. Keepalive ticks and approval decisions must not discard bytes
already consumed from TCP. The regression test interrupts a read at every byte
boundary and checks the following frame as well. Frame-size and control-payload
errors identify the relevant packet limit; offered-file errors report the
declared size and configured limit separately.
