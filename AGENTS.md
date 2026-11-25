# AGENTS.md — fnfrs (Friday Night Funkin' V-Slice Rust Port)

## 1. Environment & Compiler Rules (CRITICAL)
- **NO LOCAL RUST COMPILER**: `rustc`, `cargo`, and `rustup` are intentionally NOT installed on this machine to avoid SSD wear. **NEVER** attempt to run `cargo`, `rustc`, `rustup`, or `clippy` via shell commands.
- **GitHub Actions Only**: All formatting checks, `cargo clippy`, `cargo test`, and Windows `.exe` release builds run exclusively in `.github/workflows/build.yml` on `windows-latest`.
- **No Hardcoded User Paths**: Never hardcode `C:\Users\...` or machine-specific paths in code or tests. At runtime, resolve `assets/` by checking `<exe_dir>/assets` first, then `<cwd>/assets`.
- **Keep Assets Out of Git**: Never commit game assets (`assets/`), compiled binaries (`*.exe`), or `target/`. Ensure `.gitignore` ignores `/assets/`, `/target/`, and `*.exe`.

## 2. Pinned Crate Versions (Verified on crates.io & docs.rs)
Pin these exact versions in workspace `Cargo.toml` (`[workspace.dependencies]`):
- `wgpu = "30.0.2"`
- `winit = "0.30.13"`
- `pollster = "1.0.3"`
- `cpal = "0.18.2"`
- `symphonia = { version = "0.6.1", features = ["ogg", "vorbis", "mp3", "wav", "pcm"] }`
- `rubato = "5.1.0"`
- `rtrb = "0.4.1"`
- `glam = "0.34.3"`
- `bytemuck = { version = "1.25.2", features = ["derive"] }`
- `image = { version = "0.25.10", default-features = false, features = ["png"] }`
- `roxmltree = "0.21.2"`
- `serde = { version = "1.0.229", features = ["derive"] }`
- `serde_json = "1.0.154"`
- `rustc-hash = "2.1.3"`
- `thiserror = "2.0.21"`
- `anyhow = "1.0.105"`
- `fontdue = "0.9.5"`

### Verified Breaking API Notes (Do Not Use Stale Signatures)
- **`wgpu 30.0.2`**:
  - `wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env())` takes `InstanceDescriptor` **by value** (does not implement `Default`).
  - `SurfaceConfiguration` includes `color_space: wgpu::SurfaceColorSpace::Auto`.
  - `adapter.request_device(&wgpu::DeviceDescriptor { .. })` takes **1 argument** (no trace path parameter).
  - `RenderPipelineDescriptor` and `RenderPassDescriptor` include `multiview_mask: Option<NonZeroU32>` (use `..Default::default()` or set `multiview_mask: None`).
  - `RenderPassColorAttachment` includes `depth_slice: Option<u32>`.
  - `queue.write_texture(TexelCopyTextureInfo { .. }, data, TexelCopyBufferLayout { .. }, Extent3d { .. })`.
  - `queue.present(frame)` replaces `frame.present()`.
- **`symphonia 0.6.1`**:
  - `Probe` lives in `symphonia::core::formats::probe::Probe` (returned by `symphonia::default::get_probe()`).
  - `probe.probe(&hint, mss, fmt_opts, meta_opts)` returns `Result<Box<dyn FormatReader>>` directly (no `ProbeResult` struct).
  - `format_reader.next_packet()` returns `Result<Option<Packet>>` (`Ok(None)` on EOF).
  - `AudioDecoder::decode(&mut self, packet: &Packet)` returns `Result<GenericAudioBufferRef<'_>>`.
- **`winit 0.30.13`**:
  - Implement `winit::application::ApplicationHandler` (`resumed`, `window_event`, `device_event`).
- **`cpal 0.18.2`**:
  - `device.build_output_stream::<f32, _, _>(config, data_cb, err_cb, None)` and `use cpal::traits::StreamTrait; stream.play()?;`.

## 3. Zero-Allocation & Real-Time Audio Invariants
- **Hot Loop Zero-Allocation**: No heap allocations (`Vec` growth, `String` formatting, `format!`, `Box::new`, `HashMap` growth) inside per-frame `update()` or `render()`. Pre-allocate all:
  - Note, sustain trail, splash, hold-cover, and popup pools during song/stage load.
  - GPU `SpriteInstance` buffer and `MeshVertex` buffer (`Vec::with_capacity` at init, `.clear()` each frame without shrinking, `queue.write_buffer` upload).
  - Score HUD formatting via fixed stack byte buffer (`[u8; 32]`) instead of `format!`.
- **Audio Thread Lock-Free Rule**:
  - Pre-decode or ring-buffer PCM `f32` stereo samples at the device sample rate before song start.
  - The `cpal` audio callback must **never** lock a `Mutex`, allocate, or perform I/O.
  - Control commands (`Play`, `Pause`, `Seek`, `SetGain`, `PlaySfx`) cross via `rtrb::Producer` -> `rtrb::Consumer`.
  - Playback position is published via `AtomicU64` (played sample frames) + `Instant` timestamp anchor for sub-frame interpolation on the main/input thread.

## 4. V-Slice Parity Constants
- Virtual canvas: `1280 x 720`. Pixel-art scale: `6.0` with `wgpu::FilterMode::Nearest`.
- Scroll math: `PIXELS_PER_MS = 0.45`, `HIT_WINDOW_MS = 160.0`, `STEPS_PER_BEAT = 4`.
- Strumline layout: `STRUMLINE_SIZE = 104`, `NOTE_SPACING = 112`, `INITIAL_OFFSET = -28.6` (`-0.275 * 104`), `NUDGE = 2.0`, `STRUMLINE_X_OFFSET = 48.0`, `STRUMLINE_Y_OFFSET = 24.0`.
- PBOT1 scoring: `MAX_SCORE = 500`, `MIN_SCORE = 9.0`, `MISS_SCORE = -100`, `PERFECT_MS = 5.0`, `SICK_MS = 45.0`, `GOOD_MS = 90.0`, `BAD_MS = 135.0`, `SHIT_MS = 160.0`, `OFFSET = 54.99`, `SLOPE = 0.080`.
- Sustain clipping: `endOffset = 0.5` (`1.0` pixel), `bottomClip = 0.9` (`1.0` pixel), 8-column UV layout (`1/8` width per segment).

## 5. GitHub Actions CI Setup (`.github/workflows/build.yml`)
- Runner: `windows-latest`, toolchain: `dtolnay/rust-toolchain@stable` with `components: rustfmt, clippy`.
- Cache: `Swatinem/rust-cache@v2`.
- Steps:
  1. `cargo clippy --workspace --all-targets -- -D warnings`
  2. `cargo test --workspace`
  3. `cargo build --release --bin fnfrs-app`
  4. `cargo fmt --all -- --check` (advisory / `continue-on-error: true` after build)
  5. Upload `target/release/fnfrs-app.exe` (renamed to `fnfrs.exe`) via `actions/upload-artifact@v4`.
