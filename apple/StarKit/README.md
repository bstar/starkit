# STAR/KIT Apple transport — experimental 0.1

Swift package for native Apple clients of KIT's existing streaming protocol.
Supports tvOS 17, iOS 16 and macOS 13 or later. The Rust crate and its public APIs
are unchanged. This is a new, separately versioned Swift package.

`SSHProfile` owns the public endpoint configuration and quotes every remote
argument. `SSHCredential` supplies a password or an unencrypted OpenSSH Ed25519 /
P-256 PEM private key. The consumer owns Keychain storage and host-key enrollment.
Host-key validation is mandatory; an empty pin reports the server fingerprint,
while a changed pin fails. No accept-all trust mode is provided.

`await SSHMovieConnection.connect(profile:credential:)` establishes one native
SSH session, keeps SCREEN's headless command input open, validates the bounded
bootstrap and opens a loopback-only HTTP forward. All HTTP requests multiplex
through that same SSH socket. `connection.session.url(path:)` provides catalog,
clock and media URLs; it never exposes a remote filesystem path. Call
`await connection.close()` to close the listener, command, forwards and SSH socket.

The transport uses Apple's SwiftNIO SSH and Network.framework implementation.
Forwarding is capped at sixteen connections, with NIO backpressure between both
ends. The service bootstrap is capped at 16 KiB and 60 seconds; authentication
and TCP connection have bounded timeouts. Source bytes pass through unchanged.
Catalog domain rules, TV playlists and media codecs belong to consuming apps.

The glue handler adapts Apple's Apache-2.0-licensed NIOSSHClient example; its
original copyright header and license are retained. APIs are experimental and
physical tvOS network/device validation is distinct from desktop integration
proofs. See `../../../starscreen-tvos` via the STAR workspace for the first app.
