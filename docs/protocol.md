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

On macOS the daemon registers through `dns-sd`. AirDrop uses `-includeAWDL` so the AWDL address stays in the answer. Quick Share and the native service are registered on the LAN interface only, so a phone is not offered a tunnel address or a proxy hostname. Other operating systems advertise with userspace mDNS and pin that same address for Quick Share and native.

On this Mac, with AirDrop set to Everyone, `sharingd` does not register `_airdrop._tcp` and does not listen on port 8770. The row an iPhone shows is `_companion-link._tcp`. macOS publishes that record under the computer name, on the Mac's own hostname, served by `rapportd`. Whoosh publishes a second `_companion-link._tcp` record. Its instance name is the `--name` value, its host is `whoosh-<port>.local`, and its address is this Mac's Wi-Fi address. The port is Whoosh's AirDrop HTTPS port. The computer name is not changed, and the macOS row still saves into Downloads. Whoosh also registers `_airdrop._tcp` and `_airdrop-alt._tcp` on AWDL. Sharing.framework has `_airdrop-alt._tcp` (`_kBonjourTypeAirDropAlt`). The Discover response name is the `--name` value. Rapport pair-verify is not implemented. A phone that only speaks Rapport can list the Whoosh row and still fail the transfer. The companion-link TXT uses the same key names macOS advertises (`rpFl=0x20000`, `rpVr=715.2`, and random `rpBA` / `rpAD` values). Those values are not copied from the Mac, because copying them can replace the macOS row. Contacts-only mode needs an Apple-signed identity, which this project does not ship.
