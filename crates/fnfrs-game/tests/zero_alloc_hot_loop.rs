//! Zero-allocation hot-loop verification via a custom `#[global_allocator]`.
//! Confirms `PlayScene::update` performs 0 heap allocations and 0 reallocations
//! across 1,000 simulated gameplay frames (including sustains, popups, and bops).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fnfrs_core::schema::character::{AnimationData, CharacterData, CharacterRenderType};
use fnfrs_core::schema::song::SongNoteData;
use fnfrs_game::{
    BaseCharacter, CharacterKind, HealthIcon, InputQueue, PlayConfig, PlayScene, PlaySceneConfig,
    PlaySession, SongEventQueue, StrumlineVisualStyle,
};
use fnfrs_render::{SpriteInstance, TextureHandle, SPRITE_FLAG_ADJUST_COLOR};

struct CountingAllocator;

static COUNTING_ACTIVE: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static REALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL_ALLOC: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING_ACTIVE.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING_ACTIVE.load(Ordering::Relaxed) {
            REALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn make_anim(name: &str) -> AnimationData {
    AnimationData {
        name: name.to_owned(),
        prefix: Some(name.to_owned()),
        asset_path: None,
        offsets: [0.0, 0.0],
        looped: false,
        flip_x: false,
        flip_y: false,
        frame_rate: 24,
        frame_indices: Vec::new(),
        anim_type: "framelabel".to_owned(),
        render_type: String::new(),
        atlas_settings: None,
    }
}

fn make_char(id: &str, kind: CharacterKind) -> BaseCharacter {
    let data = CharacterData {
        version: "1.0.2".to_owned(),
        name: id.to_owned(),
        render_type: CharacterRenderType::Sparrow,
        asset_path: format!("characters/{id}"),
        scale: 1.0,
        health_icon: None,
        death: None,
        offsets: [0.0, 0.0],
        camera_offsets: [0.0, 0.0],
        is_pixel: false,
        dance_every: 1.0,
        sing_time: 4.0,
        animations: vec![
            make_anim("idle"),
            make_anim("singLEFT"),
            make_anim("singDOWN"),
            make_anim("singUP"),
            make_anim("singRIGHT"),
            make_anim("singLEFTmiss"),
        ],
        starting_animation: "idle".to_owned(),
        flip_x: false,
        apply_stage_matrix: false,
        atlas_settings: None,
    };
    BaseCharacter::new_synthetic(id, kind, [200.0, 300.0], data)
}

#[test]
fn hot_loop_and_shader_invariants() {
    // 1. Verify AdjustColor shader flag & parameters cleanly.
    let adjusted = SpriteInstance::new([10.0, 20.0], [100.0, 100.0])
        .with_adjust_color(12.0, -20.0, -23.0, 7.0);
    assert_ne!(adjusted.flags & SPRITE_FLAG_ADJUST_COLOR, 0);
    assert!((adjusted.hsbc[0] - 12.0_f32.to_radians()).abs() < 1e-5);
    assert!((adjusted.hsbc[1] - 0.80).abs() < 1e-5);
    assert!((adjusted.hsbc[2] - (-23.0 / 255.0)).abs() < 1e-5);
    assert!(adjusted.hsbc[3] > 1.0);

    // 2. Build a realistic 1,000-frame PlayScene with botplay + sustains + camera bops.
    let mut player_notes = Vec::with_capacity(64);
    let mut opponent_notes = Vec::with_capacity(64);
    for i in 0..64 {
        let t = 500.0 + (i as f64) * 240.0;
        let d = i % 4;
        player_notes.push(SongNoteData {
            time: t,
            data: d,
            length: if (i & 3) == 0 { 200.0 } else { 0.0 },
            kind: None,
            params: Vec::new(),
        });
        opponent_notes.push(SongNoteData {
            time: t + 120.0,
            data: d + 4,
            length: 0.0,
            kind: None,
            params: Vec::new(),
        });
    }

    let cfg = PlayConfig {
        is_botplay: true,
        scroll_speed: 2.0,
        ..PlayConfig::default()
    };
    let session = PlaySession::from_raw_notes(125.0, &player_notes, &opponent_notes, cfg);
    let mut scene = PlayScene::new(
        session,
        None,
        Some(make_char("bf", CharacterKind::Bf)),
        Some(make_char("dad", CharacterKind::Dad)),
        Some(make_char("gf", CharacterKind::Gf)),
        SongEventQueue::default(),
        HealthIcon::placeholder("bf", true, false),
        HealthIcon::placeholder("dad", false, false),
        None,
        PlaySceneConfig {
            song_name: "AllocGuard".to_owned(),
            difficulty: "hard".to_owned(),
            start_countdown: false,
            song_duration_ms: Some(25_000.0),
            death_count: 0,
        },
    );
    let mut inputs = InputQueue::default();
    let style = StrumlineVisualStyle::fallback(TextureHandle(0));
    let t0 = Instant::now();

    // Warm-up 1 frame outside counting window.
    let _ = scene.update(0.0, 1.0 / 60.0, t0, &mut inputs, None, &style);

    ALLOC_COUNT.store(0, Ordering::SeqCst);
    REALLOC_COUNT.store(0, Ordering::SeqCst);
    COUNTING_ACTIVE.store(true, Ordering::SeqCst);

    for frame in 1..=1000 {
        let elapsed_ms = (frame as f64) * (1000.0 / 60.0);
        let now = t0 + Duration::from_secs_f64(elapsed_ms / 1000.0);
        let _ = scene.update(elapsed_ms, 1.0 / 60.0, now, &mut inputs, None, &style);
    }

    COUNTING_ACTIVE.store(false, Ordering::SeqCst);
    let allocs = ALLOC_COUNT.load(Ordering::SeqCst);
    let reallocs = REALLOC_COUNT.load(Ordering::SeqCst);

    assert_eq!(allocs, 0, "expected 0 heap allocs during 1,000 hot frames, got {allocs}");
    assert_eq!(reallocs, 0, "expected 0 heap reallocs during 1,000 hot frames, got {reallocs}");
}
