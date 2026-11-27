# fnfrs Architecture & Handoff Guide

`fnfrs` is a Rust port of **Friday Night Funkin' V-Slice** built as a 5-crate workspace targeting `1280x720` virtual resolution with `wgpu`, `winit`, and `cpal`.

> **CRITICAL WORKFLOW RULE (see `AGENTS.md`)**: `rustc`, `cargo`, and `rustup` are **NOT** installed on this machine. Never run `cargo`/`rustc`/`clippy` locally. All compilation, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and Windows release builds run exclusively in `.github/workflows/build.yml` on `windows-latest` after pushing to `main`.

---

## 1. Workspace Crate Map

| Crate | Purpose & Key Modules |
|---|---|
| `crates/fnfrs-core` | Asset path resolution (`paths.rs`), V-Slice constants (`constants.rs`), fixed-slot object pool (`pool.rs`), BPM/step/beat `Conductor` (`conductor.rs`), PBOT1 scoring (`scoring.rs`), portable `<exe_dir>/save.json` persistence (`save.rs`), and JSON schemas (`schema/{character,dialogue,level,notestyle,song,stage}.rs`). |
| `crates/fnfrs-audio` | Symphonia OGG/Vorbis/MP3/WAV/PCM decoder (`decoder.rs` — handles truncated `.ogg` streams lacking trailing EOS via `UnexpectedEof` break + `ResetRequired`), lock-free `cpal` mixer (`mixer.rs` — song stems, SFX bank, and independent `StartBgLoop`/`StopBgLoop` for pause/dialogue music), and `AtomicU64` + `Instant` `SampleClock` (`clock.rs`). |
| `crates/fnfrs-render` | `wgpu` context & texture slot management (`gpu.rs` — supports `unload_texture`, `clear_user_textures`, and in-place `update_texture_rgba8`), instanced 2D sprite batcher (`batch.rs` — `Normal`, `Add`, `Screen`, `Multiply`, `AdjustColor`, `WiggleEffect`), sustain mesh batcher (`mesh_batch.rs`), fullscreen `RuntimeRainShader` post-pass (`rain.rs`), WGSL shaders (`shaders.rs`, `shaders/sprite.wgsl`), bitmap font (`text.rs`), `AtlasFont` (`atlas_text.rs` for `bold.xml`/`default.xml`), and texture atlases (`atlas/{animate,packer,sparrow}.rs`). |
| `crates/fnfrs-game` | Core gameplay (`play_state.rs`), strumlines & receptors (`strumline.rs`), sustain trails & hold covers (`sustain.rs`), note styles & popup PNGs (`note_style.rs`), popup pool (`popup.rs`), input ring buffers & rebindable `KeyBinds` (`input.rs`), camera rig (`camera.rs`), character & animation banks (`character.rs`), depth-sorted stages & boppers (`stage.rs`), countdown (`countdown.rs`), pause/gameover/results HUD (`hud.rs`), native `.hxc` ports (`scripts.rs`), Week 6 dialogues (`dialogue.rs`), `libvlc.dll` video player (`video.rs`), cutscene coordinator (`cutscene.rs`), `SoundTray` (`sound_tray.rs`), `F3` debug overlay (`debug_display.rs`), and menus (`menus/{title,main_menu,story_menu,freeplay,options,char_select}.rs`). |
| `crates/fnfrs-app` | `winit` event loop, lazy screen lifecycle, GPU/audio resource purging on transitions, global `-`/`+`/`0` SoundTray & `F3` DebugDisplay dispatch, `save.json` sync, and gameplay/cutscene orchestration (`src/main.rs`). |

---

## 2. Critical Invariants & Patterns

1. **Zero-Allocation Hot Loops**:
   - `PlayScene::update`, `PlayScene::draw`, and all per-frame `draw()`/`update()` paths must perform **0 heap allocations** (`crates/fnfrs-game/tests/zero_alloc_hot_loop.rs` enforces this with a custom `#[global_allocator]` over 1,000 frames).
   - Use `StackTextBuffer<N>`, fixed-size arrays (`[T; N]`), `Vec::with_capacity` allocated at load time + `.clear()` without shrinking, and `PoolHandle` object pools (`pool.rs`).
2. **Lock-Free Audio Thread**:
   - `cpal` callback (`mixer.rs`) never locks a `Mutex`, allocates, or performs I/O.
   - Commands cross via `rtrb::Producer` -> `Consumer`; replaced `Box<SongTracks>` / `Box<SfxBank>` / `Box<DecodedAudio>` return via `RetiredResource` and are freed on the main thread via `engine.reclaim_retired()`.
3. **Memory Management & Lazy Screen Loading**:
   - `RuntimeState` (`main.rs`) keeps **only the active screen** allocated (`Option<TitleScreenState>` etc.). Screen transitions call `gpu.clear_user_textures()`, re-upload `hud_font` + `SoundTray` textures, and reclaim retired audio buffers.
   - `StoryMenuScreenState` (`story_menu.rs`) lazily loads/unloads week prop textures via `sync_active_week` + `gpu.unload_texture` so all 10 weeks never sit in VRAM simultaneously.
4. **Adobe Animate Atlas Masking**:
   - `animate.rs` skips layers with `LT` / `Layer_type` equal to `"Clp"`, `"Clipper"`, `"Gd"`, or `"Guide"` (`CompiledLayer::is_mask`), preventing Flash mask/clipper layers (such as `sserafim-gf`'s `"blue mask"`) from rendering as opaque shapes.
5. **Runtime `libvlc.dll` Video Cutscenes**:
   - `video.rs` loads `libvlc.dll` dynamically on Windows via `LoadLibraryW` / `GetProcAddress` (`kernel32`) so CI needs no VLC SDK or `.lib` files. Non-Windows builds compile cleanly and return `None` from `VideoCutsceneState::open`.
   - Decodes `RGBA` frames (`1280x720`) via double-buffered `SharedVideoFrame` (`staging` -> `front` mutex -> `gpu.update_texture_rgba8`) with optional `.srt` subtitle rendering.

---

## 3. Current Status (as of commit `4c90d99` on `main`)

### Implemented
- **Audio & Asset Resolution**: Truncated OGG fix (`2hot` Darnell voices), nested `music/<key>/<key>.ogg` resolution (`freakyMenu`, `breakfast`, `results*`), `StartBgLoop`/`StopBgLoop`.
- **Save & Global Overlays**: `<exe_dir>/save.json` (`SaveData`), `SoundTray` (`-`/`+`/`0`), `F3` `FunkinDebugDisplay` (replaces permanent top bar), `AtlasFont` (`bold.xml` / `default.xml` with bare-`&` XML sanitization).
- **Menus & Gameplay HUD**:
  - `TitleScreenState` (`AtlasFont` intro text), `MainMenuScreenState` (`140px` spacing, `1.1x` BG, `Options`), `StoryMenuScreenState` (`frameIndices`, `danceLeft`/`danceRight`, lazy week prop loading), `FreeplayScreenState` (`AtlasFont`, `[F]` favorites, `[C]` `CharSelect`, saved highscores/ranks).
  - `OptionsScreenState` (Preferences, 4-lane Controls rebinding, Audio Offset) & `CharSelectScreenState` (`bf` / `pico`).
  - `PauseMenuState` (`breakfast.ogg`, Practice Mode toggle), `GameOverState` (visible centered BF + black stage), `ResultsSummaryState` (`ResultsVisuals` + `results*` music).
  - Real `ui/popup/{funkin,pixel}/` rating & combo digit PNGs and anti-aliased `vcr-bmp` score text (`crisp = false`, `letter_spacing: 0.0`).
- **Shaders, Scripts & Stages (`sserafim` included, editors excluded)**:
  - `BlendMode::Multiply` + `SPRITE_FLAG_WIGGLE` (`sprite.wgsl` reusing `hsbc`).
  - `Nene` knife state machine (`owns_gf_dance`), `spookyMansionErect` double-flash silhouettes + rain, `phillyTrainErect` lights + train + SFX, `phillyBlazin` (`blazin` centered single strumline, `skyBlur`, lightning + `Lightning1..3`), `schoolEvil`/`schoolEvilErect` wavy props, `2hot` zero-alloc `SpraycanPool`, and `sserafim` (`spaghetti`) extra members (`yunjin`, `chaewon`, `eunchae`), dust, lights, and custom events.
- **Cutscenes & Week 6 Dialogues**:
  - `DialogueState` (`senpai`, `senpai-pico`, `roses`, `roses-pico`, `thorns`, censored variants), in-game intros/outros (`winter-horrorland`, `thorns` `senpaiCrazy`, `eggnog-erect`), and `libvlc.dll` video cutscenes (`ugh`, `guns`, `stress*`, `darnell`, `2hot`, `blazin`).

### Pending / Next Steps for New Chat
1. **Check GitHub Actions CI** for commit `4c90d99` (`cargo clippy`, `cargo test --workspace`, and `fnfrs.exe` build) and fix any remaining compiler/clippy warnings if any crate downstream of `fnfrs-render` reports one.
2. **Runtime Playtesting**: Test the built `fnfrs.exe` against the game's `assets/` + `libvlc.dll` (`plugins/`), verifying:
   - `2hot` voices & spraycan shooting, Nene's knife animations, `sserafim` (`spaghetti`) visuals & events, `phillyTrainErect` / `spookyMansionErect` / `phillyBlazin`.
   - Story Mode Week 2 props/animations, Main Menu layout/music, `save.json` persistence, Week 6 dialogues, and `libvlc.dll` video cutscenes.
3. **Reference Assets (Inspection Only)**: Official V-Slice assets and Haxe `.hxc` reference scripts are **not** in git (`AGENTS.md`); when inspecting locally, check the user's extracted game directory (`assets/scripts/**/*.hxc`, `assets/data/**/*.json`) and never hardcode user paths in Rust source.
