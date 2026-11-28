//! Native Rust combat choreography matching `pico-blazin.hxc` and `darnell-blazin.hxc`.

use crate::character::BaseCharacter;

/// All `weekend-1-*` combat note kinds used by `Blazin'` (`pico-blazin.hxc` / `darnell-blazin.hxc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlazinNoteKind {
    PunchLow,
    PunchLowBlocked,
    PunchLowDodged,
    PunchLowSpin,
    PunchHigh,
    PunchHighBlocked,
    PunchHighDodged,
    PunchHighSpin,
    BlockHigh,
    BlockLow,
    BlockSpin,
    DodgeHigh,
    DodgeLow,
    DodgeSpin,
    HitHigh,
    HitLow,
    HitSpin,
    PicoUppercutPrep,
    PicoUppercut,
    DarnellUppercutPrep,
    DarnellUppercut,
    Idle,
    Fakeout,
    Taunt,
    TauntForce,
    ReverseFakeout,
}

#[must_use]
pub(super) fn parse_blazin_note_kind(k: &str) -> Option<BlazinNoteKind> {
    if k.eq_ignore_ascii_case("weekend-1-punchlow") {
        Some(BlazinNoteKind::PunchLow)
    } else if k.eq_ignore_ascii_case("weekend-1-punchlowblocked") {
        Some(BlazinNoteKind::PunchLowBlocked)
    } else if k.eq_ignore_ascii_case("weekend-1-punchlowdodged") {
        Some(BlazinNoteKind::PunchLowDodged)
    } else if k.eq_ignore_ascii_case("weekend-1-punchlowspin") {
        Some(BlazinNoteKind::PunchLowSpin)
    } else if k.eq_ignore_ascii_case("weekend-1-punchhigh") {
        Some(BlazinNoteKind::PunchHigh)
    } else if k.eq_ignore_ascii_case("weekend-1-punchhighblocked") {
        Some(BlazinNoteKind::PunchHighBlocked)
    } else if k.eq_ignore_ascii_case("weekend-1-punchhighdodged") {
        Some(BlazinNoteKind::PunchHighDodged)
    } else if k.eq_ignore_ascii_case("weekend-1-punchhighspin") {
        Some(BlazinNoteKind::PunchHighSpin)
    } else if k.eq_ignore_ascii_case("weekend-1-blockhigh") {
        Some(BlazinNoteKind::BlockHigh)
    } else if k.eq_ignore_ascii_case("weekend-1-blocklow") {
        Some(BlazinNoteKind::BlockLow)
    } else if k.eq_ignore_ascii_case("weekend-1-blockspin") {
        Some(BlazinNoteKind::BlockSpin)
    } else if k.eq_ignore_ascii_case("weekend-1-dodgehigh") {
        Some(BlazinNoteKind::DodgeHigh)
    } else if k.eq_ignore_ascii_case("weekend-1-dodgelow") {
        Some(BlazinNoteKind::DodgeLow)
    } else if k.eq_ignore_ascii_case("weekend-1-dodgespin") {
        Some(BlazinNoteKind::DodgeSpin)
    } else if k.eq_ignore_ascii_case("weekend-1-hithigh") {
        Some(BlazinNoteKind::HitHigh)
    } else if k.eq_ignore_ascii_case("weekend-1-hitlow") {
        Some(BlazinNoteKind::HitLow)
    } else if k.eq_ignore_ascii_case("weekend-1-hitspin") {
        Some(BlazinNoteKind::HitSpin)
    } else if k.eq_ignore_ascii_case("weekend-1-picouppercutprep") {
        Some(BlazinNoteKind::PicoUppercutPrep)
    } else if k.eq_ignore_ascii_case("weekend-1-picouppercut") {
        Some(BlazinNoteKind::PicoUppercut)
    } else if k.eq_ignore_ascii_case("weekend-1-darnelluppercutprep") {
        Some(BlazinNoteKind::DarnellUppercutPrep)
    } else if k.eq_ignore_ascii_case("weekend-1-darnelluppercut") {
        Some(BlazinNoteKind::DarnellUppercut)
    } else if k.eq_ignore_ascii_case("weekend-1-idle") {
        Some(BlazinNoteKind::Idle)
    } else if k.eq_ignore_ascii_case("weekend-1-fakeout") {
        Some(BlazinNoteKind::Fakeout)
    } else if k.eq_ignore_ascii_case("weekend-1-taunt") {
        Some(BlazinNoteKind::Taunt)
    } else if k.eq_ignore_ascii_case("weekend-1-tauntforce") {
        Some(BlazinNoteKind::TauntForce)
    } else if k.eq_ignore_ascii_case("weekend-1-reversefakeout") {
        Some(BlazinNoteKind::ReverseFakeout)
    } else {
        None
    }
}

#[derive(Debug, Default)]
pub(super) struct BlazinFighterState {
    pub pico_cant_uppercut: bool,
    pub darnell_cant_uppercut: bool,
    pub pico_alt: bool,
    pub darnell_alt: bool,
}

impl BlazinFighterState {
    pub(super) fn reset(&mut self) {
        self.pico_cant_uppercut = false;
        self.darnell_cant_uppercut = false;
        self.pico_alt = false;
        self.darnell_alt = false;
    }
}

#[derive(Debug, Clone, Copy)]
enum MovePose {
    Block,
    Cringe,
    Dodge,
    Idle,
    Fakeout,
    Taunt,
    Pissed,
    UppercutPrep,
    Uppercut { shake: bool },
    UppercutHit,
    HitHigh,
    HitLow,
    HitSpin,
    PunchHigh,
    PunchLow,
}

fn apply_fighter_pose(
    ch: &mut BaseCharacter,
    alt_flag: &mut bool,
    pose: MovePose,
    shake_out: &mut Option<[f32; 2]>,
) {
    ch.hold_timer = 0.0;
    let mut next_punch = |base: &str| -> &'static str {
        *alt_flag = !*alt_flag;
        match (base, *alt_flag) {
            ("punchHigh", true) => "punchHigh1",
            ("punchHigh", false) => "punchHigh2",
            (_, true) => "punchLow1",
            (_, false) => "punchLow2",
        }
    };
    let (anim, restart, front, shake): (&str, bool, bool, Option<[f32; 2]>) = match pose {
        MovePose::Block => ("block", true, false, Some([0.002, 0.1])),
        MovePose::Cringe => ("cringe", true, false, None),
        MovePose::Dodge => ("dodge", true, false, None),
        MovePose::Idle => ("idle", false, false, None),
        MovePose::Fakeout => ("fakeout", true, false, None),
        MovePose::Taunt => ("taunt", true, false, None),
        MovePose::Pissed => ("pissed", true, false, None),
        MovePose::UppercutPrep => ("uppercutPrep", true, true, None),
        MovePose::Uppercut { shake } => (
            "uppercut",
            true,
            true,
            if shake { Some([0.005, 0.25]) } else { None },
        ),
        MovePose::UppercutHit => ("uppercutHit", true, false, Some([0.005, 0.25])),
        MovePose::HitHigh => ("hitHigh", true, false, Some([0.0025, 0.15])),
        MovePose::HitLow => ("hitLow", true, false, Some([0.0025, 0.15])),
        MovePose::HitSpin => ("hitSpin", true, false, Some([0.0025, 0.15])),
        MovePose::PunchHigh => (next_punch("punchHigh"), true, true, None),
        MovePose::PunchLow => (next_punch("punchLow"), true, true, None),
    };
    ch.play_animation(anim, restart, false);
    ch.z_index = if front { 3000 } else { 2000 };
    if let Some(s) = shake {
        *shake_out = Some(s);
    }
}

pub(super) fn on_blazin_note_hit(
    state: &mut BlazinFighterState,
    kind: BlazinNoteKind,
    is_poor_hit: bool,
    is_low_health: bool,
    darnell_counter_uppercut: bool,
    bf: Option<&mut BaseCharacter>,
    mut dad: Option<&mut BaseCharacter>,
) -> Option<[f32; 2]> {
    let mut shake = None;

    // Dispatch opponent (`darnell-blazin`) FIRST, matching `pico-blazin.hxc` comment.
    if let Some(ref mut d) = dad {
        if is_poor_hit && is_low_health && darnell_counter_uppercut {
            apply_fighter_pose(d, &mut state.darnell_alt, MovePose::UppercutPrep, &mut shake);
        } else if state.darnell_cant_uppercut {
            apply_fighter_pose(d, &mut state.darnell_alt, MovePose::PunchHigh, &mut shake);
            state.darnell_cant_uppercut = false;
        } else {
            let pose = match kind {
                BlazinNoteKind::PunchLow => Some(MovePose::HitLow),
                BlazinNoteKind::PunchLowBlocked | BlazinNoteKind::PunchHighBlocked => {
                    Some(MovePose::Block)
                }
                BlazinNoteKind::PunchLowDodged | BlazinNoteKind::PunchHighDodged => {
                    Some(MovePose::Dodge)
                }
                BlazinNoteKind::PunchLowSpin | BlazinNoteKind::PunchHighSpin => {
                    Some(MovePose::HitSpin)
                }
                BlazinNoteKind::PunchHigh => Some(MovePose::HitHigh),
                BlazinNoteKind::BlockHigh
                | BlazinNoteKind::BlockSpin
                | BlazinNoteKind::DodgeHigh
                | BlazinNoteKind::DodgeSpin
                | BlazinNoteKind::HitHigh
                | BlazinNoteKind::HitSpin => Some(MovePose::PunchHigh),
                BlazinNoteKind::BlockLow | BlazinNoteKind::DodgeLow | BlazinNoteKind::HitLow => {
                    Some(MovePose::PunchLow)
                }
                BlazinNoteKind::PicoUppercutPrep => {
                    state.darnell_cant_uppercut = true;
                    Some(MovePose::HitHigh)
                }
                BlazinNoteKind::PicoUppercut => Some(MovePose::Dodge),
                BlazinNoteKind::DarnellUppercutPrep => Some(MovePose::UppercutPrep),
                BlazinNoteKind::DarnellUppercut => Some(MovePose::Uppercut { shake: false }),
                BlazinNoteKind::Idle => Some(MovePose::Idle),
                BlazinNoteKind::Fakeout => Some(MovePose::Cringe),
                BlazinNoteKind::Taunt => {
                    if d.current_animation() == "cringe" {
                        Some(MovePose::Pissed)
                    } else {
                        Some(MovePose::Idle)
                    }
                }
                BlazinNoteKind::TauntForce => Some(MovePose::Pissed),
                BlazinNoteKind::ReverseFakeout => Some(MovePose::Fakeout),
            };
            if let Some(p) = pose {
                apply_fighter_pose(d, &mut state.darnell_alt, p, &mut shake);
            }
            if !matches!(kind, BlazinNoteKind::PicoUppercutPrep) {
                state.darnell_cant_uppercut = false;
            }
        }
    }

    let darnell_prepping = is_poor_hit
        && is_low_health
        && dad.as_ref().is_some_and(|d| d.current_animation() == "uppercutPrep");

    if let Some(b) = bf {
        if darnell_prepping {
            apply_fighter_pose(b, &mut state.pico_alt, MovePose::PunchHigh, &mut shake);
            return shake;
        }
        if state.pico_cant_uppercut {
            apply_fighter_pose(b, &mut state.pico_alt, MovePose::Block, &mut shake);
            state.pico_cant_uppercut = false;
            return shake;
        }
        let pose = match kind {
            BlazinNoteKind::PunchLow
            | BlazinNoteKind::PunchLowBlocked
            | BlazinNoteKind::PunchLowDodged
            | BlazinNoteKind::PunchLowSpin => MovePose::PunchLow,
            BlazinNoteKind::PunchHigh
            | BlazinNoteKind::PunchHighBlocked
            | BlazinNoteKind::PunchHighDodged
            | BlazinNoteKind::PunchHighSpin => MovePose::PunchHigh,
            BlazinNoteKind::BlockHigh | BlazinNoteKind::BlockLow | BlazinNoteKind::BlockSpin => {
                MovePose::Block
            }
            BlazinNoteKind::DodgeHigh | BlazinNoteKind::DodgeLow | BlazinNoteKind::DodgeSpin => {
                MovePose::Dodge
            }
            BlazinNoteKind::HitHigh => MovePose::HitHigh,
            BlazinNoteKind::HitLow => MovePose::HitLow,
            BlazinNoteKind::HitSpin => MovePose::HitSpin,
            BlazinNoteKind::PicoUppercutPrep => MovePose::UppercutPrep,
            BlazinNoteKind::PicoUppercut => MovePose::Uppercut { shake: true },
            BlazinNoteKind::DarnellUppercutPrep => MovePose::Idle,
            BlazinNoteKind::DarnellUppercut => MovePose::UppercutHit,
            BlazinNoteKind::Idle | BlazinNoteKind::ReverseFakeout => MovePose::Idle,
            BlazinNoteKind::Fakeout => MovePose::Fakeout,
            BlazinNoteKind::Taunt => {
                if b.current_animation() == "fakeout" {
                    MovePose::Taunt
                } else {
                    MovePose::Idle
                }
            }
            BlazinNoteKind::TauntForce => MovePose::Taunt,
        };
        apply_fighter_pose(b, &mut state.pico_alt, pose, &mut shake);
    }

    shake
}

pub(super) fn on_blazin_note_miss(
    state: &mut BlazinFighterState,
    kind: BlazinNoteKind,
    is_lethal: bool,
    bf: Option<&mut BaseCharacter>,
    dad: Option<&mut BaseCharacter>,
) -> Option<[f32; 2]> {
    let mut shake = None;

    let darnell_in_uppercut = dad.as_ref().is_some_and(|d| {
        let cur = d.current_animation();
        cur == "uppercut" || cur == "uppercut-hold"
    });
    let darnell_was_prep = dad
        .as_ref()
        .is_some_and(|d| d.current_animation() == "uppercutPrep");

    if let Some(d) = dad {
        if darnell_was_prep {
            apply_fighter_pose(d, &mut state.darnell_alt, MovePose::Uppercut { shake: false }, &mut shake);
        } else if is_lethal {
            apply_fighter_pose(d, &mut state.darnell_alt, MovePose::PunchLow, &mut shake);
        } else if state.darnell_cant_uppercut {
            apply_fighter_pose(d, &mut state.darnell_alt, MovePose::PunchHigh, &mut shake);
            state.darnell_cant_uppercut = false;
        } else {
            let pose = match kind {
                BlazinNoteKind::PunchLow
                | BlazinNoteKind::PunchLowBlocked
                | BlazinNoteKind::PunchLowDodged
                | BlazinNoteKind::PunchLowSpin
                | BlazinNoteKind::BlockLow
                | BlazinNoteKind::DodgeLow
                | BlazinNoteKind::HitLow => MovePose::PunchLow,
                BlazinNoteKind::PunchHigh
                | BlazinNoteKind::PunchHighBlocked
                | BlazinNoteKind::PunchHighDodged
                | BlazinNoteKind::PunchHighSpin
                | BlazinNoteKind::BlockHigh
                | BlazinNoteKind::BlockSpin
                | BlazinNoteKind::DodgeHigh
                | BlazinNoteKind::DodgeSpin
                | BlazinNoteKind::HitHigh
                | BlazinNoteKind::HitSpin
                | BlazinNoteKind::Fakeout => MovePose::PunchHigh,
                BlazinNoteKind::PicoUppercutPrep => {
                    state.darnell_cant_uppercut = false;
                    MovePose::Block
                }
                BlazinNoteKind::PicoUppercut => MovePose::UppercutHit,
                BlazinNoteKind::DarnellUppercutPrep => MovePose::UppercutPrep,
                BlazinNoteKind::DarnellUppercut => MovePose::Uppercut { shake: false },
                BlazinNoteKind::Idle => MovePose::Idle,
                BlazinNoteKind::Taunt => {
                    if d.current_animation() == "cringe" {
                        MovePose::Pissed
                    } else {
                        MovePose::Idle
                    }
                }
                BlazinNoteKind::TauntForce => MovePose::Pissed,
                BlazinNoteKind::ReverseFakeout => MovePose::Fakeout,
            };
            apply_fighter_pose(d, &mut state.darnell_alt, pose, &mut shake);
        }
    }

    if let Some(b) = bf {
        if darnell_in_uppercut {
            apply_fighter_pose(b, &mut state.pico_alt, MovePose::UppercutHit, &mut shake);
            return shake;
        }
        if is_lethal {
            apply_fighter_pose(b, &mut state.pico_alt, MovePose::HitLow, &mut shake);
            return shake;
        }
        if state.pico_cant_uppercut {
            apply_fighter_pose(b, &mut state.pico_alt, MovePose::HitHigh, &mut shake);
            state.pico_cant_uppercut = false;
            return shake;
        }
        let pose = match kind {
            BlazinNoteKind::PunchLow
            | BlazinNoteKind::PunchLowBlocked
            | BlazinNoteKind::PunchLowDodged
            | BlazinNoteKind::BlockLow
            | BlazinNoteKind::DodgeLow
            | BlazinNoteKind::HitLow => MovePose::HitLow,
            BlazinNoteKind::PunchLowSpin
            | BlazinNoteKind::PunchHighSpin
            | BlazinNoteKind::BlockSpin
            | BlazinNoteKind::DodgeSpin
            | BlazinNoteKind::HitSpin => MovePose::HitSpin,
            BlazinNoteKind::PunchHigh
            | BlazinNoteKind::PunchHighBlocked
            | BlazinNoteKind::PunchHighDodged
            | BlazinNoteKind::BlockHigh
            | BlazinNoteKind::DodgeHigh
            | BlazinNoteKind::HitHigh
            | BlazinNoteKind::Fakeout => MovePose::HitHigh,
            BlazinNoteKind::PicoUppercutPrep => {
                state.pico_cant_uppercut = true;
                MovePose::PunchHigh
            }
            BlazinNoteKind::PicoUppercut => MovePose::Uppercut { shake: false },
            BlazinNoteKind::DarnellUppercutPrep => MovePose::Idle,
            BlazinNoteKind::DarnellUppercut => MovePose::UppercutHit,
            BlazinNoteKind::Idle | BlazinNoteKind::ReverseFakeout => MovePose::Idle,
            BlazinNoteKind::Taunt => {
                if b.current_animation() == "fakeout" {
                    MovePose::Taunt
                } else {
                    MovePose::Idle
                }
            }
            BlazinNoteKind::TauntForce => MovePose::Taunt,
        };
        apply_fighter_pose(b, &mut state.pico_alt, pose, &mut shake);
    }

    shake
}

pub(super) fn on_blazin_ghost_miss(
    state: &mut BlazinFighterState,
    is_lethal: bool,
    should_dodge: bool,
    bf: Option<&mut BaseCharacter>,
    dad: Option<&mut BaseCharacter>,
) -> Option<[f32; 2]> {
    let mut shake = None;
    if let Some(d) = dad {
        let pose = if is_lethal {
            MovePose::PunchLow
        } else if should_dodge {
            MovePose::Dodge
        } else {
            MovePose::Block
        };
        apply_fighter_pose(d, &mut state.darnell_alt, pose, &mut shake);
    }
    if let Some(b) = bf {
        let pose = if is_lethal {
            MovePose::HitLow
        } else {
            MovePose::PunchHigh
        };
        apply_fighter_pose(b, &mut state.pico_alt, pose, &mut shake);
    }
    shake
}
