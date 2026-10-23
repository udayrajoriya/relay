# Relay

A Windows desktop download manager built with Electron and a local Node.js transfer engine. Download real files, pause and recover progress, schedule transfers, and keep completed downloads organized.

## Run the Windows app

Open `dist/Relay-0.1.0-Windows.exe`. This is a portable, unsigned x64 build; no Node.js installation is needed to run it. Windows may show its standard unsigned-app prompt. An unpacked executable is also available at `dist/win-unpacked/Relay.exe`.

Choose **New download**, paste a direct HTTP(S) URL, and start. Files initially go to your Windows Downloads folder under `Relay`, sorted into category folders. Change the destination in **Settings & integration**. Closing the app saves and pauses running downloads. Open it again and choose **Resume all** to continue.

## Included

- Concurrent byte-range transfers, with 1–16 connections per file. Workers pick up the next unclaimed segment as they finish. Actual speed depends on the server and your network; there is no guaranteed speed multiplier.
- Persistent segment files and atomic history writes. Resume checks a strong ETag or Last-Modified validator and total file size before reusing bytes. Missing or changed validators cause a safe restart. Servers that ignore ranges use a single connection and restart on resume.
- Automatic retries for interrupted segments, visible failure messages, per-file progress and ETA, and completion notifications.
- Named, ordered queues. One file runs per queue; multiple queues can run together. **Move up** changes queue priority; choose **Queue order** in the sort menu to view the actual order.
- One-time scheduling in your local time zone. Relay must be open and Windows awake to run scheduled transfers. Overdue queued work starts when Relay next opens. Recovered in-progress work remains paused until resumed.
- A shared bandwidth cap for all active connections, and automatic categorization into Videos, Music, Archives, Documents, Images, Programs, and Other.
- Browser companions for Chrome/Edge (Manifest V3) and Firefox (temporary add-on). Direct-link context menu, optional browser download capture, and direct audio/video detection.
- Safe filenames, no overwriting existing completed files, and a sandboxed renderer with no Node.js access. Removing an entry deletes its partial data but keeps any completed file.

## Browser setup

1. Open **Settings & integration → Open extension folder**.
2. **Chrome:** open `chrome://extensions`, enable **Developer mode**, select **Load unpacked**, and choose the `chromium` folder. **Edge:** follow the same steps at `edge://extensions`.
3. **Firefox:** open `about:debugging`, choose **This Firefox → Load Temporary Add-on**, and select `firefox/manifest.json`. This development installation lasts until Firefox restarts. Signed distribution is not included. Grant requested site permissions if Firefox asks; media detection needs permission on the current site.
4. In Relay settings, click **Copy pairing token**. Open the browser companion popup, paste the token, and click **Connect**.
5. Right-click a link or media element and choose **Download with Relay**, paste a URL into the popup, or turn on automatic capture. Play media on a page and open the popup to view detected direct media files.

The bridge binds only to `127.0.0.1:17864`. Requests require a private 256-bit pairing token stored in your app profile. Keep it private. The extension asks for HTTP(S) host access to detect media across sites; it stores up to 30 detected URLs per tab locally and clears them on navigation or tab close. URLs are only sent to your local Relay app, which contacts the file's origin to download it. The app has no analytics or external backend.

Media support is **direct HTTP(S) video/audio files**, including MP4, WebM, MP3, and similar formats. DRM, HLS/DASH playlist assembly, blob URLs, browser-cookie-only authentication, and site-specific extractors such as YouTube are not implemented. Browser capture cannot transfer browser login cookies. Use a public or signed direct download URL for those files. Unsupported downloads show an error in Relay.

## Develop

Requires Node.js **22.12 or newer** (Node 24 LTS recommended) and npm.

```powershell
npm ci
npm start
npm test
npm run dist
```

Electron downloads its runtime on first launch; packaging also downloads the Windows packaging tools. `npm run dist` builds the portable x64 executable. The source is in `src/`, and the browser extensions are in `extension/`.

### Verification

`npm test` uses a local HTTP fixture and checks concurrent transfers, SHA-256 integrity, pause/resume, recovery after a forcibly killed process, remote-file changes, no-range fallback, unknown lengths, retry after disconnect, invalid range responses, HTTP errors, scheduling, queue ordering, bandwidth limiting, and bridge authentication.

Additional smoke scripts require Playwright and its Chromium browser:

```powershell
$env:PLAYWRIGHT_MODULE = 'path\to\node_modules\playwright'
node test/desktop-smoke.cjs
node test/extension-smoke.cjs
```

Set `CHROMIUM_EXECUTABLE` to an existing Chromium-for-Testing executable if it is not in Playwright's default browser directory. The desktop test launches Electron against an isolated profile and exercises the UI, verifies the resulting file hash, and checks renderer isolation. The extension test loads the actual Chromium extension and verifies pairing, direct URL handoff, media detection, and automatic download capture. Chrome/Edge share the tested Chromium implementation. Firefox's manifest and companion sources are included; Firefox installation and operation have not been verified end to end.

Download history, partial segments, and the pairing token live in `%APPDATA%/relay-download-manager/`. Preserve that folder if you need to migrate or recover downloads. Resume can only reuse bytes while the original URL remains valid and the server provides a matching validator. Normal process interruption is tested; no application can guarantee recovery from physical disk corruption. On NTFS, assembled files are flushed and published atomically through a hard link; filesystems without hard-link support use an exclusive-copy fallback.

## Design references

Implementation follows the [Electron security guidance](https://www.electronjs.org/docs/latest/tutorial/security), [HTTP If-Range semantics](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/If-Range), and the [Chrome downloads extension API](https://developer.chrome.com/docs/extensions/reference/api/downloads).
