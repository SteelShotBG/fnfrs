use std::fmt::Write as _;

use fnfrs_core::constants::{VIRTUAL_HEIGHT, VIRTUAL_WIDTH};
use fnfrs_render::{
    BitmapFont, BlendMode, FilterMode, SpriteBatcher, SpriteInstance, StackTextBuffer, TextAlign,
    TextStyle, TextureHandle,
};
/// Current phase of [`GameOverState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameOverPhase {
    #[default]
    Inactive,
    /// Playing `firstDeath` anim + `fnf_loss_sfx`.
    FirstDeath,
    /// Playing `deathLoop` anim + `gameOver` looping music.
    DeathLoop,
    /// Confirmed retry; playing `deathConfirm` + `gameOverEnd` and fading to black.
    ConfirmRetry,
    /// Exiting back to menu.
    Exiting,
}

/// Audio cue emitted once on state transitions inside [`GameOverState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOverAudioCue {
    PlayLossSfx,
    StartGameOverLoop,
    PlayGameOverEnd,
    StopMusic,
}

/// High-level state transition requested by [`GameOverState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameOverAction {
    #[default]
    None,
    RetrySong,
    ExitToMenu,
}

/// Per-frame return value from [`GameOverState::update`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GameOverEvents {
    pub audio_cue: Option<GameOverAudioCue>,
    pub action: GameOverAction,
}

/// Zero-allocation game-over substate (`GameOverSubState.hx`).
#[derive(Debug, Clone)]
pub struct GameOverState {
    pub phase: GameOverPhase,
    pub timer_s: f32,
    pub first_death_duration_s: f32,
    pub input_unlock_delay_s: f32,
    pub confirm_fade_duration_s: f32,
    pub can_input: bool,
    pub fade_alpha: f32,
    pub death_count: u32,
    pub is_pixel: bool,
    pub suffix: StackTextBuffer<32>,
}

impl Default for GameOverState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameOverState {
    /// Creates an inactive [`GameOverState`] with V-Slice timing defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: GameOverPhase::Inactive,
            timer_s: 0.0,
            first_death_duration_s: 2.0,
            input_unlock_delay_s: 1.0,
            confirm_fade_duration_s: 2.0,
            can_input: false,
            fade_alpha: 0.0,
            death_count: 0,
            is_pixel: false,
            suffix: StackTextBuffer::new(),
        }
    }

    /// Configures character-specific death audio suffix (e.g. `"-pixel"`, `"-pico"`).
    pub fn set_suffix(&mut self, suffix: &str, is_pixel: bool) {
        self.suffix.clear();
        let _ = self.suffix.write_str(suffix);
        self.is_pixel = is_pixel;
    }

    /// Enters game-over (`FirstDeath`) and emits [`GameOverAudioCue::PlayLossSfx`].
    pub fn trigger_death(&mut self) -> GameOverEvents {
        self.phase = GameOverPhase::FirstDeath;
        self.timer_s = 0.0;
        self.can_input = false;
        self.fade_alpha = 0.0;
        self.death_count = self.death_count.saturating_add(1);
        GameOverEvents {
            audio_cue: Some(GameOverAudioCue::PlayLossSfx),
            action: GameOverAction::None,
        }
    }

    /// Resets back to [`GameOverPhase::Inactive`] (e.g. when restarting song).
    pub fn reset(&mut self) {
        self.phase = GameOverPhase::Inactive;
        self.timer_s = 0.0;
        self.can_input = false;
        self.fade_alpha = 0.0;
    }

    /// Returns the V-Slice sound key for the loss SFX (`gameplay/gameover/fnf_loss_sfx<suffix>`).
    #[must_use]
    pub fn loss_sfx_key<'a>(&self, buf: &'a mut StackTextBuffer<64>) -> &'a str {
        buf.clear();
        let _ = write!(buf, "gameplay/gameover/fnf_loss_sfx{}", self.suffix.as_str());
        buf.as_str()
    }

    /// Returns the V-Slice music key for the loop (`gameplay/gameover/gameOver<suffix>`).
    #[must_use]
    pub fn loop_music_key<'a>(&self, buf: &'a mut StackTextBuffer<64>) -> &'a str {
        buf.clear();
        let _ = write!(buf, "gameplay/gameover/gameOver{}", self.suffix.as_str());
        buf.as_str()
    }

    /// Returns the V-Slice music key for retry confirm (`gameplay/gameover/gameOverEnd<suffix>`).
    #[must_use]
    pub fn end_music_key<'a>(&self, buf: &'a mut StackTextBuffer<64>) -> &'a str {
        buf.clear();
        let _ = write!(buf, "gameplay/gameover/gameOverEnd{}", self.suffix.as_str());
        buf.as_str()
    }

    /// Advances game-over timers, audio cues, and retry/exit handling with zero allocations.
    pub fn update(
        &mut self,
        elapsed_s: f32,
        accept_pressed: bool,
        back_pressed: bool,
    ) -> GameOverEvents {
        let mut events = GameOverEvents::default();
        if self.phase == GameOverPhase::Inactive || self.phase == GameOverPhase::Exiting {
            return events;
        }

        let dt = elapsed_s.max(0.0);
        self.timer_s += dt;

        match self.phase {
            GameOverPhase::FirstDeath => {
                if self.timer_s >= self.input_unlock_delay_s {
                    self.can_input = true;
                }

                if self.can_input && accept_pressed {
                    self.begin_confirm(&mut events);
                } else if back_pressed {
                    self.begin_exit(&mut events);
                } else if self.timer_s >= self.first_death_duration_s {
                    self.phase = GameOverPhase::DeathLoop;
                    self.can_input = true;
                    events.audio_cue = Some(GameOverAudioCue::StartGameOverLoop);
                }
            }
            GameOverPhase::DeathLoop => {
                self.can_input = true;
                if accept_pressed {
                    self.begin_confirm(&mut events);
                } else if back_pressed {
                    self.begin_exit(&mut events);
                }
            }
            GameOverPhase::ConfirmRetry => {
                // Match `GameOverSubState.hx`: hold on `deathConfirm` briefly before black fade.
                let delay = 0.7_f32;
                let dur = self.confirm_fade_duration_s.max(0.8);
                let fade_t = ((self.timer_s - delay) / (dur - delay).max(0.1)).clamp(0.0, 1.0);
                self.fade_alpha = if self.is_pixel {
                    (fade_t * 10.0).floor() / 10.0
                } else {
                    fade_t
                };
                if self.timer_s >= dur {
                    self.reset();
                    events.action = GameOverAction::RetrySong;
                }
            }
            GameOverPhase::Inactive | GameOverPhase::Exiting => {}
        }

        events
    }

    fn begin_confirm(&mut self, events: &mut GameOverEvents) {
        self.phase = GameOverPhase::ConfirmRetry;
        self.timer_s = 0.0;
        self.can_input = false;
        self.fade_alpha = 0.0;
        events.audio_cue = Some(GameOverAudioCue::PlayGameOverEnd);
    }

    fn begin_exit(&mut self, events: &mut GameOverEvents) {
        self.phase = GameOverPhase::Exiting;
        self.death_count = 0;
        events.audio_cue = Some(GameOverAudioCue::StopMusic);
        events.action = GameOverAction::ExitToMenu;
    }

    /// Draws the game-over backdrop, status text, and retry/exit prompt with zero allocations.
    pub fn draw(
        &self,
        batcher: &mut SpriteBatcher,
        white_tex: TextureHandle,
        font: &BitmapFont,
        font_tex: TextureHandle,
    ) {
        if self.phase == GameOverPhase::Inactive {
            return;
        }

        if self.can_input && self.phase != GameOverPhase::ConfirmRetry {
            let is_small = font.line_height() <= 10.0;
            let sub_scale = if is_small { 2.0 } else { 1.05 };
            font.draw_bytes_outlined(
                batcher,
                font_tex,
                b"ENTER: Retry Song   |   ESC: Exit",
                [VIRTUAL_WIDTH * 0.5, VIRTUAL_HEIGHT - 44.0],
                &TextStyle {
                    scale: sub_scale,
                    color: [1.0, 1.0, 1.0, 0.85],
                    align: TextAlign::Center,
                    ..Default::default()
                },
                [0.0, 0.0, 0.0, 0.85],
            );
        }

        if self.fade_alpha > 0.0 {
            batcher.push(
                white_tex,
                BlendMode::Normal,
                FilterMode::Nearest,
                SpriteInstance::new([0.0, 0.0], [VIRTUAL_WIDTH, VIRTUAL_HEIGHT])
                    .with_color([0.0, 0.0, 0.0, self.fade_alpha.clamp(0.0, 1.0)]),
            );
        }
    }
}
