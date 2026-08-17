# Security model

NFiDB is designed for a trusted home or studio LAN. It is not hardened for public Wi-Fi, a hostile shared subnet, or direct internet exposure.

## Implemented controls

- A fresh session UUID, PIN, QR secret, and token are created for every host process run.
- The six-digit PIN and 256-bit QR secret expire after ten minutes while unpaired.
- Secrets are compared in constant time; QR secrets and access tokens are stored server-side as SHA-256 digests.
- Access tokens are random 256-bit base64url strings. They are not placed in URLs; WebSocket authentication uses an HttpOnly `SameSite=Strict` cookie.
- Invalid binary packets are discarded before native input injection.
- Only one active WebRTC peer is retained. Disconnect/peer failure releases all synthetic contacts.
- The app has no account, telemetry, analytics, database, STUN/TURN service, or cloud relay.
- Static responses set `nosniff` and a no-referrer policy.

## Deliberate limitations

The bootstrap web server is HTTP. WebRTC media and DataChannel traffic are encrypted by DTLS-SRTP after signaling, but an active attacker on the same LAN could observe or modify the initial HTTP pairing/signaling exchange. A PIN reduces accidental connections; it does not make hostile Wi-Fi safe.

Run NFiDB only on a Windows network marked **Private**. Allow its firewall rule on Private profiles only. Do not port-forward the server. Avoid guest Wi-Fi, hotel Wi-Fi, or networks with unknown peers. If the numeric address is unreachable, inspect client isolation before disabling security controls.

## Credential exposure and logs

Normal logs do not print PINs, QR secrets, access tokens, request bodies, cookies, or headers. Headless diagnostic mode deliberately prints the PIN so automated tests can pair; do not publish that output while its process is running. QR URLs shown in the host UI contain the QR secret and must be treated like the PIN.

## Reporting

Do not include a live PIN, QR URL, packet capture, or token in a public issue. Report security concerns privately to the repository owner until a formal security policy/contact is configured.
