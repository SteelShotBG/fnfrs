//! Runtime `libvlc.dll` video cutscene player + `.srt` subtitle overlay.
//!
//! Dynamically loads `libvlc.dll` at runtime on Windows (`LoadLibraryW` / `GetProcAddress`)
//! so compilation and CI tests require zero compile-time VLC SDK headers or `.lib` files.

use std::fmt::Write as _;
use std::fs;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, GpuContext, SpriteBatcher, SpriteInstance, StackTextBuffer,
    TextAlign, TextStyle, TextureHandle,
};

pub const VIDEO_FRAME_WIDTH: u32 = 1280;
pub const VIDEO_FRAME_HEIGHT: u32 = 720;

/// Single parsed `.srt` subtitle cue.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleCue {
    pub start_s: f32,
    pub end_s: f32,
    pub text: String,
}

/// Parses standard `.srt` subtitles (`HH:MM:SS,mmm --> HH:MM:SS,mmm`).
#[must_use]
pub fn parse_srt(raw: &str) -> Vec<SubtitleCue> {
    let mut out = Vec::new();
    let normalized = raw.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let mut lines = block
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty());
        let Some(first) = lines.next() else {
            continue;
        };
        let timing_line = if first.contains("-->") {
            first
        } else if let Some(second) = lines.next() {
            second
        } else {
            continue;
        };
        let Some((start_str, end_str)) = timing_line.split_once("-->") else {
            continue;
        };
        let (Some(start_s), Some(end_s)) =
            (parse_srt_time(start_str.trim()), parse_srt_time(end_str.trim()))
        else {
            continue;
        };
        let body: Vec<&str> = lines.collect();
        if !body.is_empty() {
            out.push(SubtitleCue {
                start_s,
                end_s,
                text: body.join("\n"),
            });
        }
    }
    out
}

fn parse_srt_time(s: &str) -> Option<f32> {
    let clean = s.split_whitespace().next().unwrap_or(s).replace(',', ".");
    let mut parts = clean.split(':');
    let h: f32 = parts.next()?.parse().ok()?;
    let m: f32 = parts.next()?.parse().ok()?;
    let sec: f32 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCutsceneAction {
    None,
    Finished,
}

#[cfg(target_os = "windows")]
mod win32_vlc {
    use std::cell::UnsafeCell;
    use std::ffi::{c_char, c_int, c_uint, c_void, CString};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{VIDEO_FRAME_HEIGHT, VIDEO_FRAME_WIDTH};

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
        fn FreeLibrary(hLibModule: *mut c_void) -> c_int;
        fn GetProcAddress(hModule: *mut c_void, lpProcName: *const c_char) -> *const c_void;
    }

    type FnLibVlcNew = unsafe extern "C" fn(c_int, *const *const c_char) -> *mut c_void;
    type FnLibVlcRelease = unsafe extern "C" fn(*mut c_void);
    type FnMediaNewPath = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
    type FnMediaRelease = unsafe extern "C" fn(*mut c_void);
    type FnMpNewFromMedia = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type FnMpRelease = unsafe extern "C" fn(*mut c_void);
    type FnMpPlay = unsafe extern "C" fn(*mut c_void) -> c_int;
    type FnMpStop = unsafe extern "C" fn(*mut c_void);
    type FnMpGetState = unsafe extern "C" fn(*mut c_void) -> c_int;
    type FnMpSetVol = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    type FnVideoSetCallbacks = unsafe extern "C" fn(
        *mut c_void,
        Option<unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> *mut c_void>,
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const *mut c_void)>,
        Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        *mut c_void,
    );
    type FnVideoSetFormat =
        unsafe extern "C" fn(*mut c_void, *const c_char, c_uint, c_uint, c_uint);

    pub struct SharedVideoFrame {
        pub width: u32,
        pub height: u32,
        staging: UnsafeCell<Vec<u8>>,
        front: Mutex<Vec<u8>>,
        dirty: AtomicBool,
    }

    // SAFETY: `staging` is mutated strictly by VLC's single video decode thread inside
    // paired `lock`/`unlock` callbacks; `front` is guarded by a `Mutex`.
    unsafe impl Sync for SharedVideoFrame {}

    impl SharedVideoFrame {
        fn new(width: u32, height: u32) -> Self {
            let byte_len = (width as usize) * (height as usize) * 4;
            Self {
                width,
                height,
                staging: UnsafeCell::new(vec![0u8; byte_len]),
                front: Mutex::new(vec![0u8; byte_len]),
                dirty: AtomicBool::new(false),
            }
        }

        pub fn copy_dirty_into(&self, dst: &mut [u8]) -> bool {
            if !self.dirty.swap(false, Ordering::AcqRel) {
                return false;
            }
            if let Ok(guard) = self.front.lock() {
                if guard.len() == dst.len() {
                    dst.copy_from_slice(&guard);
                    return true;
                }
            }
            false
        }
    }

    unsafe extern "C" fn vlc_lock(opaque: *mut c_void, planes: *mut *mut c_void) -> *mut c_void {
        let shared = &*(opaque as *const SharedVideoFrame);
        let buf = &mut *shared.staging.get();
        *planes = buf.as_mut_ptr() as *mut c_void;
        std::ptr::null_mut()
    }

    unsafe extern "C" fn vlc_unlock(
        opaque: *mut c_void,
        _picture: *mut c_void,
        _planes: *const *mut c_void,
    ) {
        let shared = &*(opaque as *const SharedVideoFrame);
        let src = &*shared.staging.get();
        if let Ok(mut guard) = shared.front.try_lock() {
            if guard.len() == src.len() {
                guard.copy_from_slice(src);
                shared.dirty.store(true, Ordering::Release);
            }
        }
    }

    unsafe extern "C" fn vlc_display(_opaque: *mut c_void, _picture: *mut c_void) {}

    fn load_proc<T>(module: *mut c_void, name: &[u8]) -> Option<T> {
        // SAFETY: `name` is null-terminated ASCII passed to Win32 `GetProcAddress`.
        unsafe {
            let ptr = GetProcAddress(module, name.as_ptr() as *const c_char);
            if ptr.is_null() {
                None
            } else {
                Some(std::mem::transmute_copy(&ptr))
            }
        }
    }

    fn candidate_vlc_dirs(assets_root: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                dirs.push(dir.to_path_buf());
            }
        }
        if let Some(parent) = assets_root.parent() {
            dirs.push(parent.to_path_buf());
        }
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd);
        }
        dirs
    }

    pub struct VlcPlayerHandle {
        dll: *mut c_void,
        vlc_inst: *mut c_void,
        mp: *mut c_void,
        fn_mp_release: FnMpRelease,
        fn_mp_stop: FnMpStop,
        fn_mp_get_state: FnMpGetState,
        fn_mp_set_vol: Option<FnMpSetVol>,
        fn_vlc_release: FnLibVlcRelease,
        pub shared: Arc<SharedVideoFrame>,
    }

    impl std::fmt::Debug for VlcPlayerHandle {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("VlcPlayerHandle")
                .field("has_mp", &(!self.mp.is_null()))
                .finish()
        }
    }

    impl VlcPlayerHandle {
        pub fn open(video_path: &Path, assets_root: &Path, volume: f32) -> Option<Self> {
            if !video_path.exists() {
                return None;
            }
            let dirs = candidate_vlc_dirs(assets_root);
            let mut loaded_dll: *mut c_void = std::ptr::null_mut();
            let mut plugin_dir: Option<PathBuf> = None;

            for dir in &dirs {
                let candidate = dir.join("libvlc.dll");
                if candidate.exists() {
                    let wide: Vec<u16> = candidate
                        .as_os_str()
                        .to_string_lossy()
                        .encode_utf16()
                        .chain(std::iter::once(0))
                        .collect();
                    // SAFETY: valid UTF-16 null-terminated path string.
                    let m = unsafe { LoadLibraryW(wide.as_ptr()) };
                    if !m.is_null() {
                        loaded_dll = m;
                        let p = dir.join("plugins");
                        if p.exists() {
                            plugin_dir = Some(p);
                        }
                        break;
                    }
                }
            }
            if loaded_dll.is_null() {
                let fallback: Vec<u16> = "libvlc.dll\0".encode_utf16().collect();
                // SAFETY: valid null-terminated string.
                loaded_dll = unsafe { LoadLibraryW(fallback.as_ptr()) };
            }
            if loaded_dll.is_null() {
                return None;
            }

            let fn_new: FnLibVlcNew = match load_proc(loaded_dll, b"libvlc_new\0") {
                Some(f) => f,
                None => {
                    unsafe { FreeLibrary(loaded_dll) };
                    return None;
                }
            };
            let fn_vlc_release: FnLibVlcRelease =
                match load_proc(loaded_dll, b"libvlc_release\0") {
                    Some(f) => f,
                    None => {
                        unsafe { FreeLibrary(loaded_dll) };
                        return None;
                    }
                };
            let fn_media_new: FnMediaNewPath =
                load_proc(loaded_dll, b"libvlc_media_new_path\0")?;
            let fn_media_rel: FnMediaRelease =
                load_proc(loaded_dll, b"libvlc_media_release\0")?;
            let fn_mp_new: FnMpNewFromMedia =
                load_proc(loaded_dll, b"libvlc_media_player_new_from_media\0")?;
            let fn_mp_release: FnMpRelease =
                load_proc(loaded_dll, b"libvlc_media_player_release\0")?;
            let fn_mp_play: FnMpPlay = load_proc(loaded_dll, b"libvlc_media_player_play\0")?;
            let fn_mp_stop: FnMpStop = load_proc(loaded_dll, b"libvlc_media_player_stop\0")?;
            let fn_mp_get_state: FnMpGetState =
                load_proc(loaded_dll, b"libvlc_media_player_get_state\0")?;
            let fn_mp_set_vol: Option<FnMpSetVol> =
                load_proc(loaded_dll, b"libvlc_audio_set_volume\0");
            let fn_set_cb: FnVideoSetCallbacks =
                load_proc(loaded_dll, b"libvlc_video_set_callbacks\0")?;
            let fn_set_fmt: FnVideoSetFormat =
                load_proc(loaded_dll, b"libvlc_video_set_format\0")?;

            let arg_quiet = CString::new("--quiet").ok()?;
            let arg_cfg = CString::new("--ignore-config").ok()?;
            let mut argv_storage = vec![arg_quiet, arg_cfg];
            if let Some(pdir) = plugin_dir {
                if let Ok(s) = CString::new(format!("--plugin-path={}", pdir.display())) {
                    argv_storage.push(s);
                }
            }
            let argv_ptrs: Vec<*const c_char> = argv_storage.iter().map(|s| s.as_ptr()).collect();

            let vlc_inst =
                unsafe { fn_new(argv_ptrs.len() as c_int, argv_ptrs.as_ptr()) };
            if vlc_inst.is_null() {
                unsafe { FreeLibrary(loaded_dll) };
                return None;
            }

            let media_str = video_path.to_string_lossy();
            let c_media = match CString::new(media_str.as_ref()) {
                Ok(c) => c,
                Err(_) => {
                    unsafe {
                        fn_vlc_release(vlc_inst);
                        FreeLibrary(loaded_dll);
                    }
                    return None;
                }
            };

            let media = unsafe { fn_media_new(vlc_inst, c_media.as_ptr()) };
            if media.is_null() {
                unsafe {
                    fn_vlc_release(vlc_inst);
                    FreeLibrary(loaded_dll);
                }
                return None;
            }

            let mp = unsafe { fn_mp_new(media) };
            unsafe { fn_media_rel(media) };
            if mp.is_null() {
                unsafe {
                    fn_vlc_release(vlc_inst);
                    FreeLibrary(loaded_dll);
                }
                return None;
            }

            let shared = Arc::new(SharedVideoFrame::new(
                VIDEO_FRAME_WIDTH,
                VIDEO_FRAME_HEIGHT,
            ));
            let opaque = Arc::as_ptr(&shared) as *mut c_void;
            unsafe {
                fn_set_cb(
                    mp,
                    Some(vlc_lock),
                    Some(vlc_unlock),
                    Some(vlc_display),
                    opaque,
                );
                fn_set_fmt(
                    mp,
                    b"RGBA\0".as_ptr() as *const c_char,
                    VIDEO_FRAME_WIDTH,
                    VIDEO_FRAME_HEIGHT,
                    VIDEO_FRAME_WIDTH * 4,
                );
                if let Some(sv) = fn_mp_set_vol {
                    sv(mp, (volume.clamp(0.0, 1.0) * 100.0).round() as c_int);
                }
                if fn_mp_play(mp) != 0 {
                    fn_mp_release(mp);
                    fn_vlc_release(vlc_inst);
                    FreeLibrary(loaded_dll);
                    return None;
                }
            }

            Some(Self {
                dll: loaded_dll,
                vlc_inst,
                mp,
                fn_mp_release,
                fn_mp_stop,
                fn_mp_get_state,
                fn_mp_set_vol,
                fn_vlc_release,
                shared,
            })
        }

        pub fn set_volume(&self, volume: f32) {
            if let Some(sv) = self.fn_mp_set_vol {
                unsafe {
                    (sv)(self.mp, (volume.clamp(0.0, 1.0) * 100.0).round() as c_int);
                }
            }
        }

        pub fn is_finished(&self) -> bool {
            // libvlc_state_t: Ended = 6, Error = 7
            let st = unsafe { (self.fn_mp_get_state)(self.mp) };
            st == 6 || st == 7
        }
    }

    impl Drop for VlcPlayerHandle {
        fn drop(&mut self) {
            unsafe {
                (self.fn_mp_stop)(self.mp);
                (self.fn_mp_release)(self.mp);
                (self.fn_vlc_release)(self.vlc_inst);
                FreeLibrary(self.dll);
            }
        }
    }
}

/// Active fullscreen video cutscene controller backed by `libvlc.dll` and `.srt` subtitles.
#[derive(Debug)]
pub struct VideoCutsceneState {
    #[cfg(target_os = "windows")]
    vlc: Option<win32_vlc::VlcPlayerHandle>,
    video_tex: TextureHandle,
    frame_scratch: Vec<u8>,
    subtitles: Vec<SubtitleCue>,
    elapsed_s: f32,
    skip_prompt_alpha: f32,
    skip_armed: bool,
    sub_buf: StackTextBuffer<160>,
    pub is_active: bool,
}

impl VideoCutsceneState {
    /// Opens a cutscene video (`assets/videos/videos/<key>.{mp4,mkv}`) with runtime `libvlc.dll`.
    /// Returns `None` if either the video asset or `libvlc.dll` is unavailable.
    pub fn open(
        video_key: &str,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
        volume: f32,
    ) -> Option<Self> {
        let video_path = assets.video(video_key);
        if !video_path.exists() {
            return None;
        }
        let srt_path = video_path.with_extension("srt");
        let subtitles = fs::read_to_string(&srt_path)
            .ok()
            .map(|s| parse_srt(&s))
            .unwrap_or_default();

        #[cfg(target_os = "windows")]
        let vlc = win32_vlc::VlcPlayerHandle::open(&video_path, assets.root(), volume)?;
        #[cfg(not(target_os = "windows"))]
        {
            let _ = volume;
            return None;
        }

        #[cfg(target_os = "windows")]
        {
            let byte_len = (VIDEO_FRAME_WIDTH as usize) * (VIDEO_FRAME_HEIGHT as usize) * 4;
            let frame_scratch = vec![0u8; byte_len];
            let video_tex = gpu
                .create_texture_rgba8(
                    Some("fnfrs-vlc-video"),
                    VIDEO_FRAME_WIDTH,
                    VIDEO_FRAME_HEIGHT,
                    &frame_scratch,
                )
                .ok()?;

            Some(Self {
                vlc: Some(vlc),
                video_tex,
                frame_scratch,
                subtitles,
                elapsed_s: 0.0,
                skip_prompt_alpha: 0.0,
                skip_armed: false,
                sub_buf: StackTextBuffer::new(),
                is_active: true,
            })
        }
    }

    pub fn update(
        &mut self,
        elapsed_s: f32,
        gpu: &GpuContext,
        volume: f32,
        accept_pressed: bool,
        back_pressed: bool,
    ) -> VideoCutsceneAction {
        if !self.is_active {
            return VideoCutsceneAction::None;
        }

        let dt = elapsed_s.clamp(0.0, 0.1);
        self.elapsed_s += dt;

        #[cfg(target_os = "windows")]
        if let Some(vlc) = &self.vlc {
            vlc.set_volume(volume);
            if vlc.shared.copy_dirty_into(&mut self.frame_scratch) {
                let _ = gpu.update_texture_rgba8(self.video_tex, &self.frame_scratch);
            }
            if vlc.is_finished() && self.elapsed_s > 0.35 {
                self.is_active = false;
                self.vlc = None;
                return VideoCutsceneAction::Finished;
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (gpu, volume);
        }

        if back_pressed {
            self.is_active = false;
            #[cfg(target_os = "windows")]
            {
                self.vlc = None;
            }
            return VideoCutsceneAction::Finished;
        }

        if accept_pressed {
            if self.skip_armed {
                self.is_active = false;
                #[cfg(target_os = "windows")]
                {
                    self.vlc = None;
                }
                return VideoCutsceneAction::Finished;
            }
            self.skip_armed = true;
        }

        if self.skip_armed {
            self.skip_prompt_alpha = (self.skip_prompt_alpha + dt * 2.5).min(1.0);
        } else if self.elapsed_s > 1.5 {
            self.skip_prompt_alpha = (self.skip_prompt_alpha + dt * 0.8).min(0.45);
        }

        VideoCutsceneAction::None
    }

    pub fn draw(
        &mut self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if !self.is_active {
            return;
        }

        batcher.push(
            white_tex,
            BlendMode::Normal,
            FilterMode::Nearest,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                .with_color([0.0, 0.0, 0.0, 1.0]),
        );
        batcher.push(
            self.video_tex,
            BlendMode::Normal,
            FilterMode::Linear,
            SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT]),
        );

        let cur_t = self.elapsed_s;
        if let Some(cue) = self
            .subtitles
            .iter()
            .find(|c| cur_t >= c.start_s && cur_t <= c.end_s)
        {
            self.sub_buf.clear();
            let _ = write!(self.sub_buf, "{}", cue.text);
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                self.sub_buf.as_bytes(),
                [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT - 84.0],
                &TextStyle {
                    scale: if font.line_height() <= 10.0 { 2.5 } else { 1.4 },
                    color: [1.0, 1.0, 1.0, 1.0],
                    align: TextAlign::Center,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, 0.95],
            );
        }

        if self.skip_prompt_alpha > 0.02 {
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                b"SKIP [ ENTER / SPACE ]",
                [VIRTUAL_WIDTH - 36.0, VIRTUAL_HEIGHT - 44.0],
                &TextStyle {
                    scale: if font.line_height() <= 10.0 { 2.0 } else { 1.1 },
                    color: [1.0, 1.0, 1.0, self.skip_prompt_alpha],
                    align: TextAlign::Right,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, self.skip_prompt_alpha],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_srt_cues() {
        let srt = "1\n00:00:01,500 --> 00:00:04,250\nHey there!\nLine 2\n\n2\n00:00:05,000 --> 00:00:07,000\nReady?\n";
        let cues = parse_srt(srt);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start_s - 1.5).abs() < 1e-4);
        assert!((cues[0].end_s - 4.25).abs() < 1e-4);
        assert_eq!(cues[0].text, "Hey there!\nLine 2");
        assert_eq!(cues[1].text, "Ready?");
    }
}
