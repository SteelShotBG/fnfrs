//! Dual-sprite normal/dark character lighting overlays for Week 2 `spookyMansionErect`
//! (`bf-dark.hxc`, `gf-dark.hxc`, `nene-dark.hxc`, `pico-dark.hxc`, `spooky-dark.hxc`).

use fnfrs_core::paths::AssetLocator;
use fnfrs_render::{GpuContext, SpriteBatcher};

use crate::character::{BaseCharacter, CharacterKind};

#[derive(Debug)]
struct DarkOverlay {
    char_inst: BaseCharacter,
    pos_offset: [f32; 2],
    active: bool,
}

impl DarkOverlay {
    fn sync_from(&mut self, dark: &BaseCharacter, elapsed_s: f32, beat_length_ms: f64) {
        if !self.active {
            return;
        }
        self.char_inst.position = [
            dark.position[0] + self.pos_offset[0],
            dark.position[1] + self.pos_offset[1],
        ];
        self.char_inst.scale = dark.scale;
        self.char_inst.scroll_factor = dark.scroll_factor;
        self.char_inst.flip_x = dark.flip_x;
        self.char_inst.visible = dark.visible;
        // Match `*-dark.hxc` set_alpha: normalChar sits 1 z-layer behind darkChar and
        // turns fully opaque whenever darkChar.alpha != 1.0 during lightning flashes/fades.
        self.char_inst.alpha = if (dark.alpha - 1.0).abs() > 1e-4 {
            1.0
        } else {
            0.0
        };
        let target_anim = dark.current_animation();
        if !target_anim.is_empty() && self.char_inst.current_animation() != target_anim {
            self.char_inst.play_animation(target_anim, true, false);
        } else if dark.active_frame_snapshot().is_some_and(|(_, f, _)| f == 0)
            && self
                .char_inst
                .active_frame_snapshot()
                .is_some_and(|(_, f, _)| f > 0)
        {
            self.char_inst.play_animation(target_anim, true, false);
        }
        self.char_inst.update(elapsed_s, beat_length_ms, false);
    }
}

/// Preloaded normal (lit) character sprites rendered behind `*-dark` characters during
/// `spookyMansionErect` lightning strikes.
#[derive(Debug, Default)]
pub(super) struct SpookyDarkOverlays {
    bf: Option<DarkOverlay>,
    dad: Option<DarkOverlay>,
    gf: Option<DarkOverlay>,
}

impl SpookyDarkOverlays {
    pub(super) fn load(
        stage_lower: &str,
        dad_id: &str,
        gf_id: &str,
        assets: &AssetLocator,
        gpu: &mut GpuContext,
    ) -> Self {
        let gf_is_dark = gf_id.eq_ignore_ascii_case("gf-dark") || gf_id.eq_ignore_ascii_case("nene-dark");
        let dad_is_dark = dad_id.eq_ignore_ascii_case("spooky-dark");
        if stage_lower != "spookymansionerect" && !gf_is_dark && !dad_is_dark {
            return Self::default();
        }

        let bf_normal_id = if gf_id.eq_ignore_ascii_case("nene-dark") {
            "pico-playable"
        } else {
            "bf"
        };
        let bf = BaseCharacter::load_by_id(bf_normal_id, CharacterKind::Bf, assets, gpu).map(
            |mut ch| {
                ch.z_index = 199;
                ch.alpha = 0.0;
                ch.dance_every = 0.0;
                let pos_offset = if bf_normal_id == "bf" {
                    [-1.0, -11.0]
                } else {
                    [0.0, 0.0]
                };
                DarkOverlay {
                    char_inst: ch,
                    pos_offset,
                    active: false,
                }
            },
        );

        let dad = if dad_is_dark {
            BaseCharacter::load_by_id("spooky", CharacterKind::Dad, assets, gpu).map(|mut ch| {
                ch.z_index = 199;
                ch.alpha = 0.0;
                ch.dance_every = 0.0;
                DarkOverlay {
                    char_inst: ch,
                    pos_offset: [0.0, 0.0],
                    active: true,
                }
            })
        } else {
            None
        };

        let gf = if gf_id.eq_ignore_ascii_case("gf-dark") {
            BaseCharacter::load_by_id("gf", CharacterKind::Gf, assets, gpu).map(|mut ch| {
                ch.z_index = 99;
                ch.alpha = 0.0;
                ch.dance_every = 0.0;
                DarkOverlay {
                    char_inst: ch,
                    pos_offset: [-45.5, 0.0],
                    active: true,
                }
            })
        } else if gf_id.eq_ignore_ascii_case("nene-dark") {
            BaseCharacter::load_by_id("nene", CharacterKind::Gf, assets, gpu).map(|mut ch| {
                ch.z_index = 97;
                ch.alpha = 0.0;
                ch.dance_every = 0.0;
                DarkOverlay {
                    char_inst: ch,
                    pos_offset: [0.0, 0.0],
                    active: true,
                }
            })
        } else {
            None
        };

        Self { bf, dad, gf }
    }

    pub(super) fn on_stage_ready(
        &mut self,
        bf: Option<&BaseCharacter>,
        dad: Option<&BaseCharacter>,
        gf: Option<&BaseCharacter>,
    ) {
        if let (Some(ov), Some(b)) = (self.bf.as_mut(), bf) {
            ov.active = b.id.eq_ignore_ascii_case("bf-dark") || b.id.eq_ignore_ascii_case("pico-dark");
            if ov.active {
                ov.char_inst.z_index = b.z_index - 1;
                ov.sync_from(b, 0.0, 500.0);
            }
        }
        if let (Some(ov), Some(d)) = (self.dad.as_mut(), dad) {
            ov.active = d.id.eq_ignore_ascii_case("spooky-dark");
            if ov.active {
                ov.char_inst.z_index = d.z_index - 1;
                ov.sync_from(d, 0.0, 500.0);
            }
        }
        if let (Some(ov), Some(g)) = (self.gf.as_mut(), gf) {
            ov.active = g.id.eq_ignore_ascii_case("gf-dark") || g.id.eq_ignore_ascii_case("nene-dark");
            if ov.active {
                let delta = if g.id.eq_ignore_ascii_case("nene-dark") {
                    3
                } else {
                    1
                };
                ov.char_inst.z_index = g.z_index - delta;
                ov.sync_from(g, 0.0, 500.0);
            }
        }
    }

    pub(super) fn update(
        &mut self,
        elapsed_s: f32,
        beat_length_ms: f64,
        bf: Option<&BaseCharacter>,
        dad: Option<&BaseCharacter>,
        gf: Option<&BaseCharacter>,
    ) {
        if let (Some(ov), Some(b)) = (self.bf.as_mut(), bf) {
            ov.sync_from(b, elapsed_s, beat_length_ms);
        }
        if let (Some(ov), Some(d)) = (self.dad.as_mut(), dad) {
            ov.sync_from(d, elapsed_s, beat_length_ms);
        }
        if let (Some(ov), Some(g)) = (self.gf.as_mut(), gf) {
            ov.sync_from(g, elapsed_s, beat_length_ms);
        }
    }

    pub(super) fn draw_range(
        &self,
        batcher: &mut SpriteBatcher,
        z_start: i32,
        z_end: i32,
        cam_pos: [f32; 2],
        cam_zoom: f32,
    ) {
        for ov in [&self.gf, &self.dad, &self.bf].into_iter().flatten() {
            if ov.active
                && ov.char_inst.z_index >= z_start
                && ov.char_inst.z_index < z_end
                && ov.char_inst.alpha > 0.0
            {
                ov.char_inst.draw(batcher, cam_pos, cam_zoom);
            }
        }
    }
}
