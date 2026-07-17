//! zellij-tabmap — a multi-row zellij tab bar that renders each tab as a
//! color-coded minimap of its pane layout.
//!
//! The plugin holds the latest tab and pane snapshots zellij hands it and, on
//! every relevant event, repaints. The actual pixel rendering lives in the
//! dependency-free [`minimap`] module so it can be unit-tested off-wasm.

pub mod color;
pub mod config;
pub mod floating;
pub mod line;
pub mod minimap;
pub mod paint;
pub mod pinned;
pub mod projection;
pub(crate) mod router;
pub mod scroll;
pub mod suppressed;
pub mod tab_block;
pub(crate) mod theme;
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
    /// Per-tab pinned floating-pane ids (#119), keyed by tab position and
    /// rebuilt on every `PaneUpdate` from a per-tab session-layout dump
    /// ([`Self::refresh_pinned`]) — pin state is invisible in `PaneInfo`.
    /// Drives the hidden-layer partition (pinned floats overlay, the rest
    /// chip), the corner pin marker, and the wheel/anchor visibility of
    /// hidden-layer pinned floats. Ids are `PaneRect`-space (`usize`).
    pinned_by_tab: BTreeMap<usize, Vec<usize>>,
    /// Pane colors derived from the live theme. Starts at the default-theme
    /// fallback (see [`color::Palette::default`]) and is refreshed on every
    /// `ModeUpdate`, which is how zellij delivers the active style.
    palette: color::Palette,
    /// Whether the terminal runs zellij's simplified UI (no Nerd Font). zellij
    /// surfaces this as `capabilities.arrow_fonts` — counterintuitively `true`
    /// means *simplified* (the flag is its internal "fall back to ASCII
    /// separators" signal), so this field mirrors it directly. Refreshed on
    /// every `ModeUpdate` alongside the palette, and defaults to `false`
    /// (assume a Nerd Font) until the first one lands. Drives the close-glyph
    /// fallback so the affordance never renders as tofu (#86).
    simplified_ui: bool,
    /// The most recent render's per-tab column spans — the source of truth for
    /// click hit-testing. Re-recorded on every `render()` (and renders fire on
    /// each Tab/Pane update), so a click always tests against what is currently
    /// drawn, never a stale frame. Empty until the first render.
    tab_layout: Vec<line::TabHit>,
    /// The most recent render's "+" button span — the source of truth for
    /// routing a click to "open a new tab" (#76). Re-recorded every `render()`
    /// alongside [`Self::tab_layout`] and cleared on the same bail-outs, so a
    /// click always tests against the live frame. `None` whenever the button is
    /// disabled, did not fit, or no frame has drawn yet.
    button_layout: Option<line::ButtonHit>,
    /// Per-tab pane geometry for click-to-focus (#74), keyed by tab position and
    /// rebuilt every render alongside `tab_layout`. Only tabs drawn as a minimap
    /// (the L0–L2 grid rungs) get an entry — narrow tabs (L3 glyph / L4 hint)
    /// draw no per-pane regions, so a click there has no pane to resolve. Cleared
    /// on every render bail-out, so a click never hit-tests a stale frame.
    tab_panes: BTreeMap<usize, router::TabPaneGeom>,
    /// The most recent render's per-tab close-button cells (#86) — one entry per
    /// grid-rung tab that drew an "×". Re-recorded every `render()` alongside
    /// `tab_layout` and cleared on the same bail-outs, so a `LeftClick` resolves
    /// "close tab N" against the live frame. Empty whenever the close button is
    /// disabled, only one tab is open, or no frame has drawn yet.
    close_layout: Vec<line::CloseHit>,
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
        // Request the bar's fixed two-permission set — see [`Self::permissions`].
        // A plugin started from `default_tab_template` cannot show the
        // interactive permission dialog (zellij#4982), so users pre-grant the set
        // in the plugin permission cache and reload; granting is all-or-nothing
        // (event delivery freezes until every requested permission is cached),
        // which is why the set stays minimal: an existing v0.1.0 user who cached
        // only Read+Change must not hit a new-permission cache miss on auto-update.
        request_permission(&Self::permissions());
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
                // Pin state rides only in the session-layout dump (#119) —
                // refresh it with the same cadence as the manifest it
                // correlates against.
                self.refresh_pinned();
                true
            }
            Event::ModeUpdate(mode_info) => {
                // zellij delivers the active theme via the mode style. Refresh
                // the palette and repaint so pane colors track theme changes.
                // The same event carries the terminal's font capability, which
                // selects the close-glyph (Nerd Font vs ASCII fallback, #86).
                self.palette = theme::palette_from_style(&mode_info.style);
                self.simplified_ui = mode_info.capabilities.arrow_fonts;
                true
            }
            Event::Mouse(Mouse::LeftClick(row, column)) => {
                // The pure router resolves the click against the live frame's
                // recorded geometry, in the priority the bar paints its
                // affordances: the "+" button (#76) > a tab's close "×" (#86) >
                // the finer click-to-focus minimap pane (#74) > a plain tab-switch
                // (#8) > nothing. This arm is the sole host-effect dispatcher —
                // it turns the one resolved intent into the one matching host
                // call. Every effect arrives back as a Tab/Pane update that drives
                // the repaint, so the click itself requests none.
                match router::route_click(
                    self.button_layout,
                    &self.close_layout,
                    &self.tab_layout,
                    &self.tab_panes,
                    row,
                    column,
                ) {
                    // zellij focuses the new tab; its TabUpdate drives the redraw.
                    router::ClickIntent::NewTab => {
                        let _opened = new_tab::<&str>(None, None);
                    }
                    // `close_tab_with_index` closes by position without focusing
                    // first, riding the already-granted `ChangeApplicationState`
                    // (#86). Consume the close target first so a duplicate click on
                    // the same cell can't re-dispatch off stale geometry before the
                    // next render rebuilds `close_layout` (the close shifts panes).
                    router::ClickIntent::CloseTab(position) => {
                        self.close_layout.clear();
                        close_tab_with_index(position);
                    }
                    // The pane survived projection's `is_plugin/is_floating/
                    // is_suppressed` filter, so it is a visible tiled terminal
                    // pane: never hidden, making both `should_float_if_hidden` and
                    // `should_be_in_place_if_hidden` moot — pass `false`. Focusing
                    // also switches to the pane's tab, so a click on a non-active
                    // tab's pane both switches and focuses in one step. Needs
                    // `ChangeApplicationState`, already granted for `switch_tab_to`
                    // (#8), so no new permission (#74).
                    router::ClickIntent::FocusPane(id) => {
                        focus_terminal_pane(id as u32, false, false);
                    }
                    // A floating chip (hidden layer), overlay (visible), or the `+k`
                    // overflow marker (targets the first folded hidden float, #113) —
                    // reveal and focus it. Unlike the tiled `FocusPane` arm, a hidden
                    // float has no on-screen footprint, so `should_float_if_hidden =
                    // true` both reveals its layer and focuses it in one step (#110).
                    // Rides the already-granted `ChangeApplicationState`.
                    router::ClickIntent::FocusFloatingPane(id) => {
                        focus_terminal_pane(id as u32, true, false);
                    }
                    router::ClickIntent::SwitchTab(target) => switch_tab_to(target),
                    router::ClickIntent::NoOp => {}
                }
                false
            }
            // The wheel carries no position, so it acts on the live tab/pane set
            // rather than a clicked spot (#80, restored #108). One event = one
            // step: the rate-limiter #104 deleted is gone for good — `off` mode
            // is the opt-out for stepless devices whose flick bursts step several
            // at once. Stock tab-bar direction: up = forward (next). The host
            // effect lands back as a Tab/Pane update that drives the repaint, so
            // the scroll itself requests none.
            Event::Mouse(Mouse::ScrollUp(_)) => {
                self.scroll(scroll::ScrollDir::Forward);
                false
            }
            Event::Mouse(Mouse::ScrollDown(_)) => {
                self.scroll(scroll::ScrollDir::Backward);
                false
            }
            // Remaining events need no repaint.
            _ => false,
        }
    }

    fn render(&mut self, rows: usize, cols: usize) {
        // Reset the click geometry up front. If this frame bails out before
        // drawing — no permission yet, no active tab mid-transition, or too few
        // rows to draw — a click must find no spans to resolve against rather
        // than the previous frame's stale ones. The success path repopulates both
        // at the end.
        self.tab_layout.clear();
        self.button_layout = None;
        self.tab_panes.clear();
        self.close_layout.clear();
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
        // Reserve the trailing "+" button only when enabled — `pack_with_button`
        // with `with_button: false` is exactly `pack`, recording no button — so
        // the disabled bar reclaims those columns for the tab strip. When on, the
        // button is sized to match the bar's inactive tabs (#76).
        let layout = line::pack_with_button(
            cols,
            0,
            self.config.active_width,
            self.tabs.len(),
            active_position,
            self.config.align,
            self.config.tab_gap,
            self.config.new_tab_button,
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

        // Build each visible tab's floating layer spec from its live pane set and
        // the tab's `are_floating_panes_visible` flag (#110). `floating = off`
        // yields an empty map, so every tab renders exactly as before. A hidden
        // layer becomes chips (ids); a visible layer becomes an overlay (rects,
        // painted in a later step). Keyed by position like `panes_by_position`.
        let floats_by_position: BTreeMap<usize, floating::FloatSpec> = match self.config.floating {
            floating::FloatingMode::Off => BTreeMap::new(),
            floating::FloatingMode::Hybrid => layout
                .tabs
                .iter()
                .map(|hit| {
                    let panes = self
                        .panes
                        .panes
                        .get(&hit.position)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let floats = projection::project_floating(panes);
                    let visible = self
                        .tabs
                        .iter()
                        .find(|t| t.position == hit.position)
                        .map(|t| t.are_floating_panes_visible)
                        .unwrap_or(false);
                    let spec = if floats.is_empty() {
                        floating::FloatSpec::None
                    } else if visible {
                        floating::FloatSpec::Visible(floats)
                    } else {
                        // A hidden layer's pinned floats stay on the real
                        // screen (#119) — zellij renders pinned floats while
                        // the layer is hidden — so they keep overlaying and
                        // only the rest fold into chips.
                        let (overlay, chipped): (Vec<_>, Vec<_>) = floats
                            .into_iter()
                            .partition(|f| self.is_pinned(hit.position, f.id));
                        let chips = chipped.into_iter().map(|f| f.id).collect();
                        if overlay.is_empty() {
                            floating::FloatSpec::Hidden(chips)
                        } else {
                            floating::FloatSpec::Mixed { chips, overlay }
                        }
                    };
                    (hit.position, spec)
                })
                .collect(),
        };

        // Offer the close glyph only when enabled *and* more than one tab is open,
        // so the last tab keeps no close target and can't be shut from under the
        // session (#86). Per tab it lands on the active tab — and, with perspective
        // off, on every tab — but not on inactive perspective tabs (that per-tab
        // gate lives in `paint::bar` and in the `close_layout` filter below, kept
        // identical so draw and hit-test never disagree). The glyph form follows
        // the terminal: the ASCII `×` under a simplified UI (no Nerd Font), the
        // Nerd Font glyph otherwise — chosen here, the one spot that knows the
        // font capability (#94).
        let close = if self.config.close_button && self.tabs.len() > 1 {
            // Resolve the glyph's foreground here, the one spot with both the
            // config and the live palette. `close_button_color` is applied
            // against each glyph's per-terminal default — the theme's alert red
            // for the Nerd Font glyph, black for the ASCII `×` — so `theme`
            // keeps the original look while `fg` / `red` / a hex override it,
            // immune to a theme whose red is a dark shade (#94 follow-up).
            let close_color = self.config.close_button_color;
            if self.simplified_ui {
                minimap::Close::Ascii(close_color.resolve(minimap::CLOSE_FG_ASCII))
            } else {
                minimap::Close::NerdFont(close_color.resolve(self.palette.alert()))
            }
        } else {
            minimap::Close::Off
        };

        // Suppressed-pane awareness markers, active tab only (#118, design §3). Empty
        // for every other tab, so only the tab you're on shows the cue.
        let suppressed_covers_by_position: BTreeMap<usize, Vec<usize>> = {
            let panes = self
                .panes
                .panes
                .get(&active_position)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let covers = suppressed::cover_ids(
                &projection::project_suppressed(panes),
                &projection::project(panes),
            );
            BTreeMap::from([(active_position, covers)])
        };

        // `close` already carries the per-terminal glyph form (#86, #94), so the
        // renderer stamps the right glyph, column, and color directly — no
        // post-render swap.
        print!(
            "{}",
            paint::bar(
                rows,
                &layout,
                &panes_by_position,
                &self.palette,
                &self.config.shortcut_prefix,
                self.config.gradient_spec(),
                self.config.inactive_dim,
                self.config.perspective,
                close,
                // Per-tab floating layer (#110): visible layers overlay, hidden
                // layers chip, keyed by tab position (built above from the live
                // manifest + each tab's `are_floating_panes_visible`).
                &floats_by_position,
                &suppressed_covers_by_position,
                // Pinned-float ids (#119), rebuilt from the live session-layout
                // dump on every `PaneUpdate` (see `refresh_pinned`).
                &self.pinned_by_tab,
            )
        );

        // Record the spans this frame drew so a later click hit-tests against
        // the current layout. `pack_with_button` re-runs every render, so this
        // is always the live geometry — never a cached copy. The button span
        // (when reserved) is recorded the same way, so a click routes to "open
        // a new tab" against the live frame (#76).
        self.button_layout = layout.button;
        self.tab_layout = layout.tabs;

        // Record per-tab pane geometry for the finer click-to-focus hit-test
        // (#74), parallel to `tab_layout`. Only the grid rungs (L0–L2) draw a
        // per-pane minimap; narrower tabs (L3 glyph / L4 hint) carry no pane
        // regions, so they get no entry and a click there falls back to plain
        // tab-switch. `vinset_for` mirrors what `assemble` painted, so the
        // hit-test insets exactly as the frame did.
        self.tab_panes = self
            .tab_layout
            .iter()
            .filter(|hit| {
                matches!(
                    tab_block::level_for(hit.width),
                    tab_block::Level::L0 | tab_block::Level::L1 | tab_block::Level::L2
                )
            })
            .map(|hit| {
                (
                    hit.position,
                    router::TabPaneGeom {
                        start: hit.start,
                        width: hit.width,
                        rows,
                        vinset: tab_block::vinset_for(self.config.perspective, rows, hit.active),
                        panes: panes_by_position
                            .get(&hit.position)
                            .cloned()
                            .unwrap_or_default(),
                        // Hidden-float chip ids for this tab (#110), taken from the
                        // float spec built above. A *hidden* layer draws every
                        // float as a chip; a *mixed* layer draws only its unpinned
                        // half as chips (#119), the rest overlaying instead. A
                        // visible/absent layer leaves this empty and the click
                        // falls back to the tiled pane under it.
                        hidden_floats: match floats_by_position.get(&hit.position) {
                            Some(floating::FloatSpec::Hidden(ids)) => ids.clone(),
                            Some(floating::FloatSpec::Mixed { chips, .. }) => chips.clone(),
                            _ => Vec::new(),
                        },
                        // Visible-float overlay rects for this tab (#110), from the
                        // same spec. A *visible* layer overlays every float; a
                        // *mixed* layer overlays only its pinned half (#119), which
                        // stays on the real screen while the rest of that hidden
                        // layer chips. A hidden/absent layer leaves this empty and
                        // the click routes to a chip or the tiled pane instead.
                        visible_floats: match floats_by_position.get(&hit.position) {
                            Some(floating::FloatSpec::Visible(rects)) => rects.clone(),
                            Some(floating::FloatSpec::Mixed { overlay, .. }) => overlay.clone(),
                            _ => Vec::new(),
                        },
                    },
                )
            })
            .collect();

        // Record the close cell for each tab that drew one (#86) — the grid rungs
        // (L0–L2) that `assemble` stamps the glyph on, gated by the same
        // `active || !perspective` predicate `paint::bar` paints with, and only
        // when `close` is on (enabled + >1 tab). The glyph rides the top text row
        // — the tabs that show it never recede — seated `close.right_offset()`
        // cells in from the block's right edge (both modes one cell in, leaving a
        // fill cell at the corner, #94), the same inset the renderer paints at, so
        // draw and hit-test never disagree.
        self.close_layout = if close.is_on() {
            self.tab_layout
                .iter()
                .filter(|hit| {
                    (hit.active || !self.config.perspective)
                        && matches!(
                            tab_block::level_for(hit.width),
                            tab_block::Level::L0 | tab_block::Level::L1 | tab_block::Level::L2
                        )
                })
                .map(|hit| line::CloseHit {
                    position: hit.position,
                    row: 0,
                    column: hit.start + hit.width - close.right_offset(),
                })
                .collect()
        } else {
            Vec::new()
        };
    }
}

impl State {
    /// The bar's fixed permission set — always exactly two: `ReadApplicationState`
    /// (Tab/Pane/Mode updates) and `ChangeApplicationState` (`switch_tab_to`,
    /// `focus_terminal_pane`, `close_tab_with_index`, and `new_tab` — behind
    /// click-to-switch #8, click-to-focus #74, close #86, and new-tab #76). Kept
    /// minimal so an existing v0.1.0 user who cached only these two never hits a
    /// permission cache miss on auto-update (zellij#4982). Pure and arg-free, so
    /// it is unit-tested directly (host imports are stubbed off-wasm, so what
    /// `load` requests is otherwise unobservable).
    fn permissions() -> Vec<PermissionType> {
        vec![
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]
    }

    /// Rebuild [`Self::pinned_by_tab`] from a per-tab session-layout dump
    /// (#119). A pin toggle arrives as a `PaneUpdate` with no `PaneInfo`
    /// field changed, so manifest-keyed caching would miss it — every
    /// float-bearing tab is re-dumped on each `PaneUpdate` instead. Tabs
    /// without floats are skipped (no dump), and `floating = off` skips
    /// everything: the bar depicts no floats, so pin data would be dead
    /// weight.
    fn refresh_pinned(&mut self) {
        if self.config.floating == floating::FloatingMode::Off {
            self.pinned_by_tab = BTreeMap::new();
            return;
        }
        self.pinned_by_tab = self
            .panes
            .panes
            .iter()
            .filter(|(_, panes)| panes.iter().any(projection::is_floating_terminal))
            .filter_map(|(&position, panes)| {
                let kdl = layout_dump_for_tab(position);
                let floats = projection::project_floating(panes);
                let ids = pinned::pinned_ids_for_tab(kdl.as_deref(), &floats)?;
                Some((position, ids))
            })
            .collect();
    }

    /// Dispatch a wheel event to the configured navigation: switch tabs, walk
    /// panes, or do nothing (#80, restored #108). zellij scroll events carry no
    /// position, so the gesture is bar-wide — it acts on the live tab/pane set,
    /// not a clicked spot. One event = one step: the rate-limiter #104 removed
    /// (#83/#96/#100) is gone for good (#108), so a stepless device's flick burst
    /// steps several at once — `off` mode is the opt-out. The pure `next_*`
    /// resolvers decide the target; this thin arm turns it into the host call.
    fn scroll(&self, dir: scroll::ScrollDir) {
        match self.config.scroll {
            scroll::ScrollMode::Off => {}
            scroll::ScrollMode::Tab => self.scroll_tabs(dir),
            scroll::ScrollMode::Pane => self.scroll_panes(dir),
        }
    }

    /// Switch one tab in `dir` from the active tab, wrapping at the ends. No-op
    /// when there is no active tab (a mid-transition snapshot).
    fn scroll_tabs(&self, dir: scroll::ScrollDir) {
        let Some(active) = projection::active_tab(&self.tabs).map(|tab| tab.position) else {
            return;
        };
        let Some(target) = scroll::next_tab(active, self.tabs.len(), dir) else {
            return;
        };
        // `next_tab` works in 0-based positions; `switch_tab_to` is 1-based.
        switch_tab_to((target + 1) as u32);
    }

    /// Move the focused pane one step in `dir` along the reading-order traversal
    /// of every tab's panes, crossing tab boundaries and wrapping globally (#80).
    /// Focusing is absolute (`focus_terminal_pane`), which both switches to the
    /// pane's tab and emits a session-state report — so the highlight follows.
    /// No-op when nothing tiled is focused.
    fn scroll_panes(&self, dir: scroll::ScrollDir) {
        let Some(current) = self.focused_pane_id() else {
            return;
        };
        let Some(target) = scroll::next_pane(&self.pane_focus_order(), current, dir) else {
            return;
        };
        // A visible tiled terminal pane is never hidden, so both float/in-place
        // flags are moot — pass `false`, exactly as the click-to-focus path (#74).
        focus_terminal_pane(target, false, false);
    }

    /// Whether float `id` in the tab at `position` is pinned (#119). Pinned
    /// floats stay on the real screen while their layer is hidden, so the
    /// wheel and the focus anchor treat them as visible.
    fn is_pinned(&self, position: usize, id: usize) -> bool {
        self.pinned_by_tab
            .get(&position)
            .is_some_and(|ids| ids.contains(&id))
    }

    /// The id of the focused terminal pane in the active tab that the wheel can
    /// step from in `pane` mode, if any — a tiled pane, or a floating one while
    /// its layer is visible or it is pinned. The floating case mirrors
    /// `pane_focus_order`, which appends visible and pinned floats: without it,
    /// wheeling from a focused float would find no anchor in the order and
    /// stall (#80/#110/#119).
    fn focused_pane_id(&self) -> Option<u32> {
        let active = projection::active_tab(&self.tabs)?;
        self.panes
            .panes
            .get(&active.position)?
            .iter()
            .filter(|pane| {
                projection::is_tiled_terminal(pane)
                    || (projection::is_floating_terminal(pane)
                        && (active.are_floating_panes_visible
                            || self.is_pinned(active.position, pane.id as usize)))
            })
            .find(|pane| pane.is_focused)
            .map(|pane| pane.id)
    }

    /// Every tab's panes flattened into one wheel-traversal order: tabs in
    /// ascending position, and within each tab its tiled panes in reading order
    /// followed by its floating panes that are on the real screen — all of them
    /// when the tab's floating layer is visible, only the pinned ones while it
    /// is hidden (#80, #110, #119).
    ///
    /// Tab order comes from the authoritative `self.tabs`, not the `PaneManifest`
    /// keys: `TabUpdate` and `PaneUpdate` arrive as separate events, so a just-
    /// closed tab can still linger as a stale position in the manifest. Walking
    /// `self.tabs` drops those, so the wheel never steps into a pane of a tab that
    /// no longer exists.
    ///
    /// A hidden *unpinned* float is deliberately excluded — it is reached only
    /// via its corner chip (#110), never the wheel, so wheeling never pops a
    /// hidden layer open (auto-hide-agnostic; the P0-3 spike left auto-hide
    /// unconfirmed). A pinned float stays on the real screen while its layer is
    /// hidden (#119), so it walks like a visible one.
    fn pane_focus_order(&self) -> Vec<u32> {
        let mut tabs: Vec<&TabInfo> = self.tabs.iter().collect();
        tabs.sort_by_key(|tab| tab.position);
        tabs.into_iter()
            .flat_map(|tab| {
                let panes = self.panes.panes.get(&tab.position);
                let mut order: Vec<u32> = panes
                    .map(|p| projection::pane_ids_in_reading_order(p))
                    .unwrap_or_default();
                if let Some(p) = panes {
                    order.extend(
                        projection::project_floating(p)
                            .into_iter()
                            .filter(|f| {
                                tab.are_floating_panes_visible || self.is_pinned(tab.position, f.id)
                            })
                            .map(|f| f.id as u32),
                    );
                }
                order
            })
            .collect()
    }
}

/// The KDL text of one tab's live session layout, via the host (#119) — or
/// `None` when the dump fails (the cue is additive, so a failed dump just
/// means "no pins this frame"). `dump_session_layout_for_tab` reads its reply
/// back from the plugin's stdin (rule #17), which panics off-wasm, so the
/// native-test build stubs the whole call; tests inject `pinned_by_tab`
/// directly instead.
#[cfg(not(test))]
fn layout_dump_for_tab(tab_index: usize) -> Option<String> {
    dump_session_layout_for_tab(tab_index)
        .ok()
        .map(|(kdl, _metadata)| kdl)
}

#[cfg(test)]
fn layout_dump_for_tab(_tab_index: usize) -> Option<String> {
    None
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
    use crate::line::TabHit;

    /// The number of host commands `body` emits, as a delta of this thread's
    /// stub counter — robust whether the harness gives each test its own
    /// thread (the default) or reuses one (`--test-threads=1`).
    fn host_commands_during(body: impl FnOnce()) -> usize {
        let before = HOST_COMMAND_CALLS.with(std::cell::Cell::get);
        body();
        HOST_COMMAND_CALLS.with(std::cell::Cell::get) - before
    }

    /// A `TabInfo` carrying only the position and stable id the tests read.
    fn tab(position: usize, tab_id: usize) -> TabInfo {
        TabInfo {
            position,
            tab_id,
            ..Default::default()
        }
    }

    /// A drawn span; `active` is irrelevant to hit-testing here, so it is `false`.
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
    fn permissions_are_the_two_base_grants() {
        // The bar requests exactly the v0.1.0 two-permission set —
        // `ReadApplicationState` (event subscription) and `ChangeApplicationState`
        // (click-to-switch / focus / close / new-tab). No `RunActionsAsUser`, so
        // an existing user who cached only Read+Change keeps working on
        // auto-update with no cache-miss freeze (zellij#4982).
        assert_eq!(
            State::permissions(),
            vec![
                PermissionType::ReadApplicationState,
                PermissionType::ChangeApplicationState,
            ]
        );
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

    /// A floating terminal pane with real geometry, as the manifest reports it.
    fn floating_pane(id: u32, x: usize, y: usize, w: usize, h: usize) -> PaneInfo {
        PaneInfo {
            id,
            is_floating: true,
            pane_x: x,
            pane_y: y,
            pane_columns: w,
            pane_rows: h,
            title: "f".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn load_parses_the_configuration_before_requesting_permissions() {
        // `load` must parse the delivered map before it drives the rest of the
        // plugin. Host imports are stubbed natively, so the observable contract
        // is the parsed config; what gets *requested* is pinned separately by the
        // `permissions_are_the_two_base_grants` test.
        let mut state = State::default();
        state.load(config_map(&[("active_width", "20")]));

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
    fn mode_update_records_the_simplified_ui_capability() {
        // The same event carries the terminal's font capability. zellij's
        // `arrow_fonts == true` means a simplified UI (no Nerd Font), so the
        // field mirrors it directly and drives the close-glyph fallback (#86).
        let mut state = State::default();
        assert!(!state.simplified_ui, "defaults to assuming a Nerd Font");

        let mut simplified = ModeInfo::default();
        simplified.capabilities.arrow_fonts = true;
        assert!(state.update(Event::ModeUpdate(simplified)));
        assert!(
            state.simplified_ui,
            "arrow_fonts=true downgrades to simplified"
        );

        let mut fancy = ModeInfo::default();
        fancy.capabilities.arrow_fonts = false;
        assert!(state.update(Event::ModeUpdate(fancy)));
        assert!(
            !state.simplified_ui,
            "arrow_fonts=false restores the Nerd Font path"
        );
    }

    #[test]
    fn left_click_on_a_tab_switches_and_defers_the_repaint() {
        // The click switches tabs (or focuses a pane) via the host (stubbed
        // here) and requests no repaint — the switch arrives back as a TabUpdate,
        // which drives the redraw.
        let mut state = State::default();
        state.tabs = vec![tab(0, 7), tab(1, 8), tab(2, 100), tab(3, 9), tab(4, 10)];
        state.tab_layout = five_block_layout();

        assert!(!state.update(Event::Mouse(Mouse::LeftClick(0, 9))));
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
        state.tab_panes = [(0usize, geom(0, 20, &[(7, 0, 0, 80, 24)]))]
            .into_iter()
            .collect();

        state.render(MIN_ROWS, 80);

        assert!(state.tab_layout.is_empty());
        assert!(
            state.tab_panes.is_empty(),
            "the bail-out wipes the pane geometry too"
        );
    }

    /// A `TabPaneGeom` for a block at `start` of `width` columns, holding panes
    /// `(id, x, y, w, h)`, at the minimum bar height with no perspective inset —
    /// the shape `render` records for a grid-rung tab (#74).
    fn geom(
        start: usize,
        width: usize,
        panes: &[(usize, u32, u32, u32, u32)],
    ) -> router::TabPaneGeom {
        router::TabPaneGeom {
            start,
            width,
            rows: MIN_ROWS,
            vinset: 0,
            panes: panes
                .iter()
                .map(|&(id, x, y, w, h)| minimap::PaneRect::new(id, x, y, w, h, "sh", false))
                .collect(),
            hidden_floats: Vec::new(),
            visible_floats: Vec::new(),
        }
    }

    #[test]
    fn left_click_dispatches_the_focus_pane_and_no_op_arms() {
        // Driving `update` over a frame with one grid-rung tab (block at cols
        // 10..30, pane 7): a click on the minimap dispatches the FocusPane arm
        // (`focus_terminal_pane`, a no-op host stub off-wasm), and a click off
        // every block resolves to NoOp — both request no repaint, the change (if
        // any) arriving back as a Tab/Pane update. Host effects are unobservable
        // natively, so the contract is that both arms dispatch without panicking,
        // complementing the tab-switch fallback covered above.
        let mut state = State::default();
        state.tab_layout = vec![hit_active(0, 10, 20)];
        state.tab_panes = [(0usize, geom(10, 20, &[(7, 0, 0, 80, 24)]))]
            .into_iter()
            .collect();

        assert!(
            !state.update(Event::Mouse(Mouse::LeftClick(1, 12))),
            "a click on pane 7 focuses it and defers the repaint"
        );
        assert!(
            !state.update(Event::Mouse(Mouse::LeftClick(1, 5))),
            "a click off every block is a no-op and defers the repaint"
        );
    }

    #[test]
    fn render_omits_narrow_tabs_from_the_click_geometry() {
        // Squeezed into 80 columns, many tabs collapse to L3/L4 rungs that draw
        // a glyph/hint rather than a per-pane minimap. The grid-rung filter
        // drops them, so they get no `tab_panes` entry and a click there falls
        // back to #8's plain tab-switch — never a wrong-pane focus (#74).
        let mut state = State::default();
        state.permitted = true;
        state.tabs = (0..24)
            .map(|i| TabInfo {
                active: i == 0,
                ..tab(i, i + 1)
            })
            .collect();

        state.render(MIN_ROWS, 80);

        let narrow: Vec<_> = state
            .tab_layout
            .iter()
            .filter(|h| {
                matches!(
                    tab_block::level_for(h.width),
                    tab_block::Level::L3 | tab_block::Level::L4
                )
            })
            .collect();
        assert!(
            !narrow.is_empty(),
            "24 tabs in 80 columns must squeeze some to L3/L4"
        );
        assert!(
            narrow
                .iter()
                .all(|h| !state.tab_panes.contains_key(&h.position)),
            "narrow (L3/L4) tabs carry no click geometry"
        );
    }

    /// An active [`TabHit`] at `position` spanning `start..start + width`.
    fn hit_active(position: usize, start: usize, width: usize) -> line::TabHit {
        line::TabHit {
            active: true,
            ..hit(position, start, width)
        }
    }

    #[test]
    fn render_records_pane_geometry_for_the_minimap() {
        // The success path records, per grid-rung tab, the geometry a finer click
        // hit-tests against (#74): the active tab's two panes survive into its
        // `tab_panes` entry, keyed by position, carrying the frame's row count.
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

        assert_eq!(
            state.tab_panes.get(&0).map(|g| g.panes.len()),
            Some(2),
            "the active tab's panes are recorded for click-to-focus"
        );
        assert_eq!(state.tab_panes.get(&0).map(|g| g.rows), Some(MIN_ROWS));
    }

    #[test]
    fn render_records_hidden_float_chips_for_a_tab_with_a_hidden_layer() {
        // A tab whose floating layer is hidden and holds two floats records their
        // ids as chips in the tab's geometry, so a later click can reveal+focus one.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: false,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24), // tiled
                PaneInfo {
                    id: 7,
                    is_floating: true,
                    ..Default::default()
                }, // hidden float
                PaneInfo {
                    id: 9,
                    is_floating: true,
                    ..Default::default()
                }, // hidden float
            ],
        );

        state.render(MIN_ROWS, 80);

        assert_eq!(
            state.tab_panes.get(&0).map(|g| g.hidden_floats.clone()),
            Some(vec![7, 9]),
            "hidden floats are recorded as chips for click-to-reveal",
        );
    }

    #[test]
    fn floating_off_records_no_chips() {
        // With `floating = off` the bar ignores floats entirely (pre-#110 look).
        let mut state = State::default();
        state.permitted = true;
        state.config = Config {
            floating: crate::floating::FloatingMode::Off,
            ..Default::default()
        };
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: false,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24),
                PaneInfo {
                    id: 7,
                    is_floating: true,
                    ..Default::default()
                },
            ],
        );

        state.render(MIN_ROWS, 80);
        assert_eq!(
            state.tab_panes.get(&0).map(|g| g.hidden_floats.len()),
            Some(0)
        );
    }

    #[test]
    fn left_click_on_a_chip_dispatches_a_floating_focus() {
        // A recorded chip click resolves to FocusFloatingPane and dispatches the
        // host effect (a no-op stub off-wasm) without panicking, deferring the
        // repaint. Bottom row = MIN_ROWS - 1 = 2; rightmost chip cell = col
        // 10 + 20 - 1 = 29.
        let mut state = State::default();
        state.tab_layout = vec![hit_active(0, 10, 20)];
        let mut g = geom(10, 20, &[]);
        g.hidden_floats = vec![7];
        state.tab_panes = [(0usize, g)].into_iter().collect();
        assert!(!state.update(Event::Mouse(Mouse::LeftClick(2, 29))));
    }

    #[test]
    fn render_records_the_button_span_only_when_enabled() {
        // With the button enabled (the default) a wide-enough frame reserves and
        // records its span, so a later click can route to "open a new tab".
        // Turning the toggle off records no span, leaving the click router with
        // nothing to match and reclaiming the columns for the tab strip.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            tab(1, 2),
        ];
        state
            .panes
            .panes
            .insert(0, vec![content_pane(0, 1, 80, 24)]);

        state.render(MIN_ROWS, 80);
        assert!(
            state.button_layout.is_some(),
            "the enabled button records its span on a wide frame"
        );

        state.config = Config {
            new_tab_button: false,
            ..Default::default()
        };
        state.render(MIN_ROWS, 80);
        assert!(
            state.button_layout.is_none(),
            "the disabled button records no span"
        );
    }

    #[test]
    fn render_records_close_cells_only_when_enabled_and_multi_tab() {
        // With `close_button` on and more than one tab, each grid-rung block
        // records a close cell at its top-right corner, so a later click can
        // route to "close this tab". Perspective is off here, so every tab carries
        // one (#86). Turning the toggle off — or dropping to a single tab —
        // records nothing, leaving the lone tab uncloseable.
        let mut state = State::default();
        state.permitted = true;
        state.config = Config {
            close_button: true,
            perspective: false,
            ..Default::default()
        };
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            tab(1, 2),
        ];

        state.render(MIN_ROWS, 80);
        assert_eq!(
            state.close_layout.len(),
            2,
            "with perspective off both wide tabs record a close cell when enabled"
        );
        assert!(
            state.close_layout.iter().all(|hit| hit.row == 0
                && state
                    .tab_layout
                    .iter()
                    .any(|t| t.position == hit.position && hit.column == t.start + t.width - 2)),
            "each close cell sits one cell in from its block's right edge \
             (leaving a fill cell at the corner, #94)"
        );

        state.tabs = vec![TabInfo {
            active: true,
            ..tab(0, 1)
        }];
        state.render(MIN_ROWS, 80);
        assert!(
            state.close_layout.is_empty(),
            "a lone tab records no close cell — it can never be closed"
        );

        state.config = Config {
            close_button: false,
            perspective: false,
            ..Default::default()
        };
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            tab(1, 2),
        ];
        state.render(MIN_ROWS, 80);
        assert!(
            state.close_layout.is_empty(),
            "the disabled close button records no cells"
        );
    }

    #[test]
    fn simplified_ui_insets_the_close_cell_one_column_from_the_right_edge() {
        // Under a simplified UI the ASCII "×" replaces the Nerd Font glyph but
        // sits at the same column — one cell in from the right edge
        // (`start + width - 2`), #94. The recorded geometry must follow the
        // painted column so a click still lands on the mark.
        let mut state = State::default();
        state.permitted = true;
        state.simplified_ui = true;
        state.config = Config {
            close_button: true,
            perspective: false,
            ..Default::default()
        };
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            tab(1, 2),
        ];

        state.render(MIN_ROWS, 80);
        assert_eq!(state.close_layout.len(), 2, "both wide tabs record a cell");
        assert!(
            state.close_layout.iter().all(|hit| hit.row == 0
                && state
                    .tab_layout
                    .iter()
                    .any(|t| t.position == hit.position && hit.column == t.start + t.width - 2)),
            "the ASCII close cell sits one cell in from the right edge"
        );
    }

    #[test]
    fn perspective_limits_the_close_cell_to_the_active_tab() {
        // With perspective on, inactive tabs recede (#66); a close glyph in their
        // inset corner reads as unbalanced, so only the active tab — which never
        // recedes — carries one (#86). The active cell rides the top row.
        let mut state = State::default();
        state.permitted = true;
        state.config = Config {
            close_button: true,
            perspective: true,
            ..Default::default()
        };
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            tab(1, 2),
        ];

        state.render(4, 80);

        let positions: std::collections::HashSet<usize> =
            state.close_layout.iter().map(|hit| hit.position).collect();
        assert_eq!(
            positions,
            std::collections::HashSet::from([0]),
            "only the active tab records a close cell under perspective"
        );
        assert!(
            state.close_layout.iter().all(|hit| hit.row == 0),
            "the active tab's close cell rides the top row"
        );
    }

    #[test]
    fn render_omits_narrow_tabs_from_the_close_geometry() {
        // The close cell rides the same grid rungs the glyph is painted on
        // (L0–L2). Tabs squeezed to L3/L4 draw no per-tab minimap and so get no
        // "×" — the filter must drop them from `close_layout`, mirroring how #74
        // drops them from the click geometry. Exercises the filter's reject arm.
        // Perspective is off so every wide tab records — isolating the L3/L4 size
        // filter from the active-only perspective gate.
        let mut state = State::default();
        state.permitted = true;
        state.config = Config {
            close_button: true,
            perspective: false,
            ..Default::default()
        };
        state.tabs = (0..24)
            .map(|i| TabInfo {
                active: i == 0,
                ..tab(i, i + 1)
            })
            .collect();

        state.render(MIN_ROWS, 80);

        let narrow: Vec<_> = state
            .tab_layout
            .iter()
            .filter(|h| {
                matches!(
                    tab_block::level_for(h.width),
                    tab_block::Level::L3 | tab_block::Level::L4
                )
            })
            .collect();
        assert!(
            !narrow.is_empty(),
            "24 tabs in 80 columns must squeeze some to L3/L4"
        );
        let closeable: std::collections::HashSet<usize> =
            state.close_layout.iter().map(|hit| hit.position).collect();
        assert!(
            narrow.iter().all(|h| !closeable.contains(&h.position)),
            "narrow (L3/L4) tabs carry no close cell"
        );
        assert!(
            !state.close_layout.is_empty(),
            "the wide tabs still record their close cells"
        );
    }

    #[test]
    fn left_click_on_the_close_cell_closes_the_tab_and_consumes_the_gesture() {
        // A press on a recorded "×" cell closes that tab via the host (stubbed
        // here) and requests no repaint — the close arrives back as a TabUpdate,
        // which drives the redraw. Checked before the focus/switch fallback so
        // the corner closes rather than switches (#86), and the close target is
        // consumed so a duplicate click can't re-dispatch off stale geometry.
        let mut state = State::default();
        state.close_layout = vec![line::CloseHit {
            position: 2,
            row: 0,
            column: 9,
        }];

        assert!(!state.update(Event::Mouse(Mouse::LeftClick(0, 9))));
        assert!(
            state.close_layout.is_empty(),
            "closing consumes the close target"
        );
    }

    #[test]
    fn render_clears_the_button_span_when_it_cannot_draw() {
        // A frame that bails out before drawing (here: not yet permitted) must
        // wipe the previous frame's button span too — otherwise a click could
        // route to a new tab against geometry no longer on screen.
        let mut state = State::default();
        state.button_layout = Some(line::ButtonHit {
            start: 40,
            width: 3,
        });

        state.render(MIN_ROWS, 80);

        assert!(state.button_layout.is_none());
    }

    fn focusable_pane(id: u32, x: usize, focused: bool) -> PaneInfo {
        PaneInfo {
            id,
            pane_x: x,
            pane_y: 1,
            pane_columns: 40,
            pane_rows: 24,
            is_focused: focused,
            title: "sh".to_string(),
            ..Default::default()
        }
    }

    /// Two tabs (positions 0 active, 1 inactive) whose tiled panes flatten to the
    /// reading-order traversal `[10, 20, 30]` — 10 (x=0) and 20 (x=40) in tab 0,
    /// then 30 in tab 1. `focused` marks which pane (if any) carries focus.
    fn scroll_state(mode: scroll::ScrollMode, focused: Option<u32>) -> State {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            0,
            vec![
                focusable_pane(10, 0, focused == Some(10)),
                focusable_pane(20, 40, focused == Some(20)),
            ],
        );
        manifest
            .panes
            .insert(1, vec![focusable_pane(30, 0, focused == Some(30))]);

        let mut state = State::default();
        state.config = Config {
            scroll: mode,
            ..Default::default()
        };
        state.tabs = vec![
            TabInfo {
                active: true,
                ..tab(0, 1)
            },
            TabInfo {
                active: false,
                ..tab(1, 2)
            },
        ];
        state.panes = manifest;
        state
    }

    #[test]
    fn focused_pane_id_resolves_the_active_tabs_focused_pane() {
        // The anchor is the focused pane of the *active* tab only — tab 1's pane
        // is never the answer even though it sits earlier in the manifest map.
        let state = scroll_state(scroll::ScrollMode::Pane, Some(20));
        assert_eq!(state.focused_pane_id(), Some(20));
    }

    #[test]
    fn focused_pane_id_is_none_without_a_focus() {
        // No focused pane in the active tab → no anchor, so `pane` mode leaves
        // focus untouched rather than guessing a target.
        let state = scroll_state(scroll::ScrollMode::Pane, None);
        assert_eq!(state.focused_pane_id(), None);
    }

    #[test]
    fn focused_pane_id_resolves_a_focused_visible_float() {
        // A focused floating pane anchors the wheel while its layer is visible —
        // `pane_focus_order` appends visible floats, so the anchor must recognize
        // one too, else wheeling from a focused float finds no anchor and stalls
        // (#80/#110).
        let mut state = scroll_state(scroll::ScrollMode::Pane, None);
        state.tabs[0].are_floating_panes_visible = true;
        if let Some(panes) = state.panes.panes.get_mut(&0) {
            panes.push(PaneInfo {
                id: 15,
                is_floating: true,
                is_focused: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        assert_eq!(state.focused_pane_id(), Some(15));
    }

    #[test]
    fn focused_pane_id_ignores_a_focused_float_while_the_layer_is_hidden() {
        // A hidden float is reached only via its chip, never the wheel, so it is
        // no anchor while the layer is hidden — matching `pane_focus_order`, which
        // excludes hidden floats (#110).
        let mut state = scroll_state(scroll::ScrollMode::Pane, None);
        // `are_floating_panes_visible` defaults to false (layer hidden).
        if let Some(panes) = state.panes.panes.get_mut(&0) {
            panes.push(PaneInfo {
                id: 15,
                is_floating: true,
                is_focused: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        assert_eq!(state.focused_pane_id(), None);
    }

    #[test]
    fn focused_pane_id_resolves_a_focused_hidden_pinned_float() {
        // Mirrors `focused_pane_id_ignores_a_focused_float_while_the_layer_is
        // _hidden`, but pinned: the float is on screen, so it anchors the
        // wheel even while its layer is hidden.
        let mut state = scroll_state(scroll::ScrollMode::Pane, None);
        if let Some(panes) = state.panes.panes.get_mut(&0) {
            panes.push(PaneInfo {
                id: 15,
                is_floating: true,
                is_focused: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![15usize])]);
        assert_eq!(state.focused_pane_id(), Some(15));
    }

    #[test]
    fn pane_focus_order_flattens_tabs_then_reading_order() {
        // Tabs in ascending position, panes in reading order within each: tab 0's
        // 10 (x=0) then 20 (x=40), then tab 1's 30 — the global wheel traversal.
        let state = scroll_state(scroll::ScrollMode::Pane, Some(10));
        assert_eq!(state.pane_focus_order(), vec![10, 20, 30]);
    }

    #[test]
    fn pane_focus_order_ignores_manifest_positions_not_in_the_tabs() {
        // `TabUpdate` and `PaneUpdate` arrive separately, so a just-closed tab can
        // linger as a stale position in the manifest. The traversal walks the
        // authoritative `self.tabs`, so that stale pane (99 at position 5) is
        // never visited — the wheel can't step into a tab that no longer exists.
        let mut state = scroll_state(scroll::ScrollMode::Pane, Some(10));
        state
            .panes
            .panes
            .insert(5, vec![focusable_pane(99, 0, false)]);
        assert_eq!(state.pane_focus_order(), vec![10, 20, 30]);
    }

    #[test]
    fn pane_focus_order_appends_visible_floats_but_not_hidden_ones() {
        // Tab 0 (floating layer VISIBLE) has tiled 10, 20 and a float 15; tab 1
        // (layer HIDDEN, the default) has tiled 30 and a float 25. The walk visits
        // each tab's tiled panes in reading order, then only a *visible* tab's
        // floats: [10, 20, 15, 30]. Float 25 is excluded — a hidden float is
        // reached via its chip (P2), never the wheel, so wheeling never pops the
        // layer open (#110; P0-3 unconfirmed → auto-hide-agnostic).
        let mut state = scroll_state(scroll::ScrollMode::Pane, Some(10));
        state.tabs[0].are_floating_panes_visible = true;
        if let Some(panes) = state.panes.panes.get_mut(&0) {
            panes.push(PaneInfo {
                id: 15,
                is_floating: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        if let Some(panes) = state.panes.panes.get_mut(&1) {
            panes.push(PaneInfo {
                id: 25,
                is_floating: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        assert_eq!(state.pane_focus_order(), vec![10, 20, 15, 30]);
    }

    #[test]
    fn pane_focus_order_includes_a_hidden_layers_pinned_float() {
        // Tab 1's layer is hidden but its float 25 is pinned — it is on the
        // real screen (#119), so the wheel walks it; the unpinned hidden
        // float case stays excluded (pinned_by_tab carries no entry for it).
        let mut state = scroll_state(scroll::ScrollMode::Pane, Some(10));
        if let Some(panes) = state.panes.panes.get_mut(&1) {
            panes.push(PaneInfo {
                id: 25,
                is_floating: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
        }
        state.pinned_by_tab = BTreeMap::from([(1usize, vec![25usize])]);
        assert_eq!(state.pane_focus_order(), vec![10, 20, 30, 25]);
    }

    #[test]
    fn pane_focus_order_skips_an_unpinned_float_beside_a_pinned_one() {
        // Tab 1's hidden layer holds floats 25 and 26 but only 26 is pinned —
        // the map entry gates per float id, not per tab, so 25 stays
        // chip-only while 26 joins the walk.
        let mut state = scroll_state(scroll::ScrollMode::Pane, Some(10));
        if let Some(panes) = state.panes.panes.get_mut(&1) {
            panes.push(PaneInfo {
                id: 25,
                is_floating: true,
                pane_x: 5,
                pane_y: 5,
                ..Default::default()
            });
            panes.push(PaneInfo {
                id: 26,
                is_floating: true,
                pane_x: 60,
                pane_y: 5,
                ..Default::default()
            });
        }
        state.pinned_by_tab = BTreeMap::from([(1usize, vec![26usize])]);
        assert_eq!(state.pane_focus_order(), vec![10, 20, 30, 26]);
    }

    #[test]
    fn scroll_dispatches_each_mode_without_panicking() {
        // Host effects (`switch_tab_to` / `focus_terminal_pane`) are no-op stubs
        // off-wasm, so the contract is that every mode dispatches both directions
        // over the live tab/pane set without panicking. One event = one step;
        // there is no cooldown to disable (#108).
        for mode in [
            scroll::ScrollMode::Tab,
            scroll::ScrollMode::Pane,
            scroll::ScrollMode::Off,
        ] {
            let state = scroll_state(mode, Some(20));
            state.scroll(scroll::ScrollDir::Forward);
            state.scroll(scroll::ScrollDir::Backward);
        }
    }

    #[test]
    fn scroll_guards_no_op_without_an_anchor() {
        // `tab` mode with no active tab, and `pane` mode with no focused pane,
        // both hit their guard returns and leave the session untouched (no host
        // effect) rather than guessing a target.
        let mut tabless = scroll_state(scroll::ScrollMode::Tab, None);
        tabless.tabs.iter_mut().for_each(|tab| tab.active = false);
        tabless.scroll(scroll::ScrollDir::Forward);

        let paneless = scroll_state(scroll::ScrollMode::Pane, None);
        paneless.scroll(scroll::ScrollDir::Backward);
    }

    #[test]
    fn scroll_up_and_down_dispatch_through_update() {
        // The wheel arms in `update()` route to `self.scroll`; host effects are
        // stubs off-wasm, so both directions dispatch without panicking and the
        // arm requests no repaint (the host effect's Tab/Pane update does).
        let mut state = scroll_state(scroll::ScrollMode::Tab, Some(10));
        assert!(!state.update(Event::Mouse(Mouse::ScrollUp(0))));
        assert!(!state.update(Event::Mouse(Mouse::ScrollDown(0))));
    }

    #[test]
    fn render_partitions_a_hidden_layer_by_pin_state() {
        // A hidden layer with floats 7 (pinned) and 9: the pinned one records
        // as an overlay rect (it is still on the real screen, #119) while only
        // the unpinned one chips.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: false,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24),
                floating_pane(7, 10, 5, 30, 10),
                floating_pane(9, 40, 8, 20, 6),
            ],
        );
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let geom = state.tab_panes.get(&0);
        assert_eq!(
            geom.map(|g| g.hidden_floats.clone()),
            Some(vec![9]),
            "only the unpinned float chips"
        );
        assert_eq!(
            geom.map(|g| g.visible_floats.iter().map(|f| f.id).collect::<Vec<_>>()),
            Some(vec![7]),
            "the pinned float stays an overlay while its layer is hidden"
        );
    }

    #[test]
    fn render_keeps_pin_state_scoped_to_its_own_tab() {
        // Two tabs, both with a hidden layer of floats; `pinned_by_tab` names
        // an id only in tab 0. Tab 1's floats must chip exactly as if no pins
        // existed anywhere — a pin entry for one tab must never leak into
        // another tab's partition.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![
            TabInfo {
                active: true,
                are_floating_panes_visible: false,
                ..tab(0, 1)
            },
            TabInfo {
                are_floating_panes_visible: false,
                ..tab(1, 2)
            },
        ];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24),
                floating_pane(7, 10, 5, 30, 10),
                floating_pane(9, 40, 8, 20, 6),
            ],
        );
        state.panes.panes.insert(
            1,
            vec![
                content_pane(0, 1, 80, 24),
                floating_pane(20, 10, 5, 30, 10),
                floating_pane(21, 40, 8, 20, 6),
            ],
        );
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let tab0 = state.tab_panes.get(&0);
        assert_eq!(
            tab0.map(|g| g.hidden_floats.clone()),
            Some(vec![9]),
            "tab 0's pin partitions as before"
        );
        assert_eq!(
            tab0.map(|g| g.visible_floats.iter().map(|f| f.id).collect::<Vec<_>>()),
            Some(vec![7])
        );

        let tab1 = state.tab_panes.get(&1);
        assert_eq!(
            tab1.map(|g| g.hidden_floats.clone()),
            Some(vec![20, 21]),
            "tab 1 has no pin entry, so every float chips"
        );
        assert_eq!(tab1.map(|g| g.visible_floats.len()), Some(0));
    }

    #[test]
    fn render_keeps_a_fully_pinned_hidden_layer_chipless() {
        // Every hidden float pinned → all overlay, no chips.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: false,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![content_pane(0, 1, 80, 24), floating_pane(7, 10, 5, 30, 10)],
        );
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let geom = state.tab_panes.get(&0);
        assert_eq!(geom.map(|g| g.hidden_floats.len()), Some(0));
        assert_eq!(geom.map(|g| g.visible_floats.len()), Some(1));
    }

    #[test]
    fn a_visible_layer_ignores_pin_state() {
        // Pin only changes hidden-layer classification: a visible layer keeps
        // every float in the overlay exactly as before.
        let mut state = State::default();
        state.permitted = true;
        state.tabs = vec![TabInfo {
            active: true,
            are_floating_panes_visible: true,
            ..tab(0, 1)
        }];
        state.panes.panes.insert(
            0,
            vec![
                content_pane(0, 1, 80, 24),
                floating_pane(7, 10, 5, 30, 10),
                floating_pane(9, 40, 8, 20, 6),
            ],
        );
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);

        state.render(MIN_ROWS, 80);

        let geom = state.tab_panes.get(&0);
        assert_eq!(geom.map(|g| g.hidden_floats.len()), Some(0));
        assert_eq!(geom.map(|g| g.visible_floats.len()), Some(2));
    }

    #[test]
    fn pane_update_refreshes_pinned_without_a_host_panic_off_wasm() {
        // The dump call is wasm-only (rule #17); the native seam returns None,
        // so a PaneUpdate with floats must neither panic nor invent pins —
        // and stale entries from a previous refresh are dropped.
        let mut state = State::default();
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            0,
            vec![content_pane(0, 1, 80, 24), floating_pane(7, 10, 5, 30, 10)],
        );

        assert!(state.update(Event::PaneUpdate(manifest)));
        assert!(
            state.pinned_by_tab.is_empty(),
            "the native seam yields no dumps, so no pins survive a refresh"
        );
    }

    #[test]
    fn floating_off_clears_the_pinned_map() {
        // With `floating = off` the bar depicts no floats — the refresh skips
        // every dump and drops stale pin data.
        let mut state = State::default();
        state.config = Config {
            floating: crate::floating::FloatingMode::Off,
            ..Default::default()
        };
        state.pinned_by_tab = BTreeMap::from([(0usize, vec![7usize])]);
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            0,
            vec![content_pane(0, 1, 80, 24), floating_pane(7, 10, 5, 30, 10)],
        );

        assert!(state.update(Event::PaneUpdate(manifest)));
        assert!(state.pinned_by_tab.is_empty());
    }
}
