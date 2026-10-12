# ShareLocal — Architecture

ShareLocal is an AirDrop-style, LAN-only file transfer app. There is no cloud
backend, no account, and no external database. Every byte of file data travels
directly from sender to receiver over the local network.

```
        Device A (sender)                      Device B (receiver)
   ┌──────────────────────┐               ┌──────────────────────┐
   │  React UI (Tauri)    │               │  React UI (Tauri)    │
   │        │ invoke      │               │        ▲ events      │
   │  ┌─────▼──────────┐  │               │  ┌─────┴──────────┐  │
   │  │  Rust core     │  │  1. mDNS      │  │  Rust core     │  │
   │  │  • discovery   │◄─┼───────────────┼──┤  • discovery   │  │
   │  │  • ws client   │──┼──2. WS offer──┼─►│  • axum server │  │
   │  │  • http sender │──┼──3. HTTP PUT──┼─►│  • file writer │  │
   │  └────────────────┘  │   (streamed)  │  └────────────────┘  │
   └──────────────────────┘               └──────────────────────┘
                                  Wi-Fi / Ethernet LAN
```

## Three planes

| Plane      | Transport                     | Carries                                                   |
|------------|-------------------------------|-----------------------------------------------------------|
| Discovery  | mDNS/Bonjour + UDP multicast  | device id, name, os, port, tls flag, pin-required flag     |
| Control    | WebSocket (`/ws`)             | hello, peer state, offers, accept/reject, progress, cancel |
| Data       | HTTP/1.1 streaming (`PUT/GET`)| raw file bytes, chunked, resumable                          |

File bytes **never** go through the WebSocket. The control plane only carries
small JSON frames.

## Discovery

1. **mDNS/Bonjour** (`mdns-sd`) — advertises `_sharelocal._tcp.local.` with TXT
   records `id`, `name`, `os`, `ver`, `port`, `tls`, `pin`. Simultaneously
   browses for the same service type.
2. **UDP multicast fallback** (`239.255.41.42:41234`) — for networks where mDNS
   is filtered. Periodic `announce` datagrams plus a `probe`/`announce`
   request-response so a newly joined device is seen immediately.

Both feed a single `PeerRegistry`. A peer is `Online` while it has been seen
within `PEER_TTL` (12 s), then `Stale`, then removed at 30 s. The registry
diffs on every mutation and emits `peers-changed` to the UI.

Peers discovered on a different subnet than any local interface are flagged
`same_subnet: false` so the UI can warn about AP/client isolation.

## Control plane (WebSocket)

The sender is the WS *client*; the receiver is the WS *server*. The browser
client is also a WS client of the desktop it scanned.

```
sender                                      receiver
  │ ── hello {device}                    ─► │
  │ ◄─ welcome {device, pin_required}       │
  │ ── offer {files[], total, pin?}      ─► │   creates PendingSession
  │ ◄─ offer-status {session, status}       │   pending | accepted | rejected
  │                                          │   (auto-accept setting or user)
  │ ◄─ offer-status {accepted, token}       │   ← after user taps Accept
  │ ═══ HTTP PUT /api/transfer/... ══════►  │   (data plane, N files)
  │ ── progress {sent, index}            ─► │   optional, for receiver UI
  │ ◄─ transfer-complete / transfer-error   │
```

`cancel` may be sent by either side at any point and aborts the streams.

## Data plane (HTTP)

| Route                                        | Method | Purpose                          |
|----------------------------------------------|--------|----------------------------------|
| `/api/info`                                  | GET    | device identity, unauthenticated |
| `/api/transfer/:sid/:idx`                    | PUT    | streamed upload (send to peer)   |
| `/api/transfer/:sid/:idx`                    | GET    | streamed download (pull, `Range`)|
| `/api/transfer/:sid/:idx/status`             | GET    | bytes already received → resume  |
| `/api/transfer/:sid`                         | DELETE | cancel session, drop `.part`s    |
| `/api/web/session`                           | POST   | redeem a QR token → browser peer |
| `/ws`                                        | GET    | control channel                  |
| `/*`                                         | GET    | the responsive browser UI        |

Uploads stream through a bounded buffer (`STREAM_CHUNK` = 256 KiB) straight into
a `.part` file with `tokio::io::BufWriter`. Nothing larger than the chunk is ever
held in memory, so a 20 GB file costs ~1 MB of RAM. Downloads use
`ReaderStream` over a `tokio::fs::File` with the same property.

**Resume**: bytes land in `<dest>/.sharelocal-part/<session>-<idx>.part`. A
re-`PUT` sends `X-Offset: <n>`; the server truncates to `n` and appends. The
sender learns `n` from `GET …/status`.

**Conflicts**: on finalize the receiver applies the session's
`ConflictPolicy` — `Replace`, `KeepBoth` (`name (2).ext`), or `Cancel`.

## Security model

* The server binds to `0.0.0.0:<port>` but a middleware rejects any peer address
  that is not loopback, RFC1918/CGNAT private, or link-local. There is no
  configuration that exposes it publicly.
* Every session gets a 256-bit `token` (base64url, `OsRng`). Data-plane routes
  require `Authorization: Bearer <token>` and check it in constant time.
* Tokens are single-session, expire after `SESSION_TTL`, and are dropped when
  the session finishes, is cancelled, or times out.
* An optional 6-digit **transfer PIN** is verified on the offer, in constant
  time, before a session is created.
* Incoming transfers require explicit user acceptance unless *auto-accept* is on.
* Every incoming relative path is sanitised (`transfer::paths`): no absolute
  paths, no `..`, no drive letters, no NUL/control bytes, no reserved Windows
  names; the canonicalised result must remain inside the download root.
* No route ever takes a filesystem path from the network. Peers can only write
  into the download root and only read files the local user explicitly offered.
* TLS (`rcgen` self-signed + `axum-server`) is available as an opt-in setting.

## Process / task layout (Rust)

```
main
├─ tauri runtime ──► UI events, tray, commands
└─ tokio runtime
   ├─ axum server task          (accepts inbound sessions)
   ├─ mdns advertise + browse   (2 tasks)
   ├─ udp fallback rx + tx      (2 tasks)
   ├─ peer reaper (1 s tick)    (TTL sweep, emits peers-changed)
   ├─ network watcher (3 s)     (IP change → re-announce, reconnect)
   └─ per-transfer tasks        (one send task per session, N files serial)
```

State lives in `AppState` (`Arc`), holding `Settings` (RwLock, persisted to
`settings.json`), `PeerRegistry`, `SessionStore`, and a broadcast channel of
`AppEvent` that fans out to both the Tauri UI and every connected WS client.

## Frontend

One React + TypeScript + Vite + Tailwind app serves both surfaces:

* **Desktop** — runs inside Tauri, talks to Rust over `invoke` + `listen`.
* **Browser** — the exact same bundle, served by axum, talks over `fetch` + WS.

`src/platform.ts` picks a `Transport` implementation at boot, so views are
identical on both. Layout is responsive with touch-sized targets, so the
browser build works as the phone client with no separate codebase.
