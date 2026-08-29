# Security model

NFiDB is designed for a trusted home or studio LAN. It is not hardened for public Wi-Fi, a hostile shared subnet, or direct internet exposure.

## Implemented controls

- A fresh session UUID, PIN, QR secret, and token are created for every host process run and every manual/automatic credential rotation.
- The six-digit PIN and 256-bit QR secret expire after ten minutes while unpaired.
- Secrets are compared in constant time; QR secrets and access tokens are stored server-side as SHA-256 digests.
- Access tokens are random 256-bit base64url strings. They are not placed in URLs; WebSocket authentication uses an HttpOnly `SameSite=Strict` cookie.
- Invalid binary packets are discarded before native input injection; key/text fields and commands are length- and enum-bounded.
- Only one active WebRTC peer is retained. Disconnect/peer failure releases all synthetic contacts.
- Resetting the PIN/QR invalidates the active token, closes the WebRTC peer and authenticated WebSocket, and releases all synthetic contacts.
- File APIs require the same paired HttpOnly cookie; mutation requests enforce a matching origin when browsers send one. Credential reset/disconnect removes unfinished session uploads and streamed downloads recheck session identity between blocks.
- The desktop host exposes only canonical regular files selected into a temporary Outbox. Its paths are never sent to Safari, and manual or completed-download queue cleanup cannot delete the source file. Automatic cleanup requires a paired request and a successfully completed whole-file response.
- iPad uploads accept only sanitized leaf names, are size/chunk bounded, verify each chunk's SHA-256, stage under NFiDB-owned configuration storage, use collision-free Inbox names, and clean abandoned NFiDB `.part` files at startup.
- The app has no account, telemetry, analytics, database, STUN/TURN service, or cloud relay.
- Static responses set `nosniff` and a no-referrer policy.

## Deliberate limitations

The bootstrap web server is HTTP. WebRTC media and DataChannel traffic are encrypted by DTLS-SRTP after signaling, but an active attacker on the same LAN could observe or modify the initial HTTP pairing/signaling exchange. A PIN reduces accidental connections; it does not make hostile Wi-Fi safe.

Run NFiDB only on a trusted private LAN. On Windows, allow its firewall rule on **Private** profiles only. On either platform, do not port-forward the server. Avoid guest Wi-Fi, hotel Wi-Fi, or networks with unknown peers. If the numeric address is unreachable, inspect client isolation before disabling security controls.

## Credential exposure and logs

Normal logs do not print PINs, QR secrets, access tokens, request bodies, cookies, or headers. Headless diagnostic mode deliberately prints the PIN so automated tests can pair; do not publish that output while its process is running. QR URLs shown in the host UI contain the QR secret and must be treated like the PIN.

Detailed diagnostic exports stay on the host under `%APPDATA%\NFiDB\diagnostics\` on Windows or `~/Library/Application Support/NFiDB/diagnostics/` on macOS until the user moves or deletes them. They deliberately exclude credentials and typed text (only event/UTF-8-byte counts are recorded), but they can contain browser/device strings, screen dimensions, performance measurements, configuration, timing history, and the selected ICE candidates' local addresses and ports when Safari exposes them. These addresses are necessary to distinguish physical-LAN, VPN, and virtual-interface routes. Review or redact a detailed report before sharing it publicly. The separate **Copy sanitized diagnostics** action continues to omit client route samples.

File-transfer diagnostics contain sanitized file names, sizes, direction, duration, status, bandwidth, counters, and SHA-256 values. They do not contain file contents or host source paths. Incoming completed files remain in the configured Inbox until the desktop user removes them; Outbox entries exist only for the current host process.

Pairing grants the browser native mouse, keyboard, text, pen/touch, and app-control authority for the selected host. Disconnect or rotate the PIN/QR immediately if the iPad is no longer trusted. On Windows, the secure-attention boundary remains intact: synthetic Ctrl+Alt+Delete cannot enter the protected desktop. On macOS, NFiDB can post input only after the user grants Accessibility permission.

## Reporting

Do not include a live PIN, QR URL, packet capture, or token in a public issue. Report security concerns privately to the repository owner until a formal security policy/contact is configured.
