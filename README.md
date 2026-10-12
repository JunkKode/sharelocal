# ShareLocal

Send files straight to nearby devices over your own Wi-Fi or Ethernet.
No cloud, no account, no upload limits — the bytes go from one machine to the
other and nowhere else.

```
ShareLocal
Nearby Devices
  MacBook Pro     Online · Wi-Fi
  Windows PC      Online · Ethernet
  iPhone          Online · Wi-Fi
────────────────────────────────
        Drop files here
              or
  [ Select Files ]  [ Select Folder ]
```

## What it does

* **Finds devices by itself.** mDNS/Bonjour where it works, a small UDP
  broadcast fallback where it does not.
* **Streams, never buffers.** A 20 GB file costs about a megabyte of memory.
* **Keeps folders intact.** Drop a folder and its structure arrives with it.
* **Works on a phone with nothing installed.** Show a QR code, scan it, and the
  phone gets the same interface in its browser.
* **Resumes.** An interrupted transfer picks up from the exact byte the
  receiver already holds.
* **Asks first.** Incoming transfers need your approval unless you turn that
  off, and you can require a PIN.
* **Copies to and from Android phones over USB.** macOS cannot read an
  Android phone on a cable; the *USB* tab can — browse it, copy photos off,
  drag files on. Nothing to install on the phone or the Mac. See
  [Android over USB](#android-over-usb).
* **Shares your Wi-Fi as a QR code.** *Share Wi-Fi* fills in the network
  you are on (looked up in the background, so it is ready when you open it);
  guests scan it with their camera to join. Save it as a PNG or copy the
  image, the password or the details as text. The first time, macOS asks for
  an administrator name and password to read the saved Wi-Fi password — Wi-Fi
  passwords live in the System keychain, which only Apple's own apps may
  unlock with Touch ID. ShareLocal then keeps a copy in your login keychain,
  so from then on it is just Touch ID.
* **Sends text, not just files.** Paste a link, a code or a Wi-Fi password and
  it lands on the other device's clipboard — and as a `.txt` file, so it is not
  lost when the clipboard changes.

## Running it

You need [Node](https://nodejs.org) 20+ and the [Rust toolchain](https://rustup.rs).
Tauri also needs the platform build tools: Xcode Command Line Tools on macOS,
`webkit2gtk` and `libayatana-appindicator` on Linux, and the WebView2 runtime
plus MSVC build tools on Windows.

```bash
npm install
npm run app:build   # build ShareLocal.app
```

Then drag `src-tauri/target/release/bundle/macos/ShareLocal.app` into
`/Applications` and launch it like any other app — no terminal needed after
that. `npm run dmg` packages it for handing to someone else.

For development, with hot reload:

```bash
npm run app
```

Other useful commands:

| Command | What it does |
|---|---|
| `npm run dev` | the web UI alone, on `localhost:5173` |
| `npm run build` | type-check and build the browser bundle |
| `npm test` | front-end unit tests |
| `npm run test:rust` | the Rust suite: unit + end-to-end |
| `npm run headless` | a node with no window, for testing the phone client or running on a headless box |
| `npm run bench` | throughput benchmarks (writes large fixtures) |
| `npm run dmg` | package the built app as a .dmg |
| `npm run clean` | reclaim the Rust build cache (keeps the release build) |
| `npm run clean:all` | reclaim all of `src-tauri/target` |

## How it fits together

Three planes, deliberately separated — see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
for the detail.

| Plane | Transport | Carries |
|---|---|---|
| Discovery | mDNS + UDP multicast | who is nearby |
| Control | WebSocket | offers, accept/decline, progress, cancel |
| Data | HTTP streaming | the file bytes |

File data never travels over the WebSocket, and it never touches a server you
do not own.

```
Device A  ──►  your Wi-Fi / LAN  ──►  Device B
```

## The menu bar

ShareLocal keeps running in the menu bar (system tray) after you close the
window — that is what lets it receive files in the background. Click the icon
for:

* **Open ShareLocal** — bring the window back
* **Pause sharing** — hide this device and refuse incoming transfers
* **Open download folder**
* **Quit ShareLocal**

Closing the window only hides it. Clicking the Dock icon brings it back, and
*Quit* from the menu is what actually stops it.

Turn on **Start with the computer** in Settings and it will be there after a
reboot without touching a terminal.

## Sending text

Files are not always the point. **Send Text** on the desktop, or the text box on
the phone, shares a snippet directly: it travels with the transfer request
rather than as a file, so it arrives the instant it is accepted. The receiving
side shows it with a **Copy** button and also writes it to the download folder
as `shared-text-<timestamp>.txt`.

Snippets are capped at 64 KB — past that it is a file, and the composer says so
instead of failing after you press send. Unlike file contents, shared text *is*
kept in history, because being able to copy it again later is the whole point.

## Sending from a phone

1. On the desktop, press **Connect with QR**.
2. Scan it with the phone's camera.
3. The phone opens ShareLocal in its browser — pick files, take a photo with
   **Camera**, or paste a snippet into the text box.
4. Files and text the desktop sends appear on the phone: files to download,
   text with a **Copy** button.

A phone that joined this way is a *client*, not a server — it has no address to
dial. Sending to it therefore works the other way round: the desktop publishes
the transfer and the phone fetches it. That happens automatically; the device
list looks the same either way.

The QR code carries a one-time token that expires after ten minutes and can be
revoked at any moment. It only resolves to an address on your own network.

## Security

* The file server binds the LAN, and a middleware refuses any connection whose
  source address is not private. There is no setting that exposes it publicly.
* Each transfer gets its own 256-bit bearer token, checked in constant time and
  discarded when the transfer ends.
* An optional numeric PIN gates every incoming offer.
* Every filename a peer sends is sanitised before it is joined to a path:
  no `..`, no absolute paths, no drive letters, no control characters, and the
  resolved destination must still sit inside the download folder.
* A peer can only write into your download folder, and can only read files you
  explicitly offered it. There is no route that takes a filesystem path from
  the network.
* TLS is available (self-signed, generated on the device) for anyone who wants
  the wire encrypted. It is off by default because on a LAN there is no
  authority to vouch for a certificate — the token and PIN are what actually
  authenticate a transfer.

## Privacy

No cloud storage. No analytics. No account. No external tracking. History
records file names, sizes and speeds — never file contents.

## Testing

```bash
npm run test:rust     # 147 Rust tests: unit + end-to-end between two live instances
npm test              # 51 front-end tests
```

The end-to-end tests boot two complete ShareLocal instances in one process and
transfer real files between them over loopback — discovery, WebSocket
negotiation, streamed HTTP upload, conflict handling, resume, PIN checks and
path-traversal attempts all run against the real code.

Throughput benchmarks are kept separate because they write large fixtures:

```bash
cargo test --release --manifest-path src-tauri/Cargo.toml --test perf -- --ignored --nocapture

# a specific size, in megabytes
SHARELOCAL_PERF_MB=20480 cargo test --release --manifest-path src-tauri/Cargo.toml \
  --test perf -- --ignored --nocapture large_file_throughput
```

Measured on an Apple silicon laptop, sender and receiver both on the machine so
the number reflects the pipeline rather than the network:

| Size | Time | Throughput |
|---|---|---|
| 100 MB | 0.21 s | 475 MB/s |
| 1 GB | 1.81 s | 565 MB/s |
| 2 GB | 4.68 s | 437 MB/s |

The 2 GB run also asserts the memory guarantee: resident set went from 10 MB to
18 MB, so the file is genuinely streamed rather than buffered. Sizes above that
need roughly `2 × size` of free disk for the fixture plus the received copy; the
benchmark says so and skips rather than filling the volume.

## Layout

```
src/                 React + TypeScript UI (desktop window and phone browser)
  platform/          Tauri commands vs. the browser's fetch/WebSocket client
  components/        shared presentational pieces
  views/             the four desktop screens and the phone screen
src-tauri/           the Rust core
  src/discovery/     mDNS, UDP fallback, the peer registry
  src/server/        axum: routes, WebSocket control channel, TLS
  src/transfer/      streaming send/receive, sessions, path safety
  tests/             end-to-end suites
docs/ARCHITECTURE.md the design in more detail
```

## Troubleshooting

**Devices do not see each other.** Both must be running ShareLocal, and both
must be on the same network. Some guest networks turn on "client isolation",
which blocks device-to-device traffic entirely — ShareLocal detects that and
says so on the home screen. A phone with no app installed will not appear until
it scans the QR code.

Discovery deliberately ignores virtual interfaces — VPN tunnels (`utun*`),
AirDrop's own radios (`awdl0`, `llw0`) and loopback. They are not the local
network, they appear and disappear constantly, and counting them used to make
the address list churn and repeatedly clear the discovered devices.

**A firewall prompt appeared.** Allow ShareLocal to accept incoming
connections; without that, other devices can see you but cannot send anything.

**`src-tauri/target` is enormous.** That is Rust's build cache, and tens of
gigabytes is normal after a few builds — every dependency is compiled once per
profile, plus an incremental cache. An installed `ShareLocal.app` does not need
any of it. `npm run clean` reclaims the dev cache and keeps the release build;
`npm run clean:all` reclaims everything and makes the next build slow but
correct.

**The QR code does not work.** It expires after ten minutes — show a new one.
The phone must be on the same network as the computer.

**The DMG step fails during `tauri build`.** Tauri's DMG bundler drives Finder
through AppleScript, which needs a GUI session with accessibility permission.
The bundle targets therefore stop at `.app`; run `npm run dmg` for a disk image
built with `hdiutil`, which has no such requirement.

**macOS says the app is from an unidentified developer.** The build is
ad-hoc signed, not notarised. Right-click the app and choose *Open* the first
time, or build it yourself — the source is right here.

**`failed to run 'cargo metadata'`.** Rust is not installed, or it is installed
but not visible to `npm`. `npm run` executes scripts through `/bin/sh`, which
never reads `.zshenv`, `.zshrc` or `.profile` — so a rustup install that works
in your terminal can still be invisible here. The npm scripts go through
`scripts/with-rust.mjs`, which looks in the usual rustup locations and puts
cargo on PATH itself, so this should not happen; if it still does, cargo is
somewhere unusual and setting `CARGO_HOME` will fix it.

## Android over USB

Android phones share their storage over MTP, which macOS does not support —
that is why Finder never shows a plugged-in phone. ShareLocal speaks MTP itself
(through the pure-Rust [`mtp-rs`](https://crates.io/crates/mtp-rs)), so:

1. Plug the phone in with a data cable and unlock it.
2. On the phone, tap the *USB* notification and choose **File transfer**.
3. Open the **USB** tab. The phone opens by itself.

Select files and press **Copy to Downloads** (or **Copy to…**), or drag files
and folders from Finder onto the window to copy them onto the phone. Folders
keep their structure, nothing is ever overwritten (a clash becomes
`name (2).jpg`), and photos keep their original dates.

**"Another app has the phone"** — macOS opens Android phones in Image Capture
(`ptpcamerad`) the moment they are plugged in, and that locks every other app
out. **Free the phone and connect** stops it (and Android File Transfer, if
installed) so ShareLocal can open the phone; both come back by themselves the
next time they are needed. No admin password is involved.

## Releases

Pushing a version tag builds both installers on GitHub and publishes them on
the Releases page:

```bash
git tag v0.2.0 && git push --tags
```

The workflow ([`.github/workflows/build.yml`](.github/workflows/build.yml))
builds the Windows installer and a universal macOS disk image (Apple Silicon
and Intel) in parallel, then creates release **v0.2.0** with both files, the
install notes from [`.github/release-notes.md`](.github/release-notes.md), and
a list of changes since the previous tag. The tag sets the version — nothing
needs editing first. `gh workflow run build.yml` builds without publishing.

## License

[MIT](LICENSE) © 2026 Sreng Kuong
