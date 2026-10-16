# Native-player streaming (experimental 0.13)

Enable `streaming` without default features for a codec-free engine. This API is
independent of the terminal protocol, rendering, playback devices and application
library models. `Server::start` accepts an allowed source root, `Resource` entries,
an application JSON catalog and a caller-generated 128-bit hex bearer token.
Resources derive opaque IDs from relative paths; absolute/traversal paths and
external symlinks are rejected. The server rechecks confinement when serving.

The server binds only `127.0.0.1` on an allocated port. `Session` serializes its
version, port and token for an SSH command-channel bootstrap. The caller owns the
service lifetime; dropping the server cancels workers with bounded I/O timeouts.
SCREEN's headless adapter closes it on SSH stdin EOF.

Endpoints: `/v1/catalog`, `/v1/clock`, `/v1/media/ID`. Authenticate using the
bootstrap token in a query or Bearer header. File delivery supports GET, HEAD and
single open-ended/suffix HTTP byte ranges, preserving original bytes. Eight
workers, 64 KiB copy buffers, 16 KiB headers and 16 MiB catalogs bound resources.
No codec compatibility or transcoding is implied by successful file delivery.

`ssh::Endpoint::connect` supervises an OpenSSH service process and adds a local
forward through that authenticated master. `connect_via_master` borrows an
existing control socket. On close, only the connection's own forward and command
are cancelled; a borrowed master is preserved. Remote executable and arguments
are individually quoted. Bootstrap reads are limited to 16 KiB and 60 seconds.

The OpenSSH adapter is for desktop hosts. A native tvOS client supplies its own
embedded SSH implementation, receives the same `Session`, forwards the loopback
service and gives the resulting HTTP URL to its player. This module does not
require or claim that tvOS offers an OpenSSH executable.

Applications keep channel schedules, browsing models, metadata and selection
policy outside KIT. The SCREEN experimental branch is the first consumer.
