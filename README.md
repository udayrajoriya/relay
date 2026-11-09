# Relay

A Windows and Linux desktop download manager built with Electron and a local Node.js transfer engine. Download real files, pause and recover progress, schedule transfers, and keep completed downloads organized.

## Run the app

To run from source on Windows or Linux, install Node.js **22.12 or newer** and npm, then run:

```sh
npm ci
npm start
```

Packaged builds include Electron and do not require Node.js. The Linux packaging scripts target x64.

### Linux

Build on Linux with `npm ci` followed by `npm run dist:linux`. On x64 Linux, this creates `dist/Relay-0.1.0-Linux-x86_64.AppImage` and a `.tar.gz` archive. Both include Electron; Node.js is only needed to develop or build Relay.

```sh
chmod +x dist/Relay-0.1.0-Linux-x86_64.AppImage
./dist/Relay-0.1.0-Linux-x86_64.AppImage
```

AppImage needs FUSE 2 support. If FUSE is unavailable, extract and run it:

```sh
./dist/Relay-0.1.0-Linux-x86_64.AppImage --appimage-extract
./squashfs-root/AppRun
```

Alternatively, use the tar.gz package without FUSE:

```sh
tar -xzf dist/Relay-0.1.0-Linux-x64.tar.gz
./Relay-0.1.0-Linux-x64/relay
```

After building, you can also run the unpacked app directly with `./dist/linux-unpacked/relay`. Run as your regular desktop user. Relay needs a graphical Linux session and Electron’s system libraries (including GTK, NSS, and audio libraries). Folder opening uses the desktop’s default file manager; notifications require a desktop notification service.

### Windows

Build on Windows with `npm ci` followed by `npm run dist:win`, then open `dist/Relay-0.1.0-Windows.exe`. This is a portable, unsigned x64 build; no Node.js installation is needed to run it. Windows may show its standard unsigned-app prompt. An unpacked executable is also available at `dist/win-unpacked/Relay.exe`.

### Using Relay

Choose **New download**, paste a direct HTTP(S) URL, and choose a **Download directory** using the path field or **Browse** button before starting. The field starts with the default from **Settings & integration**; choosing another folder applies only to that download. Completed files are sorted into category subfolders when organization is enabled. Closing the app saves and pauses running downloads. Open it again and choose **Resume all** to continue.

Partial download data stays in `<chosen directory>/.relay-parts/<download ID>/`, and final assembly also happens on the destination drive. Partial files are removed after completion or when the download is removed. Existing partial downloads from the previous version are copied from the app profile into their saved destination on the next resume; the originals are kept until migration succeeds. If the destination is unavailable, the download shows an error and the original bytes remain available for retry.

## Included

- Concurrent byte-range transfers, with 1–16 connections per file. Workers pick up the next unclaimed segment as they finish. Actual speed depends on the server and your network; there is no guaranteed speed multiplier.
- Persistent segment files and atomic history writes. Resume checks a strong ETag or Last-Modified validator and total file size before reusing bytes. Missing or changed validators cause a safe restart. Servers that ignore ranges use a single connection and restart on resume.
- Automatic retries for interrupted segments, visible failure messages, per-file progress and ETA, and completion notifications.
- Named, ordered queues. One file runs per queue; multiple queues can run together. **Move up** changes queue priority; choose **Queue order** in the sort menu to view the actual order.
- One-time scheduling in your local time zone. Relay must be open and your computer awake to run scheduled transfers. Overdue queued work starts when Relay next opens. Recovered in-progress work remains paused until resumed.
- A shared bandwidth cap for all active connections, and automatic categorization into Videos, Music, Archives, Documents, Images, Programs, and Other.
- Browser companions for Chrome/Edge (Manifest V3) and Firefox (temporary add-on). Direct-link context menu, optional browser download capture, and direct audio/video detection.
- Safe filenames, no overwriting existing completed files, and a sandboxed renderer with no Node.js access. Removing an entry deletes its partial data but keeps any completed file.

## Browser setup

1. Open **Settings & integration → Open extension folder**.
2. **Chrome:** open `chrome://extensions`, enable **Developer mode**, select **Load unpacked**, and choose the `chromium` folder. **Edge:** follow the same steps at `edge://extensions`.
3. **Firefox:** open `about:debugging`, choose **This Firefox → Load Temporary Add-on**, and select `firefox/manifest.json`. This development installation lasts until Firefox restarts. Signed distribution is not included. Grant requested site permissions if Firefox asks; media detection needs permission on the current site.
4. In Relay settings, click **Copy pairing token**. Open the browser companion popup, paste the token, and click **Connect**.
5. Right-click a link or media element and choose **Download with Relay**, paste a URL into the popup, or turn on automatic capture. Relay asks for a destination folder before accepting each browser download. Canceling this picker leaves the browser's original download running. Play media on a page and open the popup to view detected direct media files.

The bridge binds only to `127.0.0.1:17864`. Requests require a private 256-bit pairing token stored in your app profile. Keep it private. The extension asks for HTTP(S) host access to detect media across sites; it stores up to 30 detected URLs per tab locally and clears them on navigation or tab close. URLs are only sent to your local Relay app, which contacts the file's origin to download it. The app has no analytics or external backend.

Media support is **direct HTTP(S) video/audio files**, including MP4, WebM, MP3, and similar formats. DRM, HLS/DASH playlist assembly, blob URLs, browser-cookie-only authentication, and site-specific extractors such as YouTube are not implemented. Browser capture cannot transfer browser login cookies. Use a public or signed direct download URL for those files. Unsupported downloads show an error in Relay.

## Develop

Requires Node.js **22.12 or newer** (Node 24 LTS recommended) and npm.

```sh
npm ci
npm start
npm test
npm run dist
```

Electron downloads its runtime for the current platform on first use. `npm run dist` builds for the current platform: AppImage and tar.gz on Linux, or a portable executable on Windows. Use `npm run dist:linux` or `npm run dist:win` for explicit x64 targets. Packaging downloads additional build tools as needed. `npm run pack` creates an unpacked app for local testing (`dist/linux-unpacked/relay` on Linux). The source is in `src/`, and the browser extensions are in `extension/`.

### Verification

`npm test` uses a local HTTP fixture and checks concurrent transfers, SHA-256 integrity, pause/resume, recovery after a forcibly killed process, remote-file changes, no-range fallback, unknown lengths, retry after disconnect, invalid range responses, HTTP errors, scheduling, queue ordering, bandwidth limiting, and bridge authentication.

Additional smoke scripts require Playwright and its Chromium browser. Install the same Playwright version used by CI:

```sh
npm install --no-save --package-lock=false playwright@1.65.0
npx playwright install chromium
npm run test:desktop
npm run test:extension
```

`npm run test:desktop` launches the source app by default. After building with `npm run pack` or the platform’s distribution command, `npm run test:packaged` checks the packaged launcher, queue operations, and bundled extensions. It defaults to `dist/linux-unpacked/relay` on Linux and `dist/Relay-0.1.0-Windows.exe` on Windows. Set `RELAY_EXECUTABLE` to test another packaged executable, or to run the full desktop download test against a package:

```sh
# Linux
RELAY_EXECUTABLE="$PWD/dist/linux-unpacked/relay" npm run test:desktop
# Test an extracted AppImage launcher:
RELAY_EXECUTABLE="$PWD/squashfs-root/AppRun" npm run test:packaged
```

```powershell
# Windows
$env:RELAY_EXECUTABLE = "$PWD\dist\Relay-0.1.0-Windows.exe"
npm run test:desktop
Remove-Item Env:RELAY_EXECUTABLE
```

If Playwright is installed outside the project, set `PLAYWRIGHT_MODULE` to its package path. Desktop tests need a graphical session; on headless Linux, use `xvfb-run -a npm run test:desktop` or `xvfb-run -a npm run test:packaged`.

The [Linux workflow](.github/workflows/linux.yml) is configured to run engine tests, desktop tests against source and unpacked apps, Chromium extension tests, and an extracted AppImage launch test on Ubuntu 24.04. It uploads the AppImage and tar.gz packages as the `Relay-Linux-x64` artifact. Local verification has passed on Arch Linux x64; Firefox integration has not been verified end to end.

Set `CHROMIUM_EXECUTABLE` to an existing Chromium-for-Testing executable if it is not in Playwright's default browser directory. The desktop test launches Electron against an isolated profile and exercises the UI, verifies the resulting file hash, and checks renderer isolation. The extension test loads the actual Chromium extension and verifies pairing, direct URL handoff, media detection, and automatic download capture. Chrome/Edge share the tested Chromium implementation. Firefox's manifest and companion sources are included; Firefox installation and operation have not been verified end to end.

Download history and the pairing token live in `%APPDATA%/relay-download-manager/` on Windows and `$XDG_CONFIG_HOME/relay-download-manager/` on Linux (normally `~/.config/relay-download-manager/`); download bytes live in each chosen directory's `.relay-parts` subfolder. Preserve both the app profile and those subfolders to migrate or recover downloads. Resume can only reuse bytes while the original URL remains valid and the server provides a matching validator. Normal process interruption is tested; no application can guarantee recovery from physical disk corruption. On filesystems with hard-link support (including NTFS and ext4), assembled files are flushed and published atomically through a hard link; filesystems without hard-link support use an exclusive-copy fallback.

## Design references

Implementation follows the [Electron security guidance](https://www.electronjs.org/docs/latest/tutorial/security), [HTTP If-Range semantics](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/If-Range), and the [Chrome downloads extension API](https://developer.chrome.com/docs/extensions/reference/api/downloads).
