//! Pre-parsed, zero-allocation V-Slice song event queue (`SongEventRegistry.hx`
//! and `funkin/play/event/*`).

use fnfrs_core::constants::{DEFAULT_ZOOM_RATE, STEPS_PER_BEAT};
use fnfrs_core::schema::song::{SongChartData, SongEventData};

use crate::camera::FlxEase;

/// Target character or world-space position for `FocusCamera`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraFocusTarget {
    Position,
    #[default]
    Player,
    Opponent,
    Girlfriend,
    Unknown(i32),
}

impl CameraFocusTarget {
    /// Maps V-Slice integer character IDs (`-1 = pos, 0 = bf, 1 = dad, 2 = gf`).
    #[must_use]
    pub const fn from_char_id(id: i32) -> Self {
        match id {
            -1 => Self::Position,
            0 => Self::Player,
            1 => Self::Opponent,
            2 => Self::Girlfriend,
            other => Self::Unknown(other),
        }
    }
}

/// Easing mode for `FocusCamera`: classic follow lerp (`0.04`) or explicit tween.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusCameraEase {
    #[default]
    Classic,
    Tween(FlxEase),
}

/// Interpretation of `ZoomCamera`'s `zoom` parameter (`ZoomCameraSongEvent.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZoomMode {
    #[default]
    Direct,
    Stage,
}

/// Target strumline(s) for `ScrollSpeed` (`ScrollSpeedEvent.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollSpeedTarget {
    #[default]
    Both,
    Player,
    Opponent,
}

/// Resolved target entity for `PlayAnimation` (`PlayAnimationSongEvent.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationTarget<'a> {
    Boyfriend,
    Dad,
    Girlfriend,
    Prop(&'a str),
}

/// Typed action emitted when a chart [`SongEventData`] fires at `time_ms`.
#[derive(Debug, Clone, PartialEq)]
pub enum GameEventAction {
    FocusCamera {
        target: CameraFocusTarget,
        offset: [f32; 2],
        duration_steps: f32,
        ease: FocusCameraEase,
    },
    ZoomCamera {
        zoom: f32,
        duration_steps: f32,
        mode: ZoomMode,
        ease: FlxEase,
    },
    SetCameraBop {
        rate_beats: f32,
        offset_beats: f32,
        intensity: f32,
    },
    PlayAnimation {
        target: String,
        anim: String,
        force: bool,
    },
    ScrollSpeed {
        scroll: f32,
        duration_steps: f32,
        ease: FlxEase,
        strumline: ScrollSpeedTarget,
        absolute: bool,
    },
    Custom {
        event_kind: String,
        value: serde_json::Value,
    },
}

impl GameEventAction {
    /// Returns `true` if this event should still execute when skipped past by > 1000 ms
    /// (`SongEvent.processOldEvents`).
    #[must_use]
    pub const fn process_old_events(&self) -> bool {
        matches!(
            self,
            Self::FocusCamera { .. }
                | Self::ZoomCamera { .. }
                | Self::SetCameraBop { .. }
                | Self::ScrollSpeed { .. }
        )
    }

    /// Resolves `PlayAnimation.target` against V-Slice character aliases (`bf`, `dad`, `gf`).
    #[must_use]
    pub fn resolve_animation_target(target: &str) -> AnimationTarget<'_> {
        if target == "boyfriend" || target == "bf" || target == "player" {
            AnimationTarget::Boyfriend
        } else if target == "dad" || target == "opponent" {
            AnimationTarget::Dad
        } else if target == "girlfriend" || target == "gf" {
            AnimationTarget::Girlfriend
        } else {
            AnimationTarget::Prop(target)
        }
    }

    /// Parses a single [`SongEventData`] into a typed [`GameEventAction`].
    #[must_use]
    pub fn parse(raw: &SongEventData) -> Self {
        match raw.event_kind.as_str() {
            "FocusCamera" => parse_focus_camera(raw),
            "ZoomCamera" => parse_zoom_camera(raw),
            "SetCameraBop" => parse_set_camera_bop(raw),
            "PlayAnimation" => parse_play_animation(raw),
            "ScrollSpeed" => parse_scroll_speed(raw),
            _ => Self::Custom {
                event_kind: raw.event_kind.clone(),
                value: raw.value.clone(),
            },
        }
    }
}

/// Pre-parsed chart event stored inside [`SongEventQueue`].
#[derive(Debug, Clone, PartialEq)]
pub struct QueuedSongEvent {
    pub time_ms: f64,
    pub action: GameEventAction,
}

/// Zero-allocation runtime queue of pre-parsed song events sorted by `time_ms`.
#[derive(Debug, Clone, Default)]
pub struct SongEventQueue {
    events: Vec<QueuedSongEvent>,
    cursor: usize,
}

impl SongEventQueue {
    /// Builds and sorts a [`SongEventQueue`] from a chart's `events` list.
    #[must_use]
    pub fn from_chart(chart: &SongChartData) -> Self {
        Self::from_raw_events(&chart.events)
    }

    /// Builds and sorts a [`SongEventQueue`] from raw [`SongEventData`] entries.
    #[must_use]
    pub fn from_raw_events(raw_events: &[SongEventData]) -> Self {
        let mut events = Vec::with_capacity(raw_events.len());
        for raw in raw_events {
            events.push(QueuedSongEvent {
                time_ms: raw.time,
                action: GameEventAction::parse(raw),
            });
        }
        events.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        Self { events, cursor: 0 }
    }

    /// Total number of pre-parsed events in the queue.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Returns `true` when the queue contains no events.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Index of the next untriggered event.
    #[inline]
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Number of events remaining at or after `cursor`.
    #[inline]
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.events.len().saturating_sub(self.cursor)
    }

    /// Resets the cursor to the first event at `time_ms >= seek_time_ms` (skipping earlier ones).
    /// Zero heap allocations.
    pub fn reset_to_time_ms(&mut self, seek_time_ms: f64) {
        self.cursor = self.events.partition_point(|ev| ev.time_ms < seek_time_ms);
    }

    /// Advances the cursor past all events with `time_ms <= song_position_ms` and returns
    /// them as a contiguous borrowed slice with **zero heap allocations**.
    pub fn poll(&mut self, song_position_ms: f64) -> &[QueuedSongEvent] {
        let start = self.cursor;
        while self.cursor < self.events.len()
            && self.events[self.cursor].time_ms <= song_position_ms
        {
            self.cursor += 1;
        }
        &self.events[start..self.cursor]
    }

    /// Advances the cursor up to `song_position_ms` and invokes `handler` for each newly
    /// activated event with **zero heap allocations**.
    ///
    /// Events older than `1000.0` ms whose [`GameEventAction::process_old_events`] is `false`
    /// are skipped, matching `PlayState.processSongEvents`.
    pub fn update<F>(&mut self, song_position_ms: f64, mut handler: F)
    where
        F: FnMut(&QueuedSongEvent),
    {
        let start = self.cursor;
        while self.cursor < self.events.len()
            && self.events[self.cursor].time_ms <= song_position_ms
        {
            self.cursor += 1;
        }
        for ev in &self.events[start..self.cursor] {
            let age_ms = song_position_ms - ev.time_ms;
            if age_ms > 1000.0 && !ev.action.process_old_events() {
                continue;
            }
            handler(ev);
        }
    }
}

fn parse_focus_camera(raw: &SongEventData) -> GameEventAction {
    let char_id = raw
        .get_i64("char")
        .or_else(|| raw.as_i64())
        .or_else(|| raw.value.as_str().and_then(|s| s.parse::<i64>().ok()))
        .unwrap_or(0) as i32;

    let x = raw.get_f64("x").unwrap_or(0.0) as f32;
    let y = raw.get_f64("y").unwrap_or(0.0) as f32;
    let duration_steps = raw.get_f64("duration").unwrap_or(STEPS_PER_BEAT as f64) as f32;

    let ease_str = raw.get_str("ease").unwrap_or("CLASSIC").trim();
    let ease_dir = raw.get_str("easeDir");

    let ease = if ease_str.eq_ignore_ascii_case("classic") {
        FocusCameraEase::Classic
    } else if let Some(flx) = FlxEase::parse_with_dir(ease_str, ease_dir) {
        FocusCameraEase::Tween(flx)
    } else {
        FocusCameraEase::Classic
    };

    GameEventAction::FocusCamera {
        target: CameraFocusTarget::from_char_id(char_id),
        offset: [x, y],
        duration_steps,
        ease,
    }
}

fn parse_zoom_camera(raw: &SongEventData) -> GameEventAction {
    let zoom = raw
        .get_f64("zoom")
        .or_else(|| raw.as_f64())
        .or_else(|| raw.value.as_str().and_then(|s| s.parse::<f64>().ok()))
        .unwrap_or(1.0) as f32;

    let duration_steps = raw.get_f64("duration").unwrap_or(STEPS_PER_BEAT as f64) as f32;
    let mode = match raw.get_str("mode") {
        Some(m) if m.eq_ignore_ascii_case("stage") => ZoomMode::Stage,
        _ => ZoomMode::Direct,
    };

    let ease_str = raw.get_str("ease").unwrap_or("linear");
    let ease_dir = raw.get_str("easeDir");
    let ease = FlxEase::parse_with_dir(ease_str, ease_dir).unwrap_or(FlxEase::Linear);

    GameEventAction::ZoomCamera {
        zoom,
        duration_steps,
        mode,
        ease,
    }
}

fn parse_set_camera_bop(raw: &SongEventData) -> GameEventAction {
    let rate_beats = raw.get_f64("rate").unwrap_or(f64::from(DEFAULT_ZOOM_RATE)) as f32;
    let offset_beats = raw.get_f64("offset").unwrap_or(0.0) as f32;
    let intensity = raw.get_f64("intensity").unwrap_or(1.0) as f32;

    GameEventAction::SetCameraBop {
        rate_beats,
        offset_beats,
        intensity,
    }
}

fn parse_play_animation(raw: &SongEventData) -> GameEventAction {
    let target = raw.get_str("target").unwrap_or("boyfriend").to_string();
    let anim = raw.get_str("anim").unwrap_or("idle").to_string();
    let force = raw
        .get_bool("force")
        .or_else(|| raw.get_str("force").and_then(|s| s.parse::<bool>().ok()))
        .unwrap_or(false);

    GameEventAction::PlayAnimation {
        target,
        anim,
        force,
    }
}

fn parse_scroll_speed(raw: &SongEventData) -> GameEventAction {
    let scroll = raw.get_f64("scroll").unwrap_or(1.0) as f32;
    let duration_steps = raw.get_f64("duration").unwrap_or(STEPS_PER_BEAT as f64) as f32;
    let ease_str = raw.get_str("ease").unwrap_or("linear");
    let ease_dir = raw.get_str("easeDir");
    let ease = FlxEase::parse_with_dir(ease_str, ease_dir).unwrap_or(FlxEase::Linear);

    let strumline = match raw.get_str("strumline") {
        Some(s) if s.eq_ignore_ascii_case("player") => ScrollSpeedTarget::Player,
        Some(s) if s.eq_ignore_ascii_case("opponent") => ScrollSpeedTarget::Opponent,
        _ => ScrollSpeedTarget::Both,
    };

    let absolute = raw
        .get_bool("absolute")
        .or_else(|| raw.get_str("absolute").and_then(|s| s.parse::<bool>().ok()))
        .unwrap_or(false);

    GameEventAction::ScrollSpeed {
        scroll,
        duration_steps,
        ease,
        strumline,
        absolute,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn parses_focus_camera_int_and_object_variants() {
        let int_ev: SongEventData =
            serde_json::from_str(r#"{"t": 0, "e": "FocusCamera", "v": 1}"#).expect("json");
        assert_eq!(
            GameEventAction::parse(&int_ev),
            GameEventAction::FocusCamera {
                target: CameraFocusTarget::Opponent,
                offset: [0.0, 0.0],
                duration_steps: 4.0,
                ease: FocusCameraEase::Classic,
            }
        );

        let obj_ev: SongEventData = serde_json::from_str(
            r#"{"t": 1200, "e": "FocusCamera", "v": {
                "char": 2, "x": 15.5, "y": -10,
                "duration": 8, "ease": "quad", "easeDir": "Out"
            }}"#,
        )
        .expect("json");
        assert_eq!(
            GameEventAction::parse(&obj_ev),
            GameEventAction::FocusCamera {
                target: CameraFocusTarget::Girlfriend,
                offset: [15.5, -10.0],
                duration_steps: 8.0,
                ease: FocusCameraEase::Tween(FlxEase::QuadOut),
            }
        );

        let pos_ev: SongEventData = serde_json::from_str(
            r#"{"t": 2000, "e": "FocusCamera", "v": {
                "char": -1, "x": 500, "y": 300, "ease": "INSTANT"
            }}"#,
        )
        .expect("json");
        assert_eq!(
            GameEventAction::parse(&pos_ev),
            GameEventAction::FocusCamera {
                target: CameraFocusTarget::Position,
                offset: [500.0, 300.0],
                duration_steps: 4.0,
                ease: FocusCameraEase::Tween(FlxEase::Instant),
            }
        );
    }

    #[test]
    fn parses_zoom_camera_direct_and_stage_modes() {
        let direct_scalar: SongEventData =
            serde_json::from_str(r#"{"t": 500, "e": "ZoomCamera", "v": 1.3}"#).expect("json");
        match GameEventAction::parse(&direct_scalar) {
            GameEventAction::ZoomCamera {
                zoom,
                duration_steps,
                mode,
                ease,
            } => {
                assert!(approx(zoom, 1.3, 1e-5));
                assert!(approx(duration_steps, 4.0, 1e-5));
                assert_eq!(mode, ZoomMode::Direct);
                assert_eq!(ease, FlxEase::Linear);
            }
            other => panic!("unexpected variant: {other:?}"),
        }

        let stage_obj: SongEventData = serde_json::from_str(
            r#"{"t": 1000, "e": "ZoomCamera", "v": {
                "zoom": 1.15, "duration": 2.5, "mode": "stage", "ease": "sineInOut"
            }}"#,
        )
        .expect("json");
        match GameEventAction::parse(&stage_obj) {
            GameEventAction::ZoomCamera {
                zoom,
                duration_steps,
                mode,
                ease,
            } => {
                assert!(approx(zoom, 1.15, 1e-5));
                assert!(approx(duration_steps, 2.5, 1e-5));
                assert_eq!(mode, ZoomMode::Stage);
                assert_eq!(ease, FlxEase::SineInOut);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn parses_bop_play_anim_scroll_speed_and_custom() {
        let bop: SongEventData = serde_json::from_str(
            r#"{"t": 0, "e": "SetCameraBop", "v": {"rate": 2, "intensity": 1.5}}"#,
        )
        .expect("json");
        assert_eq!(
            GameEventAction::parse(&bop),
            GameEventAction::SetCameraBop {
                rate_beats: 2.0,
                offset_beats: 0.0,
                intensity: 1.5,
            }
        );

        let anim: SongEventData = serde_json::from_str(
            r#"{"t": 100, "e": "PlayAnimation", "v": {
                "target": "dad", "anim": "hey", "force": true
            }}"#,
        )
        .expect("json");
        assert_eq!(
            GameEventAction::parse(&anim),
            GameEventAction::PlayAnimation {
                target: "dad".to_string(),
                anim: "hey".to_string(),
                force: true,
            }
        );

        let speed: SongEventData = serde_json::from_str(
            r#"{"t": 200, "e": "ScrollSpeed", "v": {
                "scroll": "1.4", "duration": "8", "ease": "cubicOut",
                "strumline": "player", "absolute": true
            }}"#,
        )
        .expect("json");
        assert_eq!(
            GameEventAction::parse(&speed),
            GameEventAction::ScrollSpeed {
                scroll: 1.4,
                duration_steps: 8.0,
                ease: FlxEase::CubicOut,
                strumline: ScrollSpeedTarget::Player,
                absolute: true,
            }
        );

        let custom: SongEventData = serde_json::from_str(
            r#"{"t": 300, "e": "CustomDialogueEvent", "v": {"line": "hi"}}"#,
        )
        .expect("json");
        assert!(matches!(
            GameEventAction::parse(&custom),
            GameEventAction::Custom { .. }
        ));
    }

    #[test]
    fn queue_sorts_advances_cursor_and_resets_on_seek() {
        let raw: Vec<SongEventData> = serde_json::from_str(
            r#"[
                {"t": 2000.0, "e": "FocusCamera", "v": 0},
                {"t": 0.0, "e": "FocusCamera", "v": 1},
                {"t": 1000.0, "e": "ZoomCamera", "v": 1.2}
            ]"#,
        )
        .expect("json");

        let mut queue = SongEventQueue::from_raw_events(&raw);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.cursor(), 0);

        let batch1 = queue.poll(500.0);
        assert_eq!(batch1.len(), 1);
        assert!((batch1[0].time_ms - 0.0).abs() < 1e-6);
        assert_eq!(queue.cursor(), 1);

        let batch2 = queue.poll(1500.0);
        assert_eq!(batch2.len(), 1);
        assert!((batch2[0].time_ms - 1000.0).abs() < 1e-6);
        assert_eq!(queue.cursor(), 2);

        // Seek back to 1000.0 -> 1000.0 is >= 1000.0 so it triggers again; 0.0 stays skipped.
        queue.reset_to_time_ms(1000.0);
        assert_eq!(queue.cursor(), 1);
        let batch3 = queue.poll(2500.0);
        assert_eq!(batch3.len(), 2);
        assert!((batch3[0].time_ms - 1000.0).abs() < 1e-6);
        assert!((batch3[1].time_ms - 2000.0).abs() < 1e-6);
        assert_eq!(queue.remaining(), 0);

        // Seek back to 0.0 -> event at 0.0 triggers again.
        queue.reset_to_time_ms(0.0);
        assert_eq!(queue.cursor(), 0);
    }
}
