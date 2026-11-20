# crystalify

A modern, high-performance headless Spotify CLI and TUI client written in Rust for Linux, Windows, and macOS. It features native `librespot` audio playback, interactive search and queue management, and inline album artwork rendering directly in your terminal.

---

## Features

- **Headless Spotify Playback**: Built on `librespot` (`rustls` + `rodio`/`cpal`), with honest playback state indicators (`[LOADING]`, `[PLAYING]`, `[PAUSED]`, `[ERROR]`) and automatic audio output detection without crashes when no device is present.
- **Flexible Authentication**: Supports Browser / Google OAuth 2.0 (PKCE) out of the box, Spotify Connect Discovery, and masked username/password login with memory zeroization, plus local credential caching on Linux, macOS, and Windows.
- **Search & Queue Playback**: Search across Tracks, Albums, Artists, and Playlists; play a single track or an entire album/artist/playlist collection with queue navigation and automatic next-track playback.
- **Inline Terminal Album Artwork**: Cover art rendered in the `Now Playing` panel via `ratatui-image` (Kitty graphics protocol, Sixel, and Unicode halfblock fallback).
- **Custom TUI Theme**: Clean pastel dark palette with responsive layout across terminal sizes.
- **Real-Time 3D Audio Visualizer (In Progress)**: Audio stream FFT analysis projected into dynamic 3D terminal meshes.
- **Synchronized Lyrics (In Progress)**: Real-time scrolling lyrics synchronized with playback timestamps.
- **Full Spotify Connect Remote Control / Spirc (In Progress)**: Controlling playback remotely from other Spotify clients.

---

## Controls / Keybindings

| Key | Action |
|---|---|
| `/` | Enter search mode (`Enter` to search, `Esc` or `Ctrl+C` to cancel) |
| `Tab` / `Shift+Tab` or `1`–`4` | Switch search category (`Tracks`, `Albums`, `Artists`, `Playlists`) |
| `Up` / `Down` or `k` / `j` | Move selection up / down |
| `Enter` | Play selected track or open selected album / artist / playlist |
| `p` | Play all tracks from the selected/opened collection |
| `n` / `b` | Next / previous track in queue |
| `Space` | Pause / resume playback (or retry loading track when in `[ERROR]` state) |
| `Left` / `Right` | Seek backward / forward by 5 seconds |
| `+` / `=` / `-` | Increase / decrease volume |
| `Backspace` | Return from an opened collection to search results |
| `q` / `Esc` | Close opened collection (`Esc`) or quit |

---

## Architecture

The project is structured into modular subsystems designed for concurrency and separation of concerns:

```
src/
├── main.rs                   # Entry point, async runtime, keybindings, and event loop
├── app.rs                    # Central application state machine, playback status, and queue
├── event.rs                  # Event subsystem scaffold (In Progress)
├── config/
│   └── mod.rs                # Configuration management scaffold (In Progress)
├── spotify/                  # Spotify streaming, auth, logging, and metadata integration
│   ├── mod.rs                # Module declarations
│   ├── auth.rs               # OAuth 2.0 PKCE, Discovery, masked password auth, and cross-platform caching
│   ├── client.rs             # Spotify Pathfinder / Web API client (search, collections, cover art)
│   ├── logger.rs             # Silent librespot error capture and optional CRYSTALIFY_LOG file output
│   ├── models.rs             # Strongly typed data models for tracks, albums, artists, and playlists
│   └── player.rs             # Librespot + rodio/cpal audio backend and safe device detection
├── audio/                    # Audio signal processing scaffold (In Progress)
│   ├── mod.rs                # Audio sink intercepts and stream pipelines (In Progress)
│   ├── fft.rs                # Spectrum extraction via RustFFT (In Progress)
│   └── visualizer.rs         # 3D coordinate transformations and projections (In Progress)
└── ui/                       # Ratatui TUI rendering
    ├── mod.rs                # Root UI layout and draw orchestration
    ├── theme.rs              # Color palette constants
    └── components/           # Isolated UI widgets
        ├── mod.rs
        ├── player.rs         # Now Playing card, album art, progress gauge, playback details
        ├── playlist.rs       # Search bar, category tabs, results, and collection view
        ├── statusbar.rs      # Header (device, user, audio backend) and keybinding footer
        └── visualizer.rs     # 3D visualizer canvas widget scaffold (In Progress)
```

---

## Dependencies and Crates

| Crate | Purpose |
|---|---|
| `librespot` | Headless Spotify playback engine (`rustls` TLS, `rodio` audio backend, `libmdns` discovery), handling authentication, stream decryption, and PCM decoding. |
| `tokio` | Asynchronous multi-threaded runtime managing background tasks, API requests, and the TUI event loop. |
| `ratatui` | Terminal UI framework providing layout trees and widgets. |
| `crossterm` | Cross-platform terminal control enabling raw mode, alternate screens, and keyboard event capture on Linux, Windows, and macOS. |
| `ratatui-image` | Inline terminal image rendering for album covers using Kitty, Sixel, or Unicode halfblocks. |
| `image` | Decoding cover art images (JPEG, PNG, WebP) fetched from Spotify CDN. |
| `cpal` | Cross-platform audio host and output device detection (ALSA, WASAPI, CoreAudio) for safe startup and backend reporting. |
| `reqwest` | Async HTTP client for Spotify API requests and cover artwork downloads. |
| `futures-util` | Async stream utilities used for Spotify Connect Discovery credential streams. |
| `serde` & `serde_json` | Serialization and deserialization of API payloads and search responses. |
| `log` | Lightweight logging facade used to silently capture `librespot` stream errors and write optional `CRYSTALIFY_LOG` files. |
| `rustfft` | Fast Fourier Transform library prepared for the upcoming real-time 3D audio visualizer. |
| `glam` | SIMD-accelerated 3D linear algebra library prepared for the upcoming 3D audio visualizer. |

---

## Requirements

### Operating System & Audio
- **Linux**: ALSA, PulseAudio, or PipeWire (requires ALSA development headers and `pkg-config`; OpenSSL dev headers are not required thanks to `rustls`).
- **Windows 10/11**: WASAPI audio output (native MSVC build or Linux `mingw-w64` cross-compilation).
- **macOS**: CoreAudio.

### Spotify Account
- A **Spotify Premium** account (required by `librespot` for streaming).
- No Spotify Developer Application or Client Secret is required — Browser OAuth 2.0 login works out of the box.

### Terminal
- A terminal emulator supporting TrueColor (24-bit color), such as **Windows Terminal** (Windows), Kitty, Ghostty, WezTerm, Alacritty, or Foot.
- Inline album artwork automatically uses Kitty graphics protocol or Sixel where supported, with Unicode halfblock fallback on other terminals (including Windows Terminal).

### Rust Toolchain
- Rust 1.85+ with Cargo (Edition 2024 support).

---

## Installation

### Clone the Repository

```bash
git clone https://github.com/entropymustdie/crystalify.git
cd crystalify
```

### Linux

1. Install system dependencies (ALSA headers + `pkg-config`):
   - Debian / Ubuntu:
     ```bash
     sudo apt update
     sudo apt install build-essential pkg-config libasound2-dev
     ```
   - Fedora / RHEL:
     ```bash
     sudo dnf install gcc pkg-config alsa-lib-devel
     ```
   - Arch Linux:
     ```bash
     sudo pacman -S base-devel pkg-config alsa-lib
     ```
2. Build and run:
   ```bash
   cargo build --release
   ./target/release/crystalify
   ```

### Windows (Native Build)

1. Install [Rust via rustup](https://rustup.rs/) and the **Visual Studio C++ Build Tools** (Desktop development with C++ / MSVC toolchain).
2. Build and run in **Windows Terminal**:
   ```powershell
   cargo build --release
   .\target\release\crystalify.exe
   ```

### Windows (Cross-Compilation from Linux)

1. Install the MinGW-w64 toolchain and Rust target:
   ```bash
   rustup target add x86_64-pc-windows-gnu
   ```
   (Install `mingw-w64` via your distribution's package manager, e.g. `sudo apt install mingw-w64` or `sudo pacman -S mingw-w64-gcc`.)
2. Build the `.exe`:
   ```bash
   cargo build --release --target x86_64-pc-windows-gnu
   ```
   The binary will be at `target/x86_64-pc-windows-gnu/release/crystalify.exe`.

### Troubleshooting Logs

To inspect detailed `librespot` diagnostics without interfering with the TUI, set `CRYSTALIFY_LOG`:
```bash
CRYSTALIFY_LOG=crystalify.log ./target/release/crystalify
```

---

## License

This project is licensed under the Apache License, Version 2.0. See the [LICENSE](LICENSE) file for details.
