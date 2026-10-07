---
name: rustic-themes
description: Design, edit and manage Rustic's UI themes (`*.rustic-theme.json`) — colors for dark and light mode, style recipes (shape, shadows, buttons, texture, density) and optional custom CSS. Use whenever the user asks to create, tweak, recolor, fix, list, export or remove a Rustic theme, or to make the app look a certain way.
---

# Rustic themes

Rustic's look comes from **theme packs**: one JSON file per theme, machine-wide. Built-in themes are read-only references; installed themes are editable files.

## Tools

- `list_themes` — every theme (id, name, builtin, modes, trusted, style) + the active theme and color mode.
- `read_theme` — a theme's file + its safety scan. `id: "template"` returns a complete starter file.
- `write_theme` — create (omit `id`) or replace an installed theme (`id`). Built-ins can't be changed: read one, edit, and save it **without** `id` (give it a new `name`).
- `delete_theme` — remove an installed theme (the user is always asked).

**Nothing you write applies by itself.** Every saved theme is untrusted until the user reviews it; Rustic shows them a "Review & apply" prompt. After `write_theme`, tell the user the theme is ready to review — never claim it is already applied. You can't change the active theme or the dark/light mode; the user does that in Settings › Appearance › Themes.

## Workflow

1. `list_themes` to see what exists and what's active.
2. Start from something real: `read_theme` the closest built-in (Basalt quiet neutral, Hearth warm, Petal pastel, Midnight deep blue, Onyx high-contrast, Neo Brutal bold, Grove organic, Neon Grid futuristic) or `read_theme("template")`.
3. Design both **dark** and **light** unless the user wants one mode.
4. `write_theme`. If the result lists BLOCKED items, fix them and write again; mention any warnings to the user.
5. Tell the user to review and trust it.

## File format (format 2)

```json
{
  "format": 2,
  "name": "My Theme",
  "author": "…", "version": "1.0.0", "description": "…",
  "dark":  { "bg": "#…", "fg": "#…", "primary": "#…", "…": "…" },
  "light": { "bg": "#…", "fg": "#…", "primary": "#…", "…": "…" },
  "style": { "radius": "0.5rem", "shadow": "soft", "button": "solid", "…": "…" },
  "css": ""
}
```

Colors are `#rrggbb` / `#rrggbbaa`, `rgb()/hsl()/oklch()`. Any slot you omit is inherited from Rustic's default for that mode — a theme can be just `bg`, `fg`, `primary`.

### Color slots and where they show

| Slot | Used for |
|---|---|
| `bg` | main app / editor background |
| `bg_hard` | terminal background (deepest surface) |
| `bg_soft` | subtle alternate surface |
| `bg1` | cards, sidebar, popovers, menus (one step of elevation) |
| `bg2` | hover, muted and secondary surfaces |
| `bg3` | selection / elevated chips |
| `bg4` | strongest neutral surface |
| `border` | dividers, inputs, outlines |
| `fg` / `fg1` | primary text |
| `fg2` | secondary text |
| `fg3` | muted text (labels, hints) — readable on `bg2` too |
| `fg4` | disabled text, faint UI |
| `primary` | primary buttons (button text is picked automatically: `bg` or `fg`, whichever reads better) |
| `accent` | focus rings, active sidebar items |
| `bright_red/green/yellow/blue` | danger / success / warning / info |
| `bright_purple` | special highlights, code keywords |
| `bright_orange` | highlights, code strings |
| `bright_aqua` | code types |
| `token_*` | syntax colors (keyword, string, comment, function, type, variable, number, operator, punctuation) |

### Design rules that make themes look designed, not generated

- **Build neutrals as one ramp**: pick one hue and a low chroma (0–0.035 in OKLCH) and step lightness evenly. Dark mode: `bg_hard` < `bg` < `bg_soft` < `bg1` < `bg2` < `bg3` < `bg4` with small steps (~0.01–0.05 L). Light mode: the same ladder going down from near-white; `bg1` (cards) is the lightest.
- **Tint everything the same way**: text, borders and surfaces share the neutral hue; pure grey looks flat, over-tinting looks muddy.
- **One accent**, used sparingly (`primary`, `accent`). Status colors share one lightness and chroma per mode so none shouts.
- **Contrast targets** (WCAG): `fg`/`bg` ≥ 12:1 (≥ 9.5:1 for deliberately low-contrast warm themes), `fg2` ≥ 7:1, `fg3` ≥ 4.5:1 against `bg2`, status colors ≥ 4.5:1 against `bg`, primary button text ≥ 4.5:1. The safety scan warns when `fg`/`bg` drops below 4.5:1.
- **Dark mode**: avoid pure `#000` backgrounds unless the theme is intentionally high-contrast; desaturate accents slightly and keep them light (OKLCH L ≈ 0.72–0.86).
- **Light mode**: accents get darker and a bit more saturated (L ≈ 0.45–0.56); avoid pure white text-on-accent with yellow/lime accents (use dark text — automatic).

## Style recipes (`style`)

| Key | Values | Effect |
|---|---|---|
| `radius` | e.g. `0px`, `0.5rem`, `1rem` | base corner radius |
| `font_sans` / `font_mono` | CSS font stacks | UI / code fonts (installed or bundled: Inter, Victor Mono) |
| `motion` | `full`, `reduced` | reduced disables animations |
| `border_width` | e.g. `1px`, `2px` | outlined controls + popups |
| `shadow` | `none`, `soft`, `strong`, `glow`, `hard` | popups / dialogs (`hard` = offset, no blur, outlined) |
| `surface` | `solid`, `glass` | frosted translucent popups and dialogs |
| `density` | `compact`, `normal`, `comfortable` | scales all spacing |
| `button` | `solid`, `pill`, `raised`, `brutal`, `glow` | primary / outline / secondary buttons |
| `texture` | `none`, `grain`, `grid`, `dots`, `scanlines` | faint overlay on the whole app |

Unknown values are rejected with the allowed list. Match recipes to the mood: bold → `radius: 0px`, `border_width: 2px`, `shadow: hard`, `button: brutal`; organic → large radius, `button: pill`, `texture: grain`, `density: comfortable`; futuristic → `surface: glass`, `shadow: glow`, `button: glow`, `texture: grid`.

## Custom CSS (`css`)

Use only when recipes can't express it. It's scoped so it never reaches protected dialogs (permission, trust, pairing prompts). Target shadcn slots: `[data-slot="button"][data-variant="default"]`, `[data-slot="dialog-content"]`, `[data-slot="popover-content"]`, `[data-slot="input"]`, etc.; use the theme's CSS variables (`var(--primary)`, `var(--background)`, `var(--foreground)`, `var(--border)`, `var(--radius)`).

- **Blocked** (the theme can't be trusted): `javascript:`/`vbscript:` URLs, `expression()`, `behavior`/`-moz-binding`, `data:` URLs other than images/fonts, `file:` URLs, anything targeting `[data-rustic-protected]`.
- **Warnings** shown to the user: remote `url()`/`@import` (prefer none — themes should work offline), hiding large parts of the UI, fixed full-screen overlays, very large stylesheets.

Keep CSS small and purposeful; never hide or cover controls.
