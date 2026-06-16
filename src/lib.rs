//! zellij-tabmap — a multi-row zellij tab bar that renders each tab as a
//! color-coded minimap of its pane layout.
//!
//! The plugin holds the latest tab and pane snapshots zellij hands it and, on
//! every relevant event, repaints. The actual pixel rendering lives in the
//! dependency-free [`minimap`] module so it can be unit-tested off-wasm.

pub mod color;
pub mod config;
pub mod line;
pub mod minimap;
pub mod paint;
pub mod projection;
pub mod tab_block;
pub mod title;

use std::collections::BTreeMap;
use zellij_tile::prelude::*;

use config::Config;

/// The fewest text rows the bar can legibly fill. The minimap renders 2 pixel
/// rows per text row, so 3 rows is the floor for a 6px canvas that fits a
/// minimap plus labels. zellij assigns the row count from the layout's
/// `pane size=N` and the bar grows to fill whatever it is handed; given fewer
/// than this floor, `render` draws nothing rather than emit a clipped block.
const MIN_ROWS: usize = 3;

/// Plugin state: parsed configuration plus the most recent tab and pane
/// snapshots from zellij, and the theme-derived color palette.
#[derive(Default)]
pub struct State {
    config: Config,
    permitted: bool,
    tabs: Vec<TabInfo>,
    panes: PaneManifest,
    /// Pane colors derived from the live theme. Starts at the default-theme
    /// fallback (see [`color::Palette::default`]) and is refreshed on every
    /// `ModeUpdate`, which is how zellij delivers the active style.
    palette: color::Palette,
    /// The most recent render's per-tab column spans — the source of truth for
    /// click hit-testing. Re-recorded on every `render()` (and renders fire on
    /// each Tab/Pane update), so a click always tests against what is currently
    /// drawn, never a stale frame. Empty until the first render.
    tab_layout: Vec<line::TabHit>,
    /// The in-progress tab drag, if any. Set when a press lands on a tab,
    /// resolved (and cleared) on release. `None` whenever no drag is underway.
    /// v2 drag-to-reorder (#10).
    drag: Option<DragState>,
}

/// An in-progress tab drag. Recorded from the `LeftClick` that begins a press
/// and resolved on `Release`; `dragging` flips only once a `Hold` arrives, so a
/// plain click (press + release, no motion) never reorders. The grabbed tab is
/// tracked by its **stable** `tab_id`, never its position — focus (the same
/// click also switches) and every reorder hop reshuffle positions, but the id
/// is invariant, so on release the tab's current slot is re-derived from it.
#[derive(Clone, Copy)]
struct DragState {
    grabbed_tab_id: usize,
    dragging: bool,
}

/// Convert a zellij theme color to the renderer's [`color::Rgb`].
fn rgb(c: PaletteColor) -> color::Rgb {
    match c {
        PaletteColor::Rgb(v) => v,
        PaletteColor::EightBit(n) => color::from_eightbit(n),
    }
}

/// Build the pane palette from the active theme style.
///
/// Slots come from the theme's `multiplayer_user_colors` — the set a theme
/// author designs to tell *different session users apart*, which is exactly
/// this bar's job: telling *different panes apart*. Being categorical
/// distinguishing colors, they read as coherent adjacent fills on the bar
/// background by construction — unlike the `emphasis` foreground-accent colors
/// an earlier version scraped, which are tuned to sit *on top of* a fill, not
/// beside one, and so never cohered as a minimap ramp. A theme defines only as
/// many player slots as it cares to; the rest stay unset and collapse to the
/// black sentinel that [`color::Palette::new`] drops — so a theme defining five
/// players yields five hues. The focused pane keeps its slot fill, and its
/// focus ring is derived from that fill as a luminance-shifted shade — the
/// outline stays in the pane's own hue family (issue #47).
/// `frame_highlight.base` is the accent that seeds the degraded-rung hint
/// text shade ([`color::Palette::hint`], issue #32).
fn palette_from_style(style: &Style) -> color::Palette {
    let colors = &style.colors;
    let players = colors.multiplayer_user_colors;
    let slots = [
        players.player_1,
        players.player_2,
        players.player_3,
        players.player_4,
        players.player_5,
        players.player_6,
        players.player_7,
        players.player_8,
        players.player_9,
        players.player_10,
    ]
    .into_iter()
    .map(rgb)
    .collect();
    color::Palette::new(slots, rgb(colors.frame_highlight.base))
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // Deliberately NO `set_selectable(false)` here: the pane stays
        // selectable until the permission flow resolves, and the
        // PermissionRequestResult arm (see `update`) pins it. Pinning before
        // `request_permission` made the pane unfocusable while the interactive
        // prompt was on screen, so `y` could never reach it (#54). Both load
        // paths still pin within the first event: a cache-granted load (the
        // normal `default_tab_template` path) receives `Granted` immediately
        // after load, and an ad-hoc load with no grant stays focusable until
        // the user answers the prompt — which then delivers the result.
        self.config = Config::from_configuration(&configuration);
        // Request exactly the permissions the active config needs — see
        // [`Self::permissions`]. A plugin started from `default_tab_template`
        // cannot show the interactive permission dialog (zellij#4982), so users
        // pre-grant the set in the plugin permission cache and reload; granting
        // is all-or-nothing (event delivery freezes until every requested
        // permission is cached), which is exactly why the set must stay minimal
        // by default (#23): an existing v0.1.0 user who cached only Read+Change
        // must not hit a third-permission cache miss on auto-update.
        request_permission(&Self::permissions(&self.config));
        subscribe(&[
            EventType::PermissionRequestResult,
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::ModeUpdate,
            EventType::Mouse,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(status) => {
                self.permitted = status == PermissionStatus::Granted;
                // The permission flow has resolved — pin the pane
                // non-selectable now, the earliest moment that cannot trap the
                // interactive prompt (#54). A fixed-size (`size=3`)
                // default_tab_template pane is only stable once the plugin is
                // non-selectable, and this fires within the first event there
                // (a cache grant emits `Granted` right after load). Pinning is
                // unconditional on purpose: a `Denied` bar renders nothing and
                // must not linger as a focusable dead pane in the tab order.
                set_selectable(false);
                true
            }
            Event::TabUpdate(tabs) => {
                self.tabs = tabs;
                true
            }
            Event::PaneUpdate(panes) => {
                self.panes = panes;
                true
            }
            Event::ModeUpdate(mode_info) => {
                // zellij delivers the active theme via the mode style. Refresh
                // the palette and repaint so pane colors track theme changes.
                self.palette = palette_from_style(&mode_info.style);
                true
            }
            Event::Mouse(Mouse::LeftClick(_row, column)) => {
                // A left click anywhere in a tab's column span focuses that tab
                // (#8); the clicked row is irrelevant. The switch arrives back as
                // a TabUpdate, which drives the repaint — so this arm requests
                // none. It also records the press as a *potential* drag (#10):
                // if the pointer then holds and releases elsewhere, the tab is
                // reordered; a plain click never sets `dragging`, so it is a
                // pure switch. Press on no tab clears any stale drag.
                self.switch_to_tab_at(column);
                self.drag = self.grab_at(column);
                false
            }
            Event::Mouse(Mouse::Hold(..)) => {
                // The pointer moved while pressed → the press is a real drag.
                // Only the fact matters here, not the column; the drop column is
                // read from the Release. (#10)
                self.mark_dragging()
            }
            Event::Mouse(Mouse::Release(_row, column)) => {
                // Release ends the gesture: reorder the grabbed tab to the drop
                // column (no-op unless it was actually dragging), then clear the
                // drag regardless. (#10)
                let moved = self.commit_drag_at(column);
                self.drag = None;
                moved
            }
            // Remaining events need no repaint.
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // Reset the click geometry up front. If this frame bails out before
        // drawing — no permission yet, no active tab mid-transition, or too few
        // rows to draw — a click must find no spans to resolve against rather
        // than the previous frame's stale ones. The success path repopulates it
        // at the end.
        self.tab_layout.clear();
        if !self.permitted {
            return;
        }
        // zellij hands us the row count from the layout's `pane size=N`. Below
        // the floor the bar can't be drawn legibly, so render nothing rather
        // than clip a block into too little height. Still clear the visible rows
        // first: zellij does not blank the pane between frames (that is why
        // `compose` homes and erases every row), so a bail-out after a prior good
        // frame — e.g. the terminal shrank below the floor — would otherwise leave
        // stale tab rows lingering.
        if rows < MIN_ROWS {
            print!("{}", paint::compose(rows, &[], &[]));
            return;
        }
        let Some(active_position) = projection::active_tab(&self.tabs).map(|tab| tab.position)
        else {
            return;
        };

        // Pack the whole tab strip into column spans — active block anchored per
        // `config.align` (centered → the strip slides to follow focus; left →
        // pinned at column 0), the tabs that don't fit collapsed into `← +N` /
        // `+N →` end markers — then render each visible tab into its budgeted
        // block. `pack` clamps the active width into the legible `16..=28` range,
        // so the parser keeps the raw value (see `config.rs`); §4.3–4.4 of the design.
        let layout = line::pack(
            cols,
            0,
            self.config.active_width,
            self.tabs.len(),
            active_position,
            self.config.align,
            self.config.tab_gap,
        );

        // Project only the visible tabs' tiled panes (the collapsed ones need no
        // block). Projection is the one step that touches zellij types, so it
        // happens here at the render site and `paint::bar` stays pure. Panes are
        // keyed by `position` so the output never depends on the manifest map's
        // iteration order.
        let panes_by_position: BTreeMap<usize, Vec<minimap::PaneRect>> = layout
            .tabs
            .iter()
            .map(|hit| {
                let panes = self
                    .panes
                    .panes
                    .get(&hit.position)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                (hit.position, projection::project(panes))
            })
            .collect();

        print!(
            "{}",
            paint::bar(
                rows,
                &layout,
                &panes_by_position,
                &self.palette,
                &self.config.shortcut_prefix,
                self.config.gradient,
                self.config.inactive_dim,
                self.config.perspective,
            )
        );

        // Record the spans this frame drew so a later click hit-tests against
        // the current layout. `pack` re-runs every render, so this is always
        // the live geometry — never a cached copy.
        self.tab_layout = layout.tabs;
    }
}

impl State {
    /// The permission set this config requires. `ReadApplicationState` (Tab/
    /// Pane/Mode updates) and `ChangeApplicationState` (`switch_tab_to` for
    /// click-to-switch, #8) are always needed. `RunActionsAsUser` (the
    /// `MoveTab` run_action behind drag-to-reorder, #10) is added **only
    /// when `reorder` is enabled** (#23) — so the default request matches the
    /// v0.1.0 two-permission set and existing auto-updaters never freeze on a
    /// cache miss (zellij#4982). Pure, so the gating is unit-tested directly
    /// (host imports are stubbed off-wasm, so what `load` requests is otherwise
    /// unobservable).
    fn permissions(config: &Config) -> Vec<PermissionType> {
        [
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]
        .into_iter()
        .chain(config.reorder.then_some(PermissionType::RunActionsAsUser))
        .collect()
    }

    /// Focus the tab whose drawn block contains `column`; a click that landed on
    /// no block (overflow marker, gap, trailing padding) is a no-op. `column` is
    /// the 0-based click column zellij delivers, and
    /// [`line::switch_target_at_column`] resolves it to the 1-based index
    /// `switch_tab_to` expects.
    fn switch_to_tab_at(&self, column: usize) {
        let Some(target) = line::switch_target_at_column(&self.tab_layout, column) else {
            return;
        };
        switch_tab_to(target);
    }

    /// Record a potential drag for the tab drawn at `column`. `None` when the
    /// press landed on no tab (overflow marker, gap, padding) — nothing to drag.
    /// The tab is captured by its stable `tab_id` (resolved from the current
    /// layout's position) so the release can find it after any position shift.
    ///
    /// Short-circuits to `None` when `reorder` is disabled (#23): without it the
    /// plugin lacks `RunActionsAsUser`, so arming a drag would only leave the
    /// gesture "armed but inert" — its `MoveTab` silently dropped at the
    /// host boundary. Refusing to arm keeps a press a clean switch-only no-op.
    fn grab_at(&self, column: usize) -> Option<DragState> {
        if !self.config.reorder {
            return None;
        }
        let position = line::position_at_column(&self.tab_layout, column)?;
        let grabbed_tab_id = self
            .tabs
            .iter()
            .find(|tab| tab.position == position)?
            .tab_id;
        Some(DragState {
            grabbed_tab_id,
            dragging: false,
        })
    }

    /// Promote the in-progress drag to actually dragging (a `Hold` arrived).
    /// No-op when nothing was grabbed. Returns `false`: the drag has no visual
    /// yet, so no repaint is warranted (a drop indicator is deferred to a
    /// follow-up — see the PR notes).
    fn mark_dragging(&mut self) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        drag.dragging = true;
        false
    }

    /// On release, reorder the grabbed tab to the drop `column`: resolve the
    /// pure [`reorder_plan`](Self::reorder_plan), then emit it. No-op (returns
    /// `false`) when the plan is `None`. Keeping the decision in `reorder_plan`
    /// leaves this method a thin resolve→effect seam and makes the move math
    /// testable without a zellij host.
    fn commit_drag_at(&self, column: usize) -> bool {
        let Some((_, shift, steps)) = self.reorder_plan(column) else {
            return false;
        };
        self.reorder(shift, steps);
        true
    }

    /// The reorder decision for a release at `column`: the grabbed tab's stable
    /// id, which way to shift it, and how many neighbour hops — or `None` when
    /// nothing should move (no drag in motion, the grabbed tab is gone, or the
    /// drop lands on its own slot). Pure: reads state and derives the plan but
    /// emits no action. The grabbed tab's *current* position is re-derived from
    /// its stable id (focus and every prior hop reshuffle positions, the id is
    /// invariant); [`line::drag_steps`] then gives the direction and neighbour
    /// count. That re-derivation is the invariant that keeps a focus/hop repack
    /// from corrupting the move, so it is pinned by unit tests.
    fn reorder_plan(&self, column: usize) -> Option<(usize, line::Shift, usize)> {
        let drag = self.drag.filter(|drag| drag.dragging)?;
        let from = self
            .tabs
            .iter()
            .find(|tab| tab.tab_id == drag.grabbed_tab_id)
            .map(|tab| tab.position)?;
        let (shift, steps) = line::drag_steps(&self.tab_layout, from, column)?;
        Some((drag.grabbed_tab_id, shift, steps))
    }

    /// Emit one `MoveTab` per step. `MoveTab` shifts the **focused** tab a
    /// single neighbour per call, and the press that armed this drag already
    /// focused the grabbed tab (`switch_to_tab_at`), so emitting it `steps`
    /// times walks that tab into the drop slot. The by-id variant
    /// (`MoveTabByTabId`) cannot be used here: zellij-utils classifies it as
    /// CLI-only and the `run_action` shim `unwrap`s the failed protobuf
    /// conversion — a guaranteed panic on every drag commit (pinned by the
    /// release tests). Needs the `RunActionsAsUser` permission; without it
    /// the host drops the action and reorder is inert.
    fn reorder(&self, shift: line::Shift, steps: usize) {
        let direction = match shift {
            line::Shift::Left => Direction::Left,
            line::Shift::Right => Direction::Right,
        };
        (0..steps).for_each(|_| {
            run_action(actions::Action::MoveTab { direction }, BTreeMap::new());
        });
    }
}

// Native test builds link the whole lib, which references zellij-tile's host
// imports (all routed through `host_run_plugin_command`). Provide the symbol
// so `cargo test --lib --target <host>` links off-wasm. On wasm the real host
// supplies it, so this stub is compiled only under `cfg(test)`.
//
// The stub counts its calls per test thread: the command payload travels over
// stdout (invisible here), but the *number* of host commands a hook emits is
// observable, which is enough to pin selectability ordering — e.g. that `load`
// emits no `set_selectable` ahead of the permission flow (#54). Thread-local
// so parallel tests never see each other's counts; tests measure deltas via
// `host_commands_during`, which also keeps them correct under
// `--test-threads=1` thread reuse.
#[cfg(test)]
thread_local! {
    static HOST_COMMAND_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[no_mangle]
extern "C" fn host_run_plugin_command() {
    HOST_COMMAND_CALLS.with(|calls| calls.set(calls.get() + 1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{Shift, TabHit};

    /// The number of host commands `body` emits, as a delta of this thread's
    /// stub counter — robust whether the harness gives each test its own
    /// thread (the default) or reuses one (`--test-threads=1`).
    fn host_commands_during(body: impl FnOnce()) -> usize {
        let before = HOST_COMMAND_CALLS.with(std::cell::Cell::get);
        body();
        HOST_COMMAND_CALLS.with(std::cell::Cell::get) - before
    }

    /// A `TabInfo` carrying only the two fields the reorder math reads.
    fn tab(position: usize, tab_id: usize) -> TabInfo {
        TabInfo {
            position,
            tab_id,
            ..Default::default()
        }
    }

    /// A drawn span; `active` is irrelevant to drag resolution, so it is `false`.
    fn hit(position: usize, start: usize, width: usize) -> TabHit {
        TabHit {
            position,
            start,
            width,
            active: false,
        }
    }

    /// Five contiguous 4-column blocks: position `p` owns columns `4p..4p+4`,
    /// so positions span 0..5 across columns 0..20.
    fn five_block_layout() -> Vec<TabHit> {
        (0..5).map(|p| hit(p, p * 4, 4)).collect()
    }

    #[test]
    fn reorder_plan_resolves_from_by_the_grabbed_tabs_current_position() {
        // The drag stores only the *stable* tab_id. Here the grabbed tab (id
        // 100) currently sits at position 1 — deliberately not position 0, the
        // slot a grab-time position snapshot might have frozen — so the plan
        // MUST look up its live slot by id. Release lands in position 4's span
        // (columns 16..20), making the move 3 hops right (1 → 4). If the code
        // ever resolved `from` from a cached position instead of the current
        // tabs, this count would change and the test would fail.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 100), tab(2, 8), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 100,
            dragging: true,
        });

        assert_eq!(
            state.reorder_plan(18),
            Some((100, Shift::Right, 3)),
            "from is the grabbed tab's current position (1), resolved by stable id"
        );
    }

    #[test]
    fn reorder_plan_is_none_when_the_press_never_became_a_drag() {
        // `dragging` stays false for a plain click (press + release, no motion).
        // The plan must be `None` so a click is a pure switch, never a reorder —
        // even though the release column would otherwise resolve to a move.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 100), tab(2, 8), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 100,
            dragging: false,
        });

        assert_eq!(state.reorder_plan(18), None);
    }

    #[test]
    fn reorder_plan_is_none_when_the_grabbed_tab_vanished() {
        // A tab closed mid-drag: the grabbed id is no longer among the tabs, so
        // there is no current position to move from. The plan must be `None`
        // rather than panicking or moving the wrong tab.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 9)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 999,
            dragging: true,
        });

        assert_eq!(state.reorder_plan(18), None);
    }

    #[test]
    fn reorder_plan_is_none_when_dropped_on_its_own_slot() {
        // The grabbed tab (id 100, position 2) is released within its own block
        // (columns 8..12). `drag_steps` yields zero hops, so the plan is `None`
        // — a drag that ends where it started moves nothing.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 100), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 100,
            dragging: true,
        });

        assert_eq!(state.reorder_plan(9), None);
    }

    #[test]
    fn reorder_plan_is_none_without_a_drag() {
        // No press was recorded (e.g. release with no prior grab). `None` drag
        // → `None` plan, no panic.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7)];
        state.tab_layout = five_block_layout();

        assert_eq!(state.reorder_plan(18), None);
    }

    #[test]
    fn render_clears_tab_layout_when_it_cannot_draw() {
        // The bar only draws once permitted, and only repopulates `tab_layout`
        // on that success path. A frame that bails out earlier must still wipe
        // the previous frame's spans — otherwise a click would resolve against
        // geometry no longer on screen. (`permitted` defaults to false, so this
        // exercises the pre-draw early return.)
        let mut state = State::default();
        state.tab_layout = vec![TabHit {
            position: 3,
            start: 0,
            width: 8,
            active: true,
        }];

        state.render(MIN_ROWS, 80);

        assert!(
            state.tab_layout.is_empty(),
            "a frame that cannot draw leaves no stale click geometry"
        );
    }

    #[test]
    fn render_draws_nothing_below_the_minimum_row_count() {
        // zellij assigns the row count from the layout's `pane size=N`, which can
        // be fewer than the bar needs. The half-block canvas needs `MIN_ROWS`
        // text rows to be legible, so a shorter bar renders nothing — proven here
        // by the click geometry staying empty even though permission is granted
        // and an active tab exists (the row count is the only thing withholding
        // the draw).
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            position: 0,
            active: true,
            ..Default::default()
        }];

        state.render(MIN_ROWS - 1, 80);
        assert!(
            state.tab_layout.is_empty(),
            "a bar with too few rows draws nothing and records no click geometry"
        );

        // Contrast: at the floor the very same state draws and records its spans,
        // so the empty result above is the row guard, not a setup miss.
        state.render(MIN_ROWS, 80);
        assert!(
            !state.tab_layout.is_empty(),
            "at the minimum row count the bar draws and records click geometry"
        );
    }

    #[test]
    fn permissions_exclude_run_actions_when_reorder_off() {
        // The default (reorder off) requests exactly the v0.1.0 two-permission
        // set, so an existing user who cached only Read+Change keeps working on
        // auto-update — no `RunActionsAsUser` cache miss, no frozen bar
        // (zellij#4982).
        let config = Config {
            reorder: false,
            ..Default::default()
        };
        assert_eq!(
            State::permissions(&config),
            vec![
                PermissionType::ReadApplicationState,
                PermissionType::ChangeApplicationState,
            ]
        );
    }

    #[test]
    fn permissions_include_run_actions_when_reorder_on() {
        // Opting into reorder adds the third permission `MoveTab` needs.
        let config = Config {
            reorder: true,
            ..Default::default()
        };
        assert_eq!(
            State::permissions(&config),
            vec![
                PermissionType::ReadApplicationState,
                PermissionType::ChangeApplicationState,
                PermissionType::RunActionsAsUser,
            ]
        );
    }

    #[test]
    fn grab_is_inert_when_reorder_off() {
        // With reorder off the plugin never even arms a drag: a press over a tab
        // records no `DragState`, so the gesture is a clean no-op rather than an
        // "armed but inert" drag whose `MoveTab` is silently dropped at
        // the host boundary for lack of `RunActionsAsUser`.
        let mut state = State::default();
        state.config = Config {
            reorder: false,
            ..Default::default()
        };
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 100), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();

        assert!(state.grab_at(9).is_none());
    }

    #[test]
    fn grab_arms_a_drag_when_reorder_on() {
        // With reorder on, a press over position 2's block (columns 8..12) arms a
        // drag on that tab's stable id (100), not yet dragging.
        let mut state = State::default();
        state.config = Config {
            reorder: true,
            ..Default::default()
        };
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 100), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();

        assert!(matches!(
            state.grab_at(9),
            Some(DragState {
                grabbed_tab_id: 100,
                dragging: false
            })
        ));
    }

    /// A configuration map as zellij would deliver it from the KDL block.
    fn config_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// A content pane zellij would report for a tiled layout.
    fn content_pane(x: usize, y: usize, w: usize, h: usize) -> PaneInfo {
        PaneInfo {
            pane_x: x,
            pane_y: y,
            pane_columns: w,
            pane_rows: h,
            title: "sh".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn load_parses_the_configuration_before_requesting_permissions() {
        // `load` must parse the delivered map first — the permission request
        // depends on `reorder` (#23). Host imports are stubbed natively, so the
        // observable contract is the parsed config; what gets *requested* per
        // config is pinned separately by the `permissions_*` tests.
        let mut state = State::default();
        state.load(config_map(&[("reorder", "true"), ("active_width", "20")]));

        assert!(state.config.reorder);
        assert_eq!(state.config.active_width, 20);
        assert!(!state.permitted, "permission arrives later as an event");
    }

    #[test]
    fn load_keeps_the_pane_selectable_until_the_permission_flow_resolves() {
        // `load` must emit exactly two host commands — `request_permission`
        // and `subscribe` — and crucially NO `set_selectable`: pinning the
        // pane non-selectable before the permission request made the
        // interactive prompt unfocusable, so an ad-hoc load could never be
        // answered (#54). The pin belongs to the PermissionRequestResult arm.
        let mut state = State::default();

        assert_eq!(
            host_commands_during(|| state.load(config_map(&[]))),
            2,
            "request_permission + subscribe only — no pre-grant set_selectable"
        );
    }

    #[test]
    fn permission_result_pins_non_selectable_on_both_outcomes() {
        // Each resolution emits exactly one host command — the
        // `set_selectable(false)` pin. Granted stabilizes the fixed-size
        // template pane within the first event; Denied must pin too, so a
        // refused prompt never leaves a focusable dead pane in the tab order.
        let mut state = State::default();

        let granted = host_commands_during(|| {
            state.update(Event::PermissionRequestResult(PermissionStatus::Granted));
        });
        assert_eq!(granted, 1, "Granted pins the pane non-selectable");

        let denied = host_commands_during(|| {
            state.update(Event::PermissionRequestResult(PermissionStatus::Denied));
        });
        assert_eq!(denied, 1, "Denied pins too — no focusable dead pane");
    }

    #[test]
    fn permission_result_tracks_granted_and_denied() {
        // Both outcomes repaint (the bar must redraw either way), and only
        // Granted flips `permitted` — a Denied result must not leave a stale
        // grant behind.
        let mut state = State::default();

        assert!(state.update(Event::PermissionRequestResult(PermissionStatus::Granted)));
        assert!(state.permitted);

        assert!(state.update(Event::PermissionRequestResult(PermissionStatus::Denied)));
        assert!(!state.permitted);
    }

    #[test]
    fn tab_and_pane_updates_replace_the_snapshots_and_repaint() {
        // The plugin stores whatever snapshot zellij hands it wholesale; both
        // events request a repaint so the bar tracks the live session.
        let mut state = State::default();

        assert!(state.update(Event::TabUpdate(vec![tab(0, 1), tab(1, 2)])));
        assert_eq!(state.tabs.len(), 2);

        let mut manifest = PaneManifest::default();
        manifest.panes.insert(0, vec![content_pane(0, 1, 80, 24)]);
        assert!(state.update(Event::PaneUpdate(manifest)));
        assert_eq!(state.panes.panes.len(), 1);
    }

    #[test]
    fn mode_update_swaps_the_palette_from_the_live_theme() {
        // zellij delivers the active theme via the mode style; the palette must
        // be rebuilt from it so pane colors track theme changes.
        let mut state = State::default();
        let mut mode_info = ModeInfo::default();
        mode_info.style.colors.multiplayer_user_colors.player_1 = PaletteColor::Rgb((10, 20, 30));

        assert!(state.update(Event::ModeUpdate(mode_info)));
        assert_eq!(state.palette.color_for(0), (10, 20, 30));
    }

    #[test]
    fn left_click_on_a_tab_arms_a_drag_and_defers_the_repaint() {
        // The click switches tabs via the host (stubbed here) and records the
        // press as a potential drag. No repaint is requested — the switch
        // arrives back as a TabUpdate, which drives the redraw.
        let mut state = State::default();
        state.config = Config {
            reorder: true,
            ..Default::default()
        };
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 100), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();

        assert!(!state.update(Event::Mouse(Mouse::LeftClick(0, 9))));
        assert!(matches!(
            state.drag,
            Some(DragState {
                grabbed_tab_id: 100,
                dragging: false
            })
        ));
    }

    #[test]
    fn left_click_off_any_tab_clears_a_stale_drag() {
        // A press past the drawn blocks (columns 0..20) resolves to no tab:
        // nothing to switch to, and any stale drag is dropped rather than left
        // armed against geometry the press never touched.
        let mut state = State::default();
        state.config = Config {
            reorder: true,
            ..Default::default()
        };
        state.tabs = vec![tab(0, 7)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 7,
            dragging: false,
        });

        assert!(!state.update(Event::Mouse(Mouse::LeftClick(0, 30))));
        assert!(state.drag.is_none());
    }

    #[test]
    fn hold_promotes_the_press_to_a_real_drag() {
        // Motion while pressed is what separates a drag from a plain click;
        // only the fact matters, the drop column is read from the Release.
        let mut state = State::default();
        state.drag = Some(DragState {
            grabbed_tab_id: 100,
            dragging: false,
        });

        assert!(!state.update(Event::Mouse(Mouse::Hold(0, 12))));
        assert!(matches!(state.drag, Some(DragState { dragging: true, .. })));
    }

    #[test]
    fn hold_without_a_grab_is_a_no_op() {
        // A Hold with no recorded press (e.g. the press landed off any tab)
        // must not conjure a drag out of nothing.
        let mut state = State::default();

        assert!(!state.update(Event::Mouse(Mouse::Hold(0, 12))));
        assert!(state.drag.is_none());
    }

    #[test]
    fn release_commits_a_rightward_drag_and_requests_a_repaint() {
        // The full gesture: a dragging grab on id 100 (currently position 1)
        // released over position 4's block resolves a rightward plan and emits
        // `MoveTab` once per hop (the press already focused the grabbed tab,
        // so the focused-tab move walks exactly that tab). The choice of
        // action is load-bearing: the by-id variant (`MoveTabByTabId`) is
        // CLI-only in zellij-utils and `run_action`'s shim panics on its
        // failed protobuf conversion — this test running update() through the
        // real emit pins that the emitted action converts cleanly.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 100), tab(2, 8), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 100,
            dragging: true,
        });

        assert!(
            state.update(Event::Mouse(Mouse::Release(0, 18))),
            "a committed drag must request a repaint"
        );
        assert!(state.drag.is_none(), "release always clears the drag");
    }

    #[test]
    fn release_commits_a_leftward_drag() {
        // Same gesture mirrored: id 100 (position 3) dropped on position 0's
        // block resolves a leftward plan — covering the `Shift::Left`
        // direction arm — and the emit passes the same shim boundary.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 9), tab(3, 100), tab(4, 10)];
        state.tab_layout = five_block_layout();
        state.drag = Some(DragState {
            grabbed_tab_id: 100,
            dragging: true,
        });

        assert_eq!(state.reorder_plan(1), Some((100, Shift::Left, 3)));
        assert!(state.update(Event::Mouse(Mouse::Release(0, 1))));
        assert!(state.drag.is_none());
    }

    #[test]
    fn release_without_a_drag_requests_no_repaint() {
        // A Release with nothing grabbed (or after a plain click) moves nothing
        // and must not waste a repaint.
        let mut state = State::default();
        state.tab_layout = five_block_layout();

        assert!(!state.update(Event::Mouse(Mouse::Release(0, 18))));
        assert!(state.drag.is_none());
    }

    #[test]
    fn unrelated_events_request_no_repaint() {
        // Anything outside the subscribed working set falls through the
        // catch-all arm without touching state.
        let mut state = State::default();

        assert!(!state.update(Event::Visible(true)));
    }

    #[test]
    fn render_draws_the_bar_and_records_click_geometry() {
        // The success path: permitted, an active tab, and a pane manifest. The
        // frame paints (to the captured test stdout) and re-records the spans a
        // later click hit-tests against — including the active block.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            tab(1, 2),
            tab(2, 3),
        ];
        state.panes.panes.insert(
            0,
            vec![content_pane(0, 1, 40, 24), content_pane(40, 1, 40, 24)],
        );

        state.render(MIN_ROWS, 80);

        assert!(!state.tab_layout.is_empty(), "the frame records its spans");
        assert!(
            state.tab_layout.iter().any(|hit| hit.active),
            "the active tab's span is among the recorded hits"
        );
    }

    #[test]
    fn render_clears_geometry_when_no_tab_is_active() {
        // Permitted but mid-transition (no tab marked active): the frame bails
        // out after wiping the previous spans, so a click resolves against
        // nothing rather than geometry no longer on screen.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![tab(0, 1)];
        state.tab_layout = five_block_layout();

        state.render(MIN_ROWS, 80);

        assert!(state.tab_layout.is_empty());
    }

    #[test]
    fn palette_slots_come_from_multiplayer_user_colors() {
        // The follow palette draws pane fills from the theme's categorical
        // "distinguish session users" colors. A theme that defines three
        // players and leaves the rest unset must yield exactly those three
        // hues, in declaration order, with the unset (black-sentinel) slots
        // dropped — so pane identity cycles over a coherent theme-authored ramp
        // rather than the foreground emphasis colors an earlier version scraped.
        let mut style = Style::default();
        style.colors.multiplayer_user_colors = MultiplayerColors {
            player_1: PaletteColor::Rgb((10, 20, 30)),
            player_2: PaletteColor::Rgb((40, 50, 60)),
            player_4: PaletteColor::Rgb((70, 80, 90)),
            // player_3 and player_5..=player_10 stay EightBit(0) → dropped.
            ..Default::default()
        };
        style.colors.frame_highlight.base = PaletteColor::Rgb((200, 100, 50));

        let p = palette_from_style(&style);

        // Exactly the three defined hues, cycled by identity in declaration
        // order (player_3 dropped between player_2 and player_4).
        assert_eq!(p.color_for(0), (10, 20, 30));
        assert_eq!(p.color_for(1), (40, 50, 60));
        assert_eq!(p.color_for(2), (70, 80, 90));
        assert_eq!(p.color_for(3), (10, 20, 30));

        // The ring is derived from the pane's own fill as a luminance-shifted
        // shade (issue #47), so the outline stays in the pane's hue family
        // rather than tracking the theme accent.
        assert_ne!(p.ring_for(0), p.color_for(0));
    }
}
