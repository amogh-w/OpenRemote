<!-- Fetched from amogh-w/digitalgarden: src/site/styles/theme-config.md (private repo). Reference for the Catppuccin theming used in assets/index.html. -->
# Theme: Catppuccin (4 variants)

This isn't a hand-rolled theme — it's the [Catppuccin](https://catppuccin.com) palette
wired into mdBook's CSS custom properties (`src/catppuccin.scss`). Each of the four
flavors becomes a CSS class (`.latte`, `.frappe`, `.macchiato`, `.mocha`) that sets the
book's `--bg`, `--fg`, `--links`, etc. from that flavor's palette.

## Mapping: mdBook variable → Catppuccin color role

| mdBook variable | Catppuccin color | Role |
|---|---|---|
| `--bg` | `base` | Page background |
| `--fg` | `text` | Body text |
| `--sidebar-bg` | `mantle` | Sidebar background (one step darker than `base`) |
| `--sidebar-fg` | `text` | Sidebar text |
| `--sidebar-active` | `blue` | Active/current sidebar link |
| `--sidebar-non-existant` | `overlay0` | Sidebar entries with no page |
| `--sidebar-spacer` | `overlay0` | Sidebar divider lines |
| `--links` | `blue` | All hyperlinks |
| `--icons` / `--icons-hover` | `overlay0` / `overlay2` | Toolbar icons, resting/hover |
| `--inline-code-color` | `text` | Inline `code` text color |
| (code bg, via `code` rule) | `mantle` | Inline code background |
| `--quote-bg` | `mantle` | Blockquote background |
| `--quote-border` | `crust` | Blockquote left border |
| `--table-border-color` / `--table-header-bg` / `--table-alternate-bg` | `surface0` / `mantle` / `mantle` | Table styling |
| `--searchbar-bg` / `--searchbar-fg` / `--searchbar-border-color` | `mantle` / `text` / `surface0` | Search input |
| `--search-mark-bg` | `peach` | Highlighted search match |
| `--theme-popup-bg` / `--theme-popup-border` | `mantle` / `overlay0` | Theme-switcher dropdown |
| `--theme-hover` | `surface0` | Hover state in menus |
| `--sidebar-header-border-color` | `lavender` | Sidebar header underline |
| `--warning-border` | `peach` | Callout/warning border |
| `--color-scheme` | `light` for latte, `dark` for the other 3 | Native form control theming |
| `--blockquote-note-color` | `blue` | GitHub-style `> [!NOTE]` |
| `--blockquote-tip-color` | `green` | `> [!TIP]` |
| `--blockquote-important-color` | `mauve` | `> [!IMPORTANT]` |
| `--blockquote-warning-color` | `yellow` | `> [!WARNING]` |
| `--blockquote-caution-color` | `red` | `> [!CAUTION]` |

**Surface hierarchy** used throughout (Catppuccin's standard elevation model, darkest → lightest
in dark flavors, and inverted in latte): `crust` (deepest recess, e.g. blockquote border) →
`mantle` (sidebar, code bg, popups — "sunken" panels) → `base` (main page) → `surface0/1/2`
(raised elements, borders, hover states) → `overlay0/1/2` (muted/secondary content, icons,
gutters) → `text` (primary foreground).

## The 4 variants and their base palette

| Flavor | `base` (bg) | `mantle` | `crust` | `text` (fg) | `blue` (links) | Scheme |
|---|---|---|---|---|---|---|
| **Latte** | `#eff1f5` | `#e6e9ef` | `#dce0e8` | `#4c4f69` | `#1e66f5` | light |
| **Frappé** | `#303446` | `#292c3c` | `#232634` | `#c6d0f5` | `#8caaee` | dark |
| **Macchiato** | `#24273a` | `#1e2030` | `#181926` | `#cad3f5` | `#8aadf4` | dark |
| **Mocha** | `#1e1e2e` | `#181825` | `#11111b` | `#cdd6f4` | `#89b4fa` | dark |

Latte is the only light flavor (dark text on light bg); Frappé, Macchiato, and Mocha are
progressively darker/higher-contrast dark themes. Users pick a flavor via mdBook's theme
switcher, which just toggles which class (`latte`/`frappe`/`macchiato`/`mocha`) is applied
to `<html>`.

## Other roles used for accents (per flavor, not tabulated above)

- `overlay0/1/2` — muted UI chrome: scrollbar, code gutter text, disabled sidebar entries, icons
- `surface0/1/2` — borders and hover backgrounds (table borders, menu hover, blockquote-in-blockquote borders)
- `peach` — search-match highlight, warning border
- `lavender` — sidebar header border accent
- `pink` — active line in the Ace editor gutter
- `green` / `mauve` / `yellow` / `red` — semantic callout colors (tip/important/warning/caution)

Full hex values for every role/flavor are published at
[catppuccin.com/palette](https://catppuccin.com/palette) (this repo consumes them via the
`@catppuccin/palette` npm package rather than hardcoding them).

## Standalone CSS file

The fully compiled, resolved-to-hex CSS (all 4 flavor classes: `.latte`, `.frappe`,
`.macchiato`, `.mocha`) lives in **[`catppuccin-theme.css`](./catppuccin-theme.css)** next
to this file. It's dependency-free — drop it into any project and apply one of the flavor
classes to `<html>`/`<body>`/a container to theme it. This md file stays as the annotated
reference (what each variable means, the surface hierarchy, why colors are mapped the way
they are); the `.css` file is the drop-in artifact.

Not included in the CSS file: the `--copy-button-filter` values (SVG `filter()` chains used
to recolor the copy-code icon to match `subtext0`/`blue` per flavor) and the
syntax-highlighting rules pulled in via `@catppuccin/highlightjs` — those come from a
separate package rather than being defined in `src/catppuccin.scss`.
