//! Zero-allocation precise input ring buffer, keybindings, and held-key tracking
//! synchronized against [`std::time::Instant`].

use std::fmt;
use std::time::Instant;

use fnfrs_core::constants::KEY_COUNT;
use winit::keyboard::KeyCode;

/// Maximum number of raw input events buffered between frames without heap allocation.
pub const INPUT_QUEUE_CAPACITY: usize = 128;

/// Maximum concurrent bound keys held simultaneously on one lane.
pub const MAX_HELD_KEYS_PER_LANE: usize = 4;

/// Cardinal 4-key lane direction matching `Strumline.DIRECTIONS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum NoteDirection {
    #[default]
    Left = 0,
    Down = 1,
    Up = 2,
    Right = 3,
}

impl NoteDirection {
    /// Canonical lane order (`[Left, Down, Up, Right]`).
    pub const ALL: [Self; KEY_COUNT] = [Self::Left, Self::Down, Self::Up, Self::Right];

    /// Converts a zero-based lane index (`0..4`) into [`NoteDirection`].
    #[inline]
    #[must_use]
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Left),
            1 => Some(Self::Down),
            2 => Some(Self::Up),
            3 => Some(Self::Right),
            _ => None,
        }
    }

    /// Zero-based lane index (`0..4`).
    #[inline]
    #[must_use]
    pub const fn as_index(self) -> usize {
        self as usize
    }

    /// Color identifier used in V-Slice Sparrow prefixes (`purple`, `blue`, `green`, `red`).
    #[inline]
    #[must_use]
    pub const fn color_name(self) -> &'static str {
        match self {
            Self::Left => "purple",
            Self::Down => "blue",
            Self::Up => "green",
            Self::Right => "red",
        }
    }

    /// Title-cased color identifier used in `NoteHoldCover` animations.
    #[inline]
    #[must_use]
    pub const fn title_color_name(self) -> &'static str {
        match self {
            Self::Left => "Purple",
            Self::Down => "Blue",
            Self::Up => "Green",
            Self::Right => "Red",
        }
    }

    /// Note scroll animation name matching `NoteSprite.DIRECTION_COLORS`.
    #[inline]
    #[must_use]
    pub const fn scroll_anim_name(self) -> &'static str {
        match self {
            Self::Left => "purpleScroll",
            Self::Down => "blueScroll",
            Self::Up => "greenScroll",
            Self::Right => "redScroll",
        }
    }
}

impl fmt::Display for NoteDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Left => "left",
            Self::Down => "down",
            Self::Up => "up",
            Self::Right => "right",
        })
    }
}

/// Timestamped key press or release event captured from `winit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreciseInputEvent {
    pub direction: NoteDirection,
    pub key_code: KeyCode,
    pub pressed: bool,
    pub timestamp: Instant,
}

/// Primary + secondary key bindings per lane (`Left`, `Down`, `Up`, `Right`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinds {
    pub lanes: [[KeyCode; 2]; KEY_COUNT],
}

impl Default for KeyBinds {
    fn default() -> Self {
        Self {
            lanes: [
                [KeyCode::ArrowLeft, KeyCode::KeyA],
                [KeyCode::ArrowDown, KeyCode::KeyS],
                [KeyCode::ArrowUp, KeyCode::KeyK],
                [KeyCode::ArrowRight, KeyCode::KeyL],
            ],
        }
    }
}

impl KeyBinds {
    /// Parses a saved keybind name (e.g. `"A"`, `"S"`, `"W"`, `"D"`, `"J"`) into [`KeyCode`].
    #[must_use]
    pub fn key_from_name(name: &str) -> Option<KeyCode> {
        match name.trim().to_ascii_uppercase().as_str() {
            "A" => Some(KeyCode::KeyA),
            "B" => Some(KeyCode::KeyB),
            "C" => Some(KeyCode::KeyC),
            "D" => Some(KeyCode::KeyD),
            "E" => Some(KeyCode::KeyE),
            "F" => Some(KeyCode::KeyF),
            "G" => Some(KeyCode::KeyG),
            "H" => Some(KeyCode::KeyH),
            "I" => Some(KeyCode::KeyI),
            "J" => Some(KeyCode::KeyJ),
            "K" => Some(KeyCode::KeyK),
            "L" => Some(KeyCode::KeyL),
            "M" => Some(KeyCode::KeyM),
            "N" => Some(KeyCode::KeyN),
            "O" => Some(KeyCode::KeyO),
            "P" => Some(KeyCode::KeyP),
            "Q" => Some(KeyCode::KeyQ),
            "R" => Some(KeyCode::KeyR),
            "S" => Some(KeyCode::KeyS),
            "T" => Some(KeyCode::KeyT),
            "U" => Some(KeyCode::KeyU),
            "V" => Some(KeyCode::KeyV),
            "W" => Some(KeyCode::KeyW),
            "X" => Some(KeyCode::KeyX),
            "Y" => Some(KeyCode::KeyY),
            "Z" => Some(KeyCode::KeyZ),
            "LEFT" => Some(KeyCode::ArrowLeft),
            "DOWN" => Some(KeyCode::ArrowDown),
            "UP" => Some(KeyCode::ArrowUp),
            "RIGHT" => Some(KeyCode::ArrowRight),
            "SPACE" => Some(KeyCode::Space),
            _ => None,
        }
    }

    /// Converts a [`KeyCode`] to its portable save string.
    #[must_use]
    pub const fn key_to_name(code: KeyCode) -> Option<&'static str> {
        match code {
            KeyCode::KeyA => Some("A"),
            KeyCode::KeyB => Some("B"),
            KeyCode::KeyC => Some("C"),
            KeyCode::KeyD => Some("D"),
            KeyCode::KeyE => Some("E"),
            KeyCode::KeyF => Some("F"),
            KeyCode::KeyG => Some("G"),
            KeyCode::KeyH => Some("H"),
            KeyCode::KeyI => Some("I"),
            KeyCode::KeyJ => Some("J"),
            KeyCode::KeyK => Some("K"),
            KeyCode::KeyL => Some("L"),
            KeyCode::KeyM => Some("M"),
            KeyCode::KeyN => Some("N"),
            KeyCode::KeyO => Some("O"),
            KeyCode::KeyP => Some("P"),
            KeyCode::KeyQ => Some("Q"),
            KeyCode::KeyR => Some("R"),
            KeyCode::KeyS => Some("S"),
            KeyCode::KeyT => Some("T"),
            KeyCode::KeyU => Some("U"),
            KeyCode::KeyV => Some("V"),
            KeyCode::KeyW => Some("W"),
            KeyCode::KeyX => Some("X"),
            KeyCode::KeyY => Some("Y"),
            KeyCode::KeyZ => Some("Z"),
            KeyCode::ArrowLeft => Some("LEFT"),
            KeyCode::ArrowDown => Some("DOWN"),
            KeyCode::ArrowUp => Some("UP"),
            KeyCode::ArrowRight => Some("RIGHT"),
            KeyCode::Space => Some("SPACE"),
            KeyCode::Escape => Some("ESCAPE"),
            _ => None,
        }
    }

    /// Builds [`KeyBinds`] keeping Arrow keys as primary + `names` as secondary bindings.
    #[must_use]
    pub fn from_names(names: &[String; 4]) -> Self {
        let def = Self::default();
        let mut lanes = def.lanes;
        for i in 0..4 {
            if let Some(k) = Self::key_from_name(&names[i]) {
                lanes[i][1] = k;
            }
        }
        Self { lanes }
    }

    /// Resolves a `winit::keyboard::KeyCode` to a gameplay lane, if bound.
    #[inline]
    #[must_use]
    pub fn resolve(&self, key_code: KeyCode) -> Option<NoteDirection> {
        for (lane_idx, pair) in self.lanes.iter().enumerate() {
            if pair[0] == key_code || pair[1] == key_code {
                return NoteDirection::from_index(lane_idx);
            }
        }
        None
    }
}

/// Fixed-capacity ring buffer of [`PreciseInputEvent`]s with zero heap allocations.
#[derive(Debug)]
pub struct InputRing<const CAP: usize = INPUT_QUEUE_CAPACITY> {
    slots: [Option<PreciseInputEvent>; CAP],
    head: usize,
    len: usize,
}

impl<const CAP: usize> Default for InputRing<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> InputRing<CAP> {
    /// Creates an empty fixed-capacity input ring buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [None; CAP],
            head: 0,
            len: 0,
        }
    }

    /// Number of queued events.
    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when empty.
    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Pushes an event at the back. If full, overwrites the oldest event and returns `false`.
    #[inline]
    pub fn push(&mut self, event: PreciseInputEvent) -> bool {
        if CAP == 0 {
            return false;
        }
        if self.len == CAP {
            self.slots[self.head] = Some(event);
            self.head = (self.head + 1) % CAP;
            return false;
        }
        let tail = (self.head + self.len) % CAP;
        self.slots[tail] = Some(event);
        self.len += 1;
        true
    }

    /// Pops the oldest queued event in FIFO order.
    #[inline]
    pub fn pop_front(&mut self) -> Option<PreciseInputEvent> {
        if self.len == 0 {
            return None;
        }
        let item = self.slots[self.head].take();
        self.head = (self.head + 1) % CAP;
        self.len -= 1;
        item
    }

    /// Discards all buffered events without deallocating.
    #[inline]
    pub fn clear(&mut self) {
        while self.pop_front().is_some() {}
    }
}

/// Dual-queue input collector separated into press and release rings matching `PlayState`.
#[derive(Debug)]
pub struct InputQueue {
    pub binds: KeyBinds,
    pub presses: InputRing<INPUT_QUEUE_CAPACITY>,
    pub releases: InputRing<INPUT_QUEUE_CAPACITY>,
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new(KeyBinds::default())
    }
}

impl InputQueue {
    /// Creates a new [`InputQueue`] with the provided key bindings.
    #[must_use]
    pub const fn new(binds: KeyBinds) -> Self {
        Self {
            binds,
            presses: InputRing::new(),
            releases: InputRing::new(),
        }
    }

    /// Enqueues a raw `winit` key event if bound to a gameplay lane (ignoring key repeats).
    #[inline]
    pub fn record_key(
        &mut self,
        key_code: KeyCode,
        pressed: bool,
        repeat: bool,
        timestamp: Instant,
    ) -> Option<NoteDirection> {
        if pressed && repeat {
            return None;
        }
        let direction = self.binds.resolve(key_code)?;
        let event = PreciseInputEvent {
            direction,
            key_code,
            pressed,
            timestamp,
        };
        if pressed {
            self.presses.push(event);
        } else {
            self.releases.push(event);
        }
        Some(direction)
    }

    /// Clears both press and release queues.
    #[inline]
    pub fn clear(&mut self) {
        self.presses.clear();
        self.releases.clear();
    }

    /// Returns total buffered press + release events.
    #[inline]
    #[must_use]
    pub const fn total_len(&self) -> usize {
        self.presses.len() + self.releases.len()
    }
}

/// Zero-allocation held-key state across the 4 strumline lanes.
///
/// Matches `Strumline.heldKeys`: a direction remains held while at least one
/// physical key bound to that lane is pressed.
#[derive(Debug, Default, Clone)]
pub struct HeldKeys {
    lanes: [[Option<KeyCode>; MAX_HELD_KEYS_PER_LANE]; KEY_COUNT],
}

impl HeldKeys {
    /// Creates an empty [`HeldKeys`] tracker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lanes: [[None; MAX_HELD_KEYS_PER_LANE]; KEY_COUNT],
        }
    }

    /// Marks `key_code` as held for `dir`.
    pub fn press(&mut self, dir: NoteDirection, key_code: KeyCode) {
        let slots = &mut self.lanes[dir.as_index()];
        if slots.contains(&Some(key_code)) {
            return;
        }
        if let Some(empty) = slots.iter_mut().find(|k| k.is_none()) {
            *empty = Some(key_code);
        }
    }

    /// Releases `key_code` for `dir` (or all keys for `dir` if `key_code` is `None`).
    pub fn release(&mut self, dir: NoteDirection, key_code: Option<KeyCode>) {
        let slots = &mut self.lanes[dir.as_index()];
        match key_code {
            None => *slots = [None; MAX_HELD_KEYS_PER_LANE],
            Some(code) => {
                for slot in slots.iter_mut() {
                    if *slot == Some(code) {
                        *slot = None;
                    }
                }
            }
        }
    }

    /// Returns `true` if at least one key is currently held for `dir`.
    #[inline]
    #[must_use]
    pub fn is_held(&self, dir: NoteDirection) -> bool {
        self.lanes[dir.as_index()].iter().any(Option::is_some)
    }

    /// Clears all held keys across every direction.
    pub fn clear(&mut self) {
        self.lanes = [[None; MAX_HELD_KEYS_PER_LANE]; KEY_COUNT];
    }
}

/// Computes `noteDiff` in milliseconds compensating for sub-frame input latency
/// (`PlayState.goodNoteHit`: `songPosition - note.time - inputLatencyMs`).
///
/// Negative means early; positive means late.
#[inline]
#[must_use]
pub fn compute_note_diff_ms(
    song_position_ms: f64,
    note_time_ms: f64,
    event_timestamp: Instant,
    frame_timestamp: Instant,
) -> f64 {
    let latency_ms = frame_timestamp
        .saturating_duration_since(event_timestamp)
        .as_secs_f64()
        * 1000.0;
    song_position_ms - note_time_ms - latency_ms
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn keybinds_and_ring_fifo_order() {
        let mut queue = InputQueue::default();
        let t0 = Instant::now();

        assert_eq!(
            queue.record_key(KeyCode::KeyA, true, false, t0),
            Some(NoteDirection::Left)
        );
        // Key repeats are ignored.
        assert_eq!(queue.record_key(KeyCode::KeyA, true, true, t0), None);
        assert_eq!(
            queue.record_key(KeyCode::ArrowUp, true, false, t0),
            Some(NoteDirection::Up)
        );
        assert_eq!(queue.presses.len(), 2);

        let first = queue.presses.pop_front().expect("first");
        assert_eq!(first.direction, NoteDirection::Left);
        let second = queue.presses.pop_front().expect("second");
        assert_eq!(second.direction, NoteDirection::Up);
        assert!(queue.presses.is_empty());
    }

    #[test]
    fn held_keys_handles_dual_bindings() {
        let mut held = HeldKeys::new();
        held.press(NoteDirection::Left, KeyCode::KeyA);
        held.press(NoteDirection::Left, KeyCode::ArrowLeft);
        assert!(held.is_held(NoteDirection::Left));

        // Releasing one bound key keeps the direction held.
        held.release(NoteDirection::Left, Some(KeyCode::KeyA));
        assert!(held.is_held(NoteDirection::Left));

        held.release(NoteDirection::Left, Some(KeyCode::ArrowLeft));
        assert!(!held.is_held(NoteDirection::Left));
    }

    #[test]
    fn sub_frame_latency_compensation() {
        let now = Instant::now();
        let pressed_at = now - Duration::from_millis(4);
        let diff = compute_note_diff_ms(1004.0, 1000.0, pressed_at, now);
        assert!(diff.abs() < 1e-6);
    }
}
