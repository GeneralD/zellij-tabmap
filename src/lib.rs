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

/// Text-row height of the bar. The layout pins the plugin pane to `size=3`, and
/// the minimap renders 2 vertical pixels per text row → a 6px-tall canvas.
const ROWS: usize = 3;

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
/// players yields five hues. The focused pane uses `frame_highlight.base` as
/// its accent fill; the ring is derived from that accent as a luminance-shifted
/// shade ([`color::Palette::new`] with `None`), so the outline always tracks the
/// focused fill instead of a separately-scraped theme color (issue #32).
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
    color::Palette::new(slots, rgb(colors.frame_highlight.base), None)
}

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        // A fixed-size (`size=3`) default_tab_template pane is only stable when
        // the plugin marks itself non-selectable. Assert it first, then again
        // on PermissionResult (see `update`), since the post-permission
        // re-render is when a stale selectable state would surface.
        set_selectable(false);
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
                // Re-assert non-selectable: the post-permission re-render is the
                // moment a stale selectable state would destabilize the bar.
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

    fn render(&mut self, _rows: usize, cols: usize) {
        // Reset the click geometry up front. If this frame bails out before
        // drawing — no permission yet, or no active tab mid-transition — a click
        // must find no spans to resolve against rather than the previous frame's
        // stale ones. The success path repopulates it at the end.
        self.tab_layout.clear();
        if !self.permitted {
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
                ROWS,
                &layout,
                &panes_by_position,
                &self.palette,
                &self.config.shortcut_prefix,
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
    /// `MoveTabByTabId` run_action behind drag-to-reorder, #10) is added **only
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
    /// gesture "armed but inert" — its `MoveTabByTabId` silently dropped at the
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
        let Some((tab_id, shift, steps)) = self.reorder_plan(column) else {
            return false;
        };
        self.reorder(tab_id, shift, steps);
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

    /// Emit one `MoveTabByTabId` per step. The action shifts a tab a single
    /// neighbour per call and targets the **stable** id, so emitting it `steps`
    /// times walks that tab that many slots in `shift`'s direction — immune to
    /// the positions reshuffling under each hop. Needs the `RunActionsAsUser`
    /// permission; without it the host drops the action and reorder is inert.
    fn reorder(&self, tab_id: usize, shift: line::Shift, steps: usize) {
        let direction = match shift {
            line::Shift::Left => Direction::Left,
            line::Shift::Right => Direction::Right,
        };
        (0..steps).for_each(|_| {
            run_action(
                actions::Action::MoveTabByTabId {
                    id: tab_id as u64,
                    direction,
                },
                BTreeMap::new(),
            );
        });
    }
}

// Native test builds link the whole lib, which references zellij-tile's host
// imports (all routed through `host_run_plugin_command`). Provide the symbol
// so `cargo test --lib --target <host>` links off-wasm. On wasm the real host
// supplies it, so this stub is compiled only under `cfg(test)`.
#[cfg(test)]
#[no_mangle]
extern "C" fn host_run_plugin_command() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{Shift, TabHit};

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

        state.render(ROWS, 80);

        assert!(
            state.tab_layout.is_empty(),
            "a frame that cannot draw leaves no stale click geometry"
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
        // Opting into reorder adds the third permission `MoveTabByTabId` needs.
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
        // "armed but inert" drag whose `MoveTabByTabId` is silently dropped at
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

        // Focus fill is frame_highlight.base; the ring is derived from that
        // accent (issue #32), so it tracks the focused fill rather than a
        // separately-scraped theme color. style_for hands back the palette ring.
        let focused = p.style_for(0, true);
        assert_eq!(focused.fill, (200, 100, 50));
        assert_eq!(focused.ring, Some(p.ring()));
        assert_ne!(focused.ring, Some(focused.fill));
    }
}
