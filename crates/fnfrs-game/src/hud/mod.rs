//! V-Slice HUD components and gameplay substates:
//! - [`HealthIcon`] and [`HealthBarLayout`] (`HealthIcon.hx`, `PlayState.hx`)
//! - [`PauseMenuState`] (`funkin/play/PauseSubState.hx`)
//! - [`GameOverState`] (`funkin/play/GameOverSubState.hx`)
//! - [`ResultsSummaryState`] (`funkin/play/ResultState.hx`)
//!
//! All per-frame `update` and `draw` paths are strictly zero-allocation.

pub mod game_over;
pub mod health_icon;
pub mod pause_menu;
pub mod results;

pub use game_over::{
    GameOverAction, GameOverAudioCue, GameOverEvents, GameOverPhase, GameOverState,
};
pub use health_icon::{
    HealthBarLayout, HealthIcon, HealthIconSheet, HealthIconSparrowFrames, HealthIconState,
    HEALTH_ICON_OVERLAP, HEALTH_LOSING_RATIO, HEALTH_WINNING_RATIO,
};
pub use pause_menu::{
    PauseMenuAction, PauseMenuInput, PauseMenuMode, PauseMenuOption, PauseMenuState,
};
pub use results::{compute_accuracy_percent, ResultsAction, ResultsRank, ResultsSummaryState};

#[cfg(test)]
mod tests {
    use super::*;
    use fnfrs_core::scoring::ScoreTallies;
    use super::*;

    #[test]
    fn icon_frame_selection_for_two_and_three_frame_strips() {
        // 2-frame strip (neutral 0, losing 1)
        assert_eq!(HealthIcon::select_strip_frame(0.10, 2), 1);
        assert_eq!(HealthIcon::select_strip_frame(0.199, 2), 1);
        assert_eq!(HealthIcon::select_strip_frame(0.20, 2), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.50, 2), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.95, 2), 0);

        // 3-frame strip (neutral 0, losing 1, winning 2)
        assert_eq!(HealthIcon::select_strip_frame(0.15, 3), 1);
        assert_eq!(HealthIcon::select_strip_frame(0.50, 3), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.80, 3), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.85, 3), 2);

        // 1-frame icon always stays on 0
        assert_eq!(HealthIcon::select_strip_frame(0.05, 1), 0);
        assert_eq!(HealthIcon::select_strip_frame(0.95, 1), 0);
    }

    #[test]
    fn health_bar_split_and_icon_positions_match_vslice() {
        let bar = HealthBarLayout::compute(601.0, 19.0, false, 1.0);
        // 50% health => split_x is exact center of bar (640.0)
        assert!((bar.split_x - 640.0).abs() < 1e-4);
        assert!((bar.bg_y - 648.0).abs() < 1e-4);

        // Player icon on right of split with 26px overlap
        let p_x = bar.icon_x(true, 150.0);
        assert!((p_x - (640.0 - 26.0)).abs() < 1e-4);

        // Opponent icon on left of split with 26px overlap
        let o_x = bar.icon_x(false, 150.0);
        assert!((o_x - (640.0 - (150.0 - 26.0))).abs() < 1e-4);

        // Y centered on healthBarBG
        let y = bar.icon_y_centered(150.0);
        assert!((y - (648.0 + (19.0 - 150.0) * 0.5)).abs() < 1e-4);

        // Downscroll moves health bar to 10% height (72.0)
        let down_bar = HealthBarLayout::compute(601.0, 19.0, true, 2.0);
        assert!((down_bar.bg_y - 72.0).abs() < 1e-4);
        assert!((down_bar.split_x - down_bar.bar_x).abs() < 1e-4);
    }

    #[test]
    fn health_icon_bop_decays_exponentially() {
        let mut icon = HealthIcon::placeholder("bf", true, false);
        let bar = HealthBarLayout::compute(601.0, 19.0, false, 1.0);
        icon.bop();
        assert!((icon.bop_scale_mult - 1.2).abs() < 1e-6);

        icon.update(0.1, 1.0, &bar);
        assert!(icon.bop_scale_mult < 1.2 && icon.bop_scale_mult > 1.0);

        for _ in 0..20 {
            icon.update(0.05, 1.0, &bar);
        }
        assert!((icon.bop_scale_mult - 1.0).abs() < 1e-3);
    }

    #[test]
    fn pause_menu_navigation_wraps_around_and_guards_first_frame() {
        let mut menu = PauseMenuState::new("Bopeebo", "normal");
        menu.open(false, false, 1);
        let count = menu.item_count();
        assert_eq!(count, 7);
        assert_eq!(menu.selected, 0);

        // Wrap upward from 0 -> last item
        menu.move_selection(-1);
        assert_eq!(menu.selected, count - 1);

        // Wrap downward from last -> 0
        menu.move_selection(1);
        assert_eq!(menu.selected, 0);

        // First frame ignores accept so opening key doesn't immediately resume
        let first = menu.update(
            0.016,
            PauseMenuInput {
                accept_pressed: true,
                ..Default::default()
            },
        );
        assert_eq!(first, PauseMenuAction::None);
        assert!(menu.is_open);

        // Next frame toggles botplay
        menu.move_selection(4);
        assert_eq!(menu.selected_main_option(), Some(PauseMenuOption::ToggleBotplay));
        let act = menu.update(
            0.016,
            PauseMenuInput {
                accept_pressed: true,
                ..Default::default()
            },
        );
        assert_eq!(act, PauseMenuAction::ToggleBotplay);
        assert!(menu.is_botplay);
    }

    #[test]
    fn game_over_transitions_and_cues() {
        let mut go = GameOverState::new();
        let start = go.trigger_death();
        assert_eq!(go.phase, GameOverPhase::FirstDeath);
        assert_eq!(start.audio_cue, Some(GameOverAudioCue::PlayLossSfx));

        // Before 1s unlock, accept is ignored
        let early = go.update(0.5, true, false);
        assert_eq!(early.action, GameOverAction::None);
        assert_eq!(go.phase, GameOverPhase::FirstDeath);

        // Crossing 2s transitions to DeathLoop and emits StartGameOverLoop
        let to_loop = go.update(1.6, false, false);
        assert_eq!(go.phase, GameOverPhase::DeathLoop);
        assert_eq!(to_loop.audio_cue, Some(GameOverAudioCue::StartGameOverLoop));

        // Accept starts confirm & gameOverEnd, then completes retry after fade
        let confirm = go.update(0.016, true, false);
        assert_eq!(go.phase, GameOverPhase::ConfirmRetry);
        assert_eq!(confirm.audio_cue, Some(GameOverAudioCue::PlayGameOverEnd));

        let done = go.update(2.1, false, false);
        assert_eq!(done.action, GameOverAction::RetrySong);
        assert_eq!(go.phase, GameOverPhase::Inactive);
    }

    #[test]
    fn results_rank_and_accuracy_calculation() {
        let perfect_gold = ScoreTallies {
            sick: 100,
            total_notes: 100,
            total_notes_hit: 100,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&perfect_gold), ResultsRank::PerfectGold);
        assert!((compute_accuracy_percent(&perfect_gold) - 100.0).abs() < 1e-6);

        let perfect = ScoreTallies {
            sick: 95,
            good: 5,
            total_notes: 100,
            total_notes_hit: 100,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&perfect), ResultsRank::Perfect);

        let excellent = ScoreTallies {
            sick: 80,
            good: 12,
            bad: 6,
            missed: 2,
            total_notes: 100,
            total_notes_hit: 98,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&excellent), ResultsRank::Excellent);

        let great = ScoreTallies {
            sick: 70,
            good: 12,
            bad: 15,
            missed: 2,
            total_notes: 100,
            total_notes_hit: 97,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&great), ResultsRank::Great);

        let good = ScoreTallies {
            sick: 50,
            good: 15,
            bad: 30,
            missed: 5,
            total_notes: 100,
            total_notes_hit: 95,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&good), ResultsRank::Good);

        let loss = ScoreTallies {
            sick: 20,
            good: 10,
            missed: 70,
            total_notes: 100,
            total_notes_hit: 30,
            ..Default::default()
        };
        assert_eq!(ResultsRank::from_tallies(&loss), ResultsRank::Loss);
    }
}
