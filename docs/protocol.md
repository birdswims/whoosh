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

Each offered file is `u32` id, `u64` size, `u16` name length, name, `u16` MIME length, MIME. Strings are UTF-8 and the length prefixes are little-endian.

After Accept, the client opens one unidirectional stream per file:

```text
u32 file id | u64 size | file bytes | 32-byte BLAKE3
```

Up to four of those streams are active at once. Reads use 1 MiB buffers. The QUIC connection uses BBR, an 8 MiB stream window, and a 32 MiB connection window. BLAKE3 is updated while the bytes are read, so a video is not loaded into memory and is not read twice.

The receiver writes `.<name>.partial`, checks the hash, then renames the file into place. A mismatch deletes the partial file.

mDNS service type: `_whoosh._udp.local.` TXT keys are `n` (display name), `v` (`1`), and `fp` (fingerprint hex).
