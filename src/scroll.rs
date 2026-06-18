//! Pure traversal math for wheel-driven navigation on the tab bar (#80).
//!
//! zellij delivers `Mouse::ScrollUp` / `Mouse::ScrollDown` with only a line
//! count — **no position** — so the wheel acts on the bar as a whole. These
//! helpers turn one wheel step into the next tab index ([`next_tab`], `tab`
//! mode) or the next pane id ([`next_pane`], `pane` mode), wrapping at the ends.
//! They take plain numbers / an id slice and return plain numbers, so they
//! unit-test off-wasm with no zellij types — the same dependency-free discipline
//! the renderer follows.

/// The direction one wheel notch maps to. zellij's stock tab-bar maps
/// `ScrollUp → Forward` (next) and `ScrollDown → Backward` (previous); we follow
/// that direction so the wheel feels native.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollDir {
    Forward,
    Backward,
}

/// How the mouse wheel navigates over the tab bar (config key `scroll`, #80).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollMode {
    /// Scroll switches tabs (forward = next, backward = previous), wrapping —
    /// matching zellij's stock tab-bar direction. The default.
    #[default]
    Tab,
    /// Scroll walks the focused pane forward / backward in reading order,
    /// crossing tab boundaries (the last pane of a tab steps to the first pane of
    /// the next, and back), wrapping globally.
    Pane,
    /// Scroll does nothing.
    Off,
}

impl std::str::FromStr for ScrollMode {
    type Err = ();

    /// `"tab"` / `"pane"` / `"off"` (exact match); any other value errors so the
    /// config parser falls back to the documented default rather than panicking.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "tab" => Ok(Self::Tab),
            "pane" => Ok(Self::Pane),
            "off" => Ok(Self::Off),
            _ => Err(()),
        }
    }
}

/// The 0-based tab index one wheel step from `active` among `count` tabs,
/// wrapping at both ends. `None` when there are no tabs (nothing to switch to).
/// The caller adds 1 for `switch_tab_to`, which is 1-based.
pub fn next_tab(active: usize, count: usize, dir: ScrollDir) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(step(active.min(count - 1), count, dir))
}

/// The pane id one wheel step from `current` in `order` — a flattened traversal
/// of every tab's panes (tabs in position order, panes in reading order) — with
/// a global wrap (last pane of the last tab ↔ first pane of the first tab).
/// `None` when `order` is empty or `current` is not in it (nothing to move
/// from), so the caller leaves focus untouched.
pub fn next_pane(order: &[u32], current: u32, dir: ScrollDir) -> Option<u32> {
    let here = order.iter().position(|&id| id == current)?;
    Some(order[step(here, order.len(), dir)])
}

/// One wrapping step over `0..len`. Both callers guarantee `len > 0` (a guarded
/// `count`, or a `position` match that proves the slice non-empty).
fn step(here: usize, len: usize, dir: ScrollDir) -> usize {
    match dir {
        ScrollDir::Forward => (here + 1) % len,
        ScrollDir::Backward => (here + len - 1) % len,
    }
}

/// What a wheel event does under the leading-edge cooldown rate-limiter (#83).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Navigate now; the limiter is off (`cooldown_ms == 0`), so nothing is armed.
    Navigate,
    /// Navigate now *and* open the cooldown window — the caller arms a
    /// `cooldown_ms` timer and ignores further events until it fires.
    NavigateThenCool,
    /// Drop this event: a cooldown window is already open.
    Ignore,
}

/// Decide a wheel event's fate under the cooldown limiter (#83). zellij emits one
/// `Mouse::ScrollUp` / `ScrollDown` per sub-notch, and a stepless device (Magic
/// Mouse, trackpad) fires a *burst* of them for a single flick — so "one event =
/// one nav step" (#80) races through several tabs/panes. We can't tell the
/// devices apart (the `Mouse` event carries no device identity), so instead of a
/// physical detent we rate-limit by *timing*: the first event navigates at once
/// and opens a `cooldown_ms` window; events arriving inside the window are
/// dropped. A fast burst collapses to one step per window, while a deliberate,
/// well-spaced notch (its window long since elapsed) always navigates
/// immediately — responsive on a detented wheel, damped on a flick.
///
/// `cooling` is whether a cooldown window is currently open; `cooldown_ms == 0`
/// disables the limiter so every event navigates (the pre-#83 one-step-per-event
/// feel). The leading edge is never delayed — unlike a trailing-edge debounce the
/// step lands *on* the event, not after the window closes.
pub fn gate(cooling: bool, cooldown_ms: usize) -> Gate {
    if cooldown_ms == 0 {
        return Gate::Navigate;
    }
    if cooling {
        return Gate::Ignore;
    }
    Gate::NavigateThenCool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scroll_modes() {
        assert_eq!("tab".parse(), Ok(ScrollMode::Tab));
        assert_eq!("pane".parse(), Ok(ScrollMode::Pane));
        assert_eq!("off".parse(), Ok(ScrollMode::Off));
    }

    #[test]
    fn malformed_scroll_mode_errors() {
        // Case-sensitive, exact-match only — the config parser turns the error
        // into the documented default.
        assert_eq!("Tab".parse::<ScrollMode>(), Err(()));
        assert_eq!("wheel".parse::<ScrollMode>(), Err(()));
        assert_eq!("".parse::<ScrollMode>(), Err(()));
    }

    #[test]
    fn next_tab_steps_forward_and_backward() {
        // Three tabs, active in the middle (index 1).
        assert_eq!(next_tab(1, 3, ScrollDir::Forward), Some(2));
        assert_eq!(next_tab(1, 3, ScrollDir::Backward), Some(0));
    }

    #[test]
    fn next_tab_wraps_at_both_ends() {
        // Forward off the last tab wraps to the first; backward off the first
        // wraps to the last (zellij stock clamps here; we wrap by design, #80).
        assert_eq!(next_tab(2, 3, ScrollDir::Forward), Some(0));
        assert_eq!(next_tab(0, 3, ScrollDir::Backward), Some(2));
    }

    #[test]
    fn next_tab_handles_degenerate_counts() {
        // A single tab steps to itself; no tabs yields nothing.
        assert_eq!(next_tab(0, 1, ScrollDir::Forward), Some(0));
        assert_eq!(next_tab(0, 1, ScrollDir::Backward), Some(0));
        assert_eq!(next_tab(0, 0, ScrollDir::Forward), None);
        // An out-of-range active index is clamped into the tab set rather than
        // indexing past the end.
        assert_eq!(next_tab(9, 3, ScrollDir::Forward), Some(0));
    }

    #[test]
    fn next_pane_walks_the_flattened_order() {
        // Ids are arbitrary (not positions): [10, 20, 30] is the traversal.
        let order = [10u32, 20, 30];
        assert_eq!(next_pane(&order, 10, ScrollDir::Forward), Some(20));
        assert_eq!(next_pane(&order, 20, ScrollDir::Forward), Some(30));
        assert_eq!(next_pane(&order, 20, ScrollDir::Backward), Some(10));
    }

    #[test]
    fn next_pane_wraps_globally() {
        // Forward off the last id wraps to the first; backward off the first
        // wraps to the last — the cross-tab hand-off at the very ends (#80).
        let order = [10u32, 20, 30];
        assert_eq!(next_pane(&order, 30, ScrollDir::Forward), Some(10));
        assert_eq!(next_pane(&order, 10, ScrollDir::Backward), Some(30));
    }

    #[test]
    fn next_pane_is_none_when_unanchored() {
        // No panes, or a focused id absent from the order, leaves focus
        // untouched rather than guessing.
        assert_eq!(next_pane(&[], 10, ScrollDir::Forward), None);
        assert_eq!(next_pane(&[10, 20], 99, ScrollDir::Forward), None);
    }

    #[test]
    fn next_pane_wraps_around_a_single_pane() {
        // A lone pane (e.g. a single tab with one pane) steps to itself.
        assert_eq!(next_pane(&[7], 7, ScrollDir::Forward), Some(7));
        assert_eq!(next_pane(&[7], 7, ScrollDir::Backward), Some(7));
    }

    #[test]
    fn gate_navigates_and_opens_the_window_on_the_leading_edge() {
        // First event (not yet cooling): navigate now and open the cooldown
        // window — the immediate, no-latency response a detented wheel wants.
        assert_eq!(gate(false, 40), Gate::NavigateThenCool);
    }

    #[test]
    fn gate_ignores_events_inside_the_window() {
        // While cooling, a flick's burst of follow-up events is dropped so it
        // can't race through tabs.
        assert_eq!(gate(true, 40), Gate::Ignore);
    }

    #[test]
    fn gate_zero_cooldown_disables_the_limiter() {
        // `0` = off: every event navigates and nothing is armed — the pre-#83
        // one-step-per-event feel — regardless of the cooling flag.
        assert_eq!(gate(false, 0), Gate::Navigate);
        assert_eq!(gate(true, 0), Gate::Navigate);
    }
}
