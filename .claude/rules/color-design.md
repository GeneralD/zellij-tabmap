# Color Design — When to Use the Gemini Advisor

When the user asks for color feedback, design critique, or help tuning
the minimap palette (blend fractions, contrast between active/inactive
states), invoke the **`color-design-advisor`** skill instead of reasoning
about hex values alone.

## Trigger phrases

- "色のフィードバックが欲しい" / "色の相談"
- "get color feedback" / "color advice" / "how does this look"
- "ACTIVE_UNFOCUSED_BLEND を調整したい" / "INACTIVE_LABEL_BLEND を変えたい"
- "Geminiに色を見てもらって"

## Why Gemini, not Claude

Gemini is multimodal and will actually *see* the rendered screenshot.
Claude reasons about color values abstractly. For visual judgment —
"is the contrast sufficient?" — Gemini's answer is more reliable.

## What the skill does

1. Builds `render_active_cue` (native target)
2. Takes a PNG screenshot via VHS
3. Reads the current blend constants from `src/minimap.rs`
4. Calls `gemini -p "tabmap-color-check: ..."` — triggers the
   `tabmap-color-advisor` Gemini skill
5. Returns structured suggestions: which constants to change and by how much

## After receiving Gemini's suggestions

Offer to apply them directly to `src/minimap.rs`. The constants are:

| Constant | File | Purpose |
|---|---|---|
| `ACTIVE_UNFOCUSED_BLEND` | `src/minimap.rs` | Blend % for unfocused panes in the active tab |
| `INACTIVE_LABEL_BLEND` | `src/minimap.rs` | Blend % for all panes in inactive tabs |
