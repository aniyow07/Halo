# Halo

AI-powered desktop screen analysis, in the spirit of Android's Circle to Search.
Press **Ctrl+Shift+Space** anywhere, drag a rectangle over anything on screen,
and ask Google Gemini about it. Answers stream into a small panel next to the
selection, and you can ask follow-up questions about the same region.

> Status: phase 1 (functional UI). Verified on Arch Linux + Hyprland 0.56
> (Wayland, 2560×1600 at 1.6× fractional scaling, NVIDIA). Windows and X11 code
> paths compile-target their own backends but are **NOT VERIFIED**; see
> [Verification status](#verification-status).

## Technology

| Layer | Stack |
| --- | --- |
| Shell | Tauri 2 (Rust), WebKitGTK on Linux, WebView2 on Windows |
| UI | React 19, TypeScript (strict), Vite 8 |
| Capture | Wayland: `zwlr_screencopy_v1` via `libwayshot`; X11: `GetImage` + RandR via `x11rb`; Windows: `xcap` (GDI) |
| Images | `image` crate: crop, Lanczos3 resize, PNG / lossless WebP / JPEG |
| AI | Gemini REST `streamGenerateContent?alt=sse` via `reqwest` (streamed, cancellable) |
| Secrets | OS credential store via `keyring` (Secret Service / Windows Credential Manager) or `GEMINI_API_KEY` |

## Architecture

```
 hotkey / tray / button / `halo --activate`
                │
                ▼
  overlay::activate ── hide Halo windows ── capture::capture_monitor (frozen frame, in memory)
                │
                ▼
  fullscreen overlay window (React OverlayApp) draws the frozen frame on a canvas
                │ drag
                ▼
  commands::process_selection ── selection::to_frame_pixels ── image_processing::process_selection
                │
                ▼
  FloatingHaloPanel ── commands::ask ── gemini::stream_generate_with_retry ── tauri::ipc::Channel
                                                         (SSE parser, cancellation token)
```

Key design decisions:

- **Freeze-frame capture.** Halo captures the whole monitor *before* showing
  anything, then lets you select on top of that frozen image. Halo's own
  overlay and selection outline can therefore never appear in the pixels sent
  to Gemini, and the crop is exactly what you saw.
- **Coordinates.** The overlay reports the selection in CSS pixels relative to
  the measured drawing surface; the backend maps that to physical frame pixels
  by ratio, which absorbs fractional scaling (1.6× here) without guessing.
- **State machine** (`src-tauri/src/state`): one session at a time, every
  session/request has a UUID, every mutation checks the ids. Late or
  duplicate stream events, answers after cancellation, and answers about a
  previous selection are discarded. The frontend reducer
  (`src/app/conversation.ts`) applies the same rules a second time.
- **Streaming.** Each question gets its own Tauri `Channel`, so events from
  different requests can't interleave. The SSE parser buffers raw bytes and
  only decodes complete lines, so arbitrary chunk boundaries (including inside
  UTF-8 characters) are safe.
- **Follow-ups.** Gemini is stateless. Every request sends the image and the
  whole conversation explicitly. A new selection starts a new context.

Source map:

```
src-tauri/src/
  capture/           real screen capture backends
  selection/         CSS → frame pixel mapping and validation
  image_processing/  crop, resize, encode, measurements
  gemini/            request schema, SSE parser, streaming client, error mapping
  configuration/     settings file, API key resolution and credential store
  state/             session/request state machine
  shortcut/          global shortcut (OS hotkey or Hyprland binding)
  platform/          display-server detection, Hyprland IPC
  overlay.rs         window lifecycle: activation, overlay, dismissal
  commands/          Tauri commands (validated inputs only)
  live_tests.rs      opt-in live tests against the real screen and Gemini
src/
  app/               HomeApp, OverlayApp, conversation reducer, panel placement
  components/        SelectionOverlay, FloatingHaloPanel, ImagePreview,
                     ChatMessages, StreamingResponse, ContextualActions,
                     Settings, ErrorDisplay
  hooks/             useConversation
  services/          typed wrappers for the Rust commands
```

## Prerequisites

- Rust 1.85+ (tested with 1.99) and Cargo
- Node.js 20+ and npm (tested with Node 24.21)
- Linux: WebKitGTK 4.1, GTK 3, libsoup 3, `libayatana-appindicator` (tray),
  a C toolchain and `pkg-config`. On Arch:
  `sudo pacman -S webkit2gtk-4.1 gtk3 libsoup3 libayatana-appindicator base-devel`.
  On Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libayatana-appindicator3-dev build-essential pkg-config`.
- Windows: Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 11).
- A Gemini API key from <https://aistudio.google.com/apikey>.

If Node isn't installed system-wide you can use an official tarball; this
project was built with Node unpacked into `~/.local/opt/node`, so prefix
commands with `PATH=$HOME/.local/opt/node/bin:$PATH`.

## Install and run

```bash
cd halo
npm install
cp .env.example .env   # then put your key in GEMINI_API_KEY
npm run tauri dev
```

Release build (the binary runs without the dev server):

```bash
npm run tauri build -- --bundles deb,rpm   # Linux packages
./src-tauri/target/release/halo            # or run the binary directly
```

Windows (NOT VERIFIED): `npm run tauri build` produces MSI/NSIS installers in
`src-tauri/target/release/bundle/`. An AppImage can be added with
`--bundles appimage`; that step downloads linuxdeploy and wasn't run here.

## Gemini configuration

The key never reaches the web UI. The Rust backend resolves it in this order:

1. `GEMINI_API_KEY` in the environment, or in a `.env` file in the directory
   Halo is started from (backend only; Vite only exposes `VITE_*` variables).
2. A key saved from **Settings → Gemini API key**, stored in the OS credential
   store (Secret Service on Linux, Credential Manager on Windows). Halo refuses
   to store keys in plain text if no credential store is available.

The key is sent only as the `x-goog-api-key` header, never in a URL or log.

Model: defaults to `gemini-3.6-flash`. Override it with `GEMINI_MODEL` (which
takes precedence) or in Settings. **Check model** calls the live `models.get`
endpoint. `gemini-3.8-flash` also validates, but it returned "high demand" 503s
throughout testing, so it isn't the default. Transient 5xx errors are retried
twice with backoff, but only if no text has streamed yet.

## Usage

- **Ctrl+Shift+Space** (configurable), the **Select screen region** button,
  the tray menu, or `halo --activate` starts a selection.
- Drag a rectangle. Escape cancels the drag; Escape again (or ✕) dismisses Halo.
- Use a quick action (Explain, Summarize, Solve, Debug, Describe, Translate)
  or type a question and press Enter. Shift+Enter inserts a newline.
- Ask follow-ups in the same box. **Cancel** stops generation, **Retry**
  re-asks a failed or cancelled question, **Copy** copies the last answer,
  and **New selection** draws a new region on the same frozen screen.
- Closing the home window keeps Halo running for the shortcut. Quit from the
  window or the tray.

### Keyboard shortcut per platform

| Platform | Mechanism |
| --- | --- |
| Windows, X11 | `tauri-plugin-global-shortcut` (OS hotkey). Registration failures (for example, the key is taken by another app) are reported in the home window and Settings. |
| Hyprland | Runtime compositor binding `CTRL SHIFT, space → halo --activate`, added over Hyprland IPC (supports both legacy and Lua configs) and removed on exit, including on SIGTERM/SIGINT/SIGHUP. It is never written to your config. Bindings left by a killed process are cleaned up at the next start. An existing user binding for the same chord is reported as a conflict and not touched. |
| Other Wayland compositors | Apps can't grab global keys. Bind `halo --activate` in your compositor. Settings shows this. |

## Image processing

Pipeline (Rust, off the UI thread): validate → crop from the frozen frame →
drop alpha → resize so the longest edge is at most *Max dimension* (default
2048, Lanczos3, never upscales) → encode → measure.

Encoding **Auto** (default) encodes lossless PNG, lossless WebP and JPEG
(quality 90) and sends the smallest. The baseline ("was …") is the selected
region as a lossless PNG at captured resolution, which is what you would send
without processing. Every selection shows original and processed
dimensions, sizes, reduction and format in the panel.

Measured on this machine (2560×1600 screen, release build):

| Selection | Result (Auto) | Reduction vs PNG baseline |
| --- | --- | --- |
| Full screen, 2560×1600 → 2048×1280 | JPEG 342 KB (was 631 KB) | 45.8% |
| Centre 1280×800, mixed UI | JPEG 117 KB (was 199 KB) | 40.8% |
| Left half 1280×1600, text UI | JPEG 224 KB (was 315 KB) | 28.9% |
| Terminal error text 1245×384 | JPEG 57 KB (was 75 KB) | 23.8% |
| Browser paragraph 1024×295 | PNG 76.5 KB | 0% |
| Simple bar chart 632×384 | PNG 13.9 KB | 0% |

The 50–65% reduction target is **not met in general**. Flat UI and text
screenshots are already compact as PNG, and pushing JPEG quality down far
enough to reach the target would blur small text. Processing takes about
30 ms for a typical region and 0.6 s for a full 2560×1600 screen (release
build).

## Linux: X11 and Wayland notes

- **Hyprland / wlroots compositors (verified on Hyprland):** capture uses
  `wlr-screencopy` at native resolution with no permission prompt. The overlay
  is a fullscreen window on the focused monitor.
- **GNOME / KDE Wayland:** `wlr-screencopy` isn't available, so capture fails
  with an explanatory error. Portal (`org.freedesktop.portal.Screenshot`) or
  PipeWire capture is not implemented yet. NOT VERIFIED.
- **X11:** capture via `GetImage` on the root window, monitor chosen by
  cursor position via RandR. NOT VERIFIED (no X11 session was available).
- **NVIDIA + Wayland:** WebKitGTK's DMA-BUF renderer crashes with
  "Error 71 (Protocol error)". Halo sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`
  automatically when the NVIDIA driver is loaded.
- **Fractional scaling:** GTK3 renders at an integer scale and the compositor
  downsamples, so the frozen frame in the overlay can look slightly soft.
  The image sent to Gemini is the original capture at native resolution.
- **Multi-monitor:** Halo captures and overlays the focused monitor (Hyprland)
  or the monitor under the cursor (X11/Windows). Selections can't span
  monitors. NOT VERIFIED with more than one monitor.

## Privacy

- The screen is captured only when you activate Halo, one frame per
  activation. There is no continuous recording.
- Captures and selections live in memory only and are never written to disk.
- The selected region (not the whole screen) and your questions are sent to
  Google Gemini. The panel says so.
- Model output is rendered as Markdown with raw HTML disabled. Remote images
  aren't loaded, links open in the system browser only when clicked (http/https
  only), and Halo never executes model-generated code or commands.
- The webview gets only `core:default` permissions and a strict CSP. All
  native functionality goes through Halo's own validated commands, and there is
  no filesystem or shell access from the UI.

## Testing

```bash
# Rust
cd src-tauri
cargo fmt --check
cargo clippy --all-targets
cargo test
# Live (real screen + real Gemini; needs a graphical session and GEMINI_API_KEY):
cargo test --release live_ -- --ignored --nocapture --test-threads=1

# Frontend
cd ..
npm run typecheck
npm run lint
npm test
npm run build
npm run tauri build -- --bundles deb,rpm
```

Automated coverage includes selection mapping (fractional scaling, edges,
invalid input), cropping, encoding round-trips, compression math, settings and
key validation, request serialization, SSE parsing at every chunk split
(including split UTF-8), stream errors, premature disconnects, idle timeouts,
cancellation, HTTP error mapping through a local HTTP server, request-ID
isolation, conversation context and stale-session rejection (Rust). The
frontend tests cover the selection lifecycle, submission, streaming,
completion, cancellation before and after the request id is known, errors,
follow-ups, quick actions, settings, copy, dismissal and Markdown sanitisation.

## Verification status

Verified on Arch Linux, Hyprland 0.56.2 (Wayland), single 2560×1600 display
at 1.6×, on 2026-10-12:

| Check | Result |
| --- | --- |
| `cargo fmt`, `cargo clippy --all-targets` | clean |
| `cargo test` | 76 passed (2 live tests ignored by default) |
| Live tests (`--ignored`) | passed: real capture, compression measurements, streaming, follow-up, mid-stream cancel, invalid model → `invalidModel`, invalid key → `authentication` |
| Frontend typecheck, lint, 38 tests, production build | passed |
| `tauri build` (deb, rpm) | built |
| E2E browser test: select a paragraph in Firefox, summarize, follow-up | passed; preview pixels matched the selection and the follow-up used context |
| E2E code test: rustc error in a terminal, explain, follow-up | passed |
| E2E image test: bar chart | passed |
| E2E recovery: cancel selection, desktop usable, reactivate, cancel generation, new selection with no stale text | passed |
| Duplicate activation while overlay is open | rejected |
| Ctrl+Shift+Space through Hyprland | opens the overlay |
| Shortcut cleanup on exit and after a crash | verified |

E2E inputs were driven with a virtual pointer and keyboard
(`zwlr_virtual_pointer_v1` / `zwp_virtual_keyboard_v1`) on the real desktop.

**NOT VERIFIED:** Windows (all of it), X11, GNOME/KDE Wayland, multiple
monitors, negative monitor coordinates, the OS-hotkey backend
(`tauri-plugin-global-shortcut`), saving a key to the credential store from the
UI (gnome-keyring's Secret Service is running here, but no key was saved to it), changing the shortcut
or image settings through the UI (covered by unit tests only), mid-stream
cancel through the GUI (cancel was exercised before the first token in the
GUI and mid-stream in the live Rust test), and AppImage/MSI/NSIS bundles.

## Troubleshooting

- **Blank window / "Error 71 (Protocol error)" on Wayland:** start with
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` (done automatically for NVIDIA).
- **Shortcut does nothing:** check the shortcut line on the home window. On
  Hyprland, run `hyprctl binds` and look for "Halo: select screen region".
  Elsewhere on Wayland, bind `halo --activate` yourself.
- **"Screen capture failed … wlr-screencopy":** your compositor doesn't
  support the protocol (GNOME, KDE). Not supported yet.
- **"Gemini is temporarily unavailable (HTTP 503)":** the model is
  overloaded. Retry, or pick another model in Settings.
- **Authentication errors:** check the key, or remove `GEMINI_API_KEY` from
  the environment if you meant to use the stored key (the environment wins).
- **Capture diagnostics:** Settings → Capture diagnostics shows the display
  server, capture backend, shortcut backend and the last capture's geometry.

## Known limitations

- The panel lives inside the fullscreen overlay, so the desktop behind it is
  frozen until you dismiss Halo (like Circle to Search). Dismissal restores it.
- No selection across monitors and no portal/PipeWire capture yet.
- Compression doesn't reach 50–65% on text-heavy UI (see the measurements above).
- The UI is intentionally plain. The visual redesign is a later phase.
# Halo
