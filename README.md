# Relay

A native Rust download manager for Linux and Windows. Relay uses egui/eframe for its desktop interface, Tokio for background work, and reqwest with Rustls for HTTP(S) transfers. The app runs without Electron, Node.js, or a webview.

## Run from source

Install the [stable Rust toolchain](https://rustup.rs/) and a C/C++ linker. Windows builds require Visual Studio Build Tools with **Desktop development with C++** and the Windows SDK. Linux needs a graphical X11 or Wayland session with OpenGL and libxkbcommon. Native folder dialogs use the desktop's XDG portal service on Linux.

Typical Ubuntu/Debian build dependencies:

```sh
sudo apt-get install build-essential pkg-config libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libgl1-mesa-dev libegl1-mesa-dev libx11-dev libxcursor-dev libxi-dev libxrandr-dev
```

Then, on either platform:

```sh
cargo run --locked --release
```

Rust 1.88 is the minimum version for the selected dependencies; the repository uses the stable toolchain. First builds download and compile dependencies. Subsequent builds reuse Cargo's local build output.

## Packaged builds

Build the native executable and create a portable package using Python 3.11 or newer:

```sh
cargo build --locked --release
python scripts/package.py
```

On Linux x64, this produces `dist/Relay-0.2.0-Linux-x64.tar.gz`:

```sh
tar -xzf dist/Relay-0.2.0-Linux-x64.tar.gz
./Relay-0.2.0-Linux-x64/relay
```

On Windows x64, this produces `dist/Relay-0.2.0-Windows-x64.zip`. Extract it and run `relay.exe`. The native binary embeds both browser companions; no separate extension download is necessary. Windows builds are unsigned.

The Linux archive includes an optional desktop entry and icon. Put the binary on your `PATH`, install `local.relay.downloads.desktop` under `~/.local/share/applications/`, and install `relay.png` as `~/.local/share/icons/hicolor/256x256/apps/relay.png` to add an application-menu entry. The native Linux archive does not need FUSE. The old Electron AppImage and Windows `.exe` packaging scripts have been replaced by these native packages.

The [Rust desktop workflow](.github/workflows/rust.yml) runs checks for pushes and pull requests. After the Linux, Windows, and browser checks pass on `master`, it automatically publishes a GitHub Release named for the version in `Cargo.toml` (for example, `v0.2.0`). The release contains the Linux tar.gz, Windows zip, generated notes, and `SHA256SUMS`. Uploaded assets are downloaded again and checked against their local SHA-256 hashes before the draft release is published.

A published version is never overwritten. To release the next version, update `version` in `Cargo.toml`, run `cargo check` to refresh `Cargo.lock`, and commit both files to `master`. You can also manually run the workflow on `master` to retry an unpublished version. Draft retries must target the same commit. Prerelease versions such as `0.3.0-rc.1` are marked as prereleases.

Temporary Actions artifacts are uploaded only for release candidates. After successful publication, the workflow deletes its own Linux and Windows artifacts; the release assets remain available. If checks or publication fail, temporary artifacts expire after one day. Pull requests, feature branches, and already-published versions do not upload package artifacts.

## Use Relay

Choose **New download**, paste a direct HTTP(S) URL, and select a **Download directory** before starting. An optional file name overrides the server's name. Select a named queue, 1–16 connections, a local schedule in `YYYY-MM-DD HH:MM` format, or **Add paused**.

Use **Pause**, **Resume**, **Pause all**, or **Resume all** to control transfers. **Move up** changes queue priority; choose **Queue order** to see the saved order. A named queue runs one file at a time, while different queues can run concurrently up to the simultaneous-download setting. Search, category views, queue filters, and sorting help organize the list.

Settings control the default destination, connections per file, simultaneous downloads, shared bandwidth cap, and category subfolders. A bandwidth cap of zero means unlimited. Per-download destination choices do not change the default. Completed files can be revealed in their containing folder. Removing an entry deletes partial bytes and keeps any completed file.

Closing Relay cancels active requests, flushes partial files, and saves running jobs as paused. Reopen Relay and choose **Resume all** to continue. Scheduled jobs need Relay open and the computer awake; overdue queued work starts when Relay opens. Completion notifications depend on your desktop notification service.

## Existing Electron profiles

Close the Electron version before opening the Rust version. Relay keeps the existing JSON history, pairing token, and partial-file layout:

- Windows profile: `%APPDATA%/relay-download-manager/`.
- Linux profile: `$XDG_CONFIG_HOME/relay-download-manager/`, normally `~/.config/relay-download-manager/`.
- Partial bytes: `<chosen directory>/.relay-parts/<download ID>/`.

Back up the profile and partial directories before switching between versions. The Rust app reads existing `state.json` files directly, reuses the existing browser token, recounts partial bytes from disk, and restores interrupted jobs as paused. Older partial data in the profile's `parts/` folder is copied into the saved destination before resume; originals remain until migration succeeds. Unavailable destinations report an error and retain the originals for retry. Corrupt history stops startup instead of replacing the original file.

Resume reuses bytes only when file size and a strong ETag or Last-Modified validator match. Missing or changed validators trigger a safe restart. Servers that ignore byte ranges use a single connection and restart on resume. Transient segment failures retry up to three attempts. Final assembly happens on the destination drive, flushes the staging file, and publishes without overwriting existing files. Filesystems without hard-link support use an exclusive-copy fallback. Physical disk corruption and power-loss recovery are not guaranteed.

Only one native Relay process can open a profile. A second instance reports that the profile is in use; switch to the existing window. For isolated testing, use absolute paths:

```sh
cargo run -- --profile /absolute/test-profile --downloads /absolute/test-downloads
```

## Browser integration

1. Open **Settings & integration → Open extension folder**. The embedded companions are extracted under the profile's `extensions/0.2.0/` directory.
2. **Chrome:** open `chrome://extensions`, enable **Developer mode**, choose **Load unpacked**, and select `chromium`. **Edge:** use `edge://extensions`.
3. **Firefox:** open `about:debugging`, choose **This Firefox → Load Temporary Add-on**, and select `firefox/manifest.json`. The temporary installation lasts until Firefox restarts. Signed distribution is not included.
4. In Relay, click **Copy pairing token**. Paste it into the companion popup and click **Connect**.
5. Right-click a link or media element and choose **Download with Relay**, paste a direct URL into the popup, or enable automatic capture. Relay asks for a destination for every browser download. Canceling the picker leaves the browser's original download running.

The bridge binds only to `127.0.0.1:17864`. Requests require a private 256-bit pairing token from your profile. Keep it private. Browser handoffs that disconnect while waiting for a folder are discarded. If the port is occupied, Relay still opens and reports that browser integration is unavailable.

The companion asks for HTTP(S) site access, detects direct audio/video responses, keeps up to 30 URLs per tab locally, and clears them on navigation or tab close. URLs go only to your local Relay app, which contacts the file's origin. Relay has no analytics or external backend.

Media support covers direct HTTP(S) files such as MP4, WebM, and MP3. DRM, HLS/DASH assembly, blob URLs, browser-cookie authentication, and site-specific extractors such as YouTube are not implemented. Use public or signed direct URLs. Chromium integration is tested against the Rust bridge; Firefox end-to-end operation remains unverified.

## Verify changes

```sh
python -m unittest discover -s test -p 'test_release.py'
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

Rust tests use local HTTP fixtures to check segmented transfers and SHA-256 integrity, pause/resume, forced process termination, custom destinations, legacy migration and failed migration, validator changes, no-range fallback, unknown lengths, empty files, retries, protocol errors, scheduling, queue order, bandwidth caps, bridge authentication, canceled and disconnected handoffs, safe filenames, atomic profile writes, and exclusive profile locking.

The core can be tested without building desktop dependencies:

```sh
cargo test --locked --no-default-features
```

The native smoke test launches the real application with an isolated profile, renders the downloads view, add form, and settings, saves screenshots, and verifies state persistence and embedded companions. It needs Python 3.11+ and a graphical session:

```sh
cargo build --locked
python test/native-smoke.py --artifacts dist/smoke
# Headless Linux:
xvfb-run -a python test/native-smoke.py --artifacts dist/smoke
```

The browser smoke test additionally needs Node.js and Playwright **only as test tools**. Install Playwright outside the project or set `PLAYWRIGHT_MODULE` to an existing installation:

```sh
cargo build --locked --no-default-features --example integration_host
npm install --prefix /tmp/relay-browser playwright@1.65.0
/tmp/relay-browser/node_modules/.bin/playwright install chromium
PLAYWRIGHT_MODULE=/tmp/relay-browser/node_modules/playwright node test/extension-smoke.cjs
```

The test drives the real Rust engine and bridge through a test-only host and checks pairing, direct-link handoff, media detection, automatic capture, and popup-close behavior. The host automatically selects an isolated test destination; the desktop app always uses the native folder picker.

## Source layout

- `src/engine.rs`: transfers, resume, scheduling, queues, and file publication.
- `src/model.rs` and `src/profile.rs`: compatible state models, atomic persistence, tokens, and profile locking.
- `src/bridge.rs`: authenticated local browser protocol.
- `src/desktop.rs` and `src/main.rs`: native interface and application lifecycle.
- `extension/`: JavaScript browser companions required by Chrome/Edge and Firefox extension APIs.
- `tests/`: Rust regression fixtures; `test/`: native and browser smoke runners.

## References

[egui/eframe](https://github.com/emilk/egui), [Tokio](https://tokio.rs/), [reqwest](https://docs.rs/reqwest/), [HTTP If-Range semantics](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/If-Range), and the [Chrome downloads extension API](https://developer.chrome.com/docs/extensions/reference/api/downloads).
