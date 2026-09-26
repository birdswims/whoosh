# Whoosh

Whoosh sends files, photos, and videos between macOS, Windows, and Linux. Computers running Whoosh talk to each other over QUIC. The same receiver also speaks Android Quick Share on the local Wi-Fi network and Apple AirDrop's Discover, Ask, and Upload calls.

```text
whoosh receive --name "Harry's Mac"
whoosh send --to 192.168.1.20:45823 --trust-first photo.jpg clip.mp4
```

## Install

Install a stable Rust toolchain, then:

```bash
cargo install --path .
whoosh --help
```

`cargo test --all-targets` runs the suite. Continuous integration runs that on Linux, macOS, and Windows.

## Receive

```bash
whoosh receive --dir ~/Whoosh --name "Harry's Mac"
```

This listens for all three protocols until you press Ctrl-C. On macOS it registers them with the system mDNS responder. Quick Share is announced on the LAN interface. An Android phone can list it while the Quick Share sheet is open, on that same Wi-Fi, set to Everyone. The iPhone share sheet does not list this service.

| Flag | Effect |
| --- | --- |
| `--yes` | Accept transfers without a prompt |
| `--require-pin` | Print a pin and require it for native sends |
| `--sort-media` | Store photos, videos, and audio in their own folders |
| `--no-native`, `--no-quickshare`, `--no-airdrop` | Turn one listener off |
| `--max-mib` | Largest accepted file, default 8192 MiB |

A native sender must pass `--trust-first` the first time, after comparing the fingerprint printed by the receiver. Later sends to that address use the saved fingerprint in `~/.config/whoosh/known-peers`.

## Send

```bash
# Another Whoosh receiver. QUIC, streamed, parallel files.
whoosh send --to 127.0.0.1:45823 --pin 1234 --trust-first a.jpg b.mp4

# Android Quick Share receiver on the same Wi-Fi.
whoosh quickshare --to 192.168.1.30:12345 vacation.mp4

# AirDrop receiver. HTTPS. Add --http only for a lab listener.
whoosh airdrop --to 192.168.1.40:8770 photo.jpg

whoosh discover --seconds 5
```

`host:port` skips discovery. A bare name is looked up over mDNS for about three seconds.

## What is fast

The native path is the one built for speed:

- QUIC with BBR and multi-megabyte flow-control windows
- one stream per file, up to four at once, so a photo does not wait behind a video
- 1 MiB reads and BLAKE3 computed while the file is read
- files land in `.<name>.partial` and are renamed after the hash matches

Quick Share and AirDrop are single TCP sessions with their own framing. Use them to reach a phone. Use native Whoosh between computers.

The wire format is in [docs/protocol.md](docs/protocol.md).

## Phone compatibility

| Peer | What works | What does not |
| --- | --- | --- |
| Another Whoosh on macOS, Windows, or Linux | QUIC send and receive on the LAN | A public relay. This is a local transfer. |
| Android Quick Share | Same-Wi-Fi receive and send. mDNS type `_FC9F5ED42C8A._tcp`. UKEY2, then encrypted file frames. On macOS the record is registered on the LAN interface, and Whoosh also broadcasts the `FE2C` Bluetooth packet. Open the Quick Share sheet and set it to Everyone. | Wi-Fi Direct. |
| Apple AirDrop | Discover, Ask, and Upload over HTTPS, plus `_airdrop._tcp` on AWDL. On macOS, Whoosh also registers its own `_companion-link._tcp` row under `--name`, at `whoosh-<port>.local` on the Wi-Fi address. That is the service an iPhone lists. The Mac's computer name is left alone. | Contacts-only mode. It needs an Apple-signed identity, which is not in this project. The Mac's own row is still `rapportd` and saves into Downloads. Rapport pair-verify is not implemented, so a phone that only speaks Rapport may show the Whoosh row and then fail the send. Windows and Linux do not have AWDL. |

Quick Share's four-digit code is derived from the UKEY2 authentication string. Compare it on both screens before accepting. AirDrop Everyone mode is a ten-minute choice on the Apple device. The Whoosh receiver still asks before it writes.

File names are one path component. `..`, slashes, and Windows device names are rejected. A failed transfer deletes its partial file.

## Development

```bash
cargo test --all-targets
cargo fmt --check
```

The tests transfer photos, videos, and empty files over the native protocol, a full Quick Share session (including decline, traversal, and oversize), and AirDrop over HTTP and HTTPS. They do not require a phone.

## Credits

The Quick Share record layout follows the public Nearby Share description and Google's UKEY2 specification (Apache-2.0). AirDrop's Discover, Ask, and Upload behavior follows the published research on that protocol. This repository is an independent implementation and does not copy those projects.

## License

Apache-2.0. See [LICENSE](LICENSE).
