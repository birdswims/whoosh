# Security

Whoosh moves files on the local network. Treat a peer as untrusted until you have checked it yourself.

- Native transfers run inside QUIC. The receiver shows a certificate fingerprint. Pass `--trust-first` only after that fingerprint matches, and set `--require-pin` when other people can reach the machine.
- Clipboard text and images use that same encrypted QUIC connection. A device can read this computer's clipboard only when its certificate fingerprint is already in `known-peers`, and only after this computer has trusted that device too. Each computer announces the fingerprints it has added. The clipboard control is shown only when both announcements name the other computer. The snapshot stays in memory for the session. It is not written to disk or the log. Quick Share and AirDrop cannot request it. The nearby list adds a Whoosh computer by the fingerprint it advertises. Settings lists these fingerprints and can remove one. Removing one drops every saved address for it, so the next send asks again and clipboard pulls stop.
- Quick Share and AirDrop Everyone mode are proximity protocols. Both sides should look at the device name, and Quick Share also shows a four-digit code derived from the UKEY2 authentication string. The receiver still has to accept the file list.
- File names are a single path component. Names that contain separators, `..`, or Windows device names are rejected. Partial files are removed if a transfer fails.
- Declared sizes are capped (`--max-mib`, default 8192). Quick Share and the native protocol write to disk as bytes arrive. An AirDrop upload is read up to that cap before it is unpacked.
- AirDrop contacts-only mode is not implemented. It depends on an Apple-signed identity, which this project does not include.
- The AirDrop client checks that the peer proved possession of its certificate key. It does not chain that certificate to a public CA, because AirDrop uses an Apple private CA.

Report vulnerabilities privately to the maintainers of `birdswims/whoosh` before opening a public issue.
