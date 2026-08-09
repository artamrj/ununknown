# Ununknown Design Language

> For agents designing or redesigning frontend UI. Read this before writing any CSS or JSX component.

---

## Philosophy

- **Dark-first, purple-accented.** The default theme is dark. Light mode exists but dark is canonical.
- **Zero external UI dependencies.** No shadcn, no Radix, no Tailwind. Every component is custom. Every icon is hand-drawn SVG.
- **Monolithic vanilla CSS** with CSS custom properties as design tokens. Single file: `frontend/src/styles/index.css`.
- **System fonts only.** No web font loads. Stack: `SF Pro Text, -apple-system, BlinkMacSystemFont, Segoe UI Variable, Segoe UI, sans-serif`.
- **Compact, information-dense.** Designed for metadata workflows. Small type (8-12px for labels), tight spacing, high density.

---

## Color Tokens

All colors live as CSS custom properties on `:root`. Reference these, never hardcode hex values.

### Surfaces

| Token | Value (dark) | Usage |
|---|---|---|
| `--app-bg` | `#0c0c11` | Page background (near-black, blue undertone) |
| `--surface-1` | `#111117` | Primary surfaces (panels, cards) |
| `--surface-2` | `#17171f` | Secondary surfaces (inputs, card bodies) |
| `--surface-3` | `#1e1e28` | Tertiary surfaces (hover states, badges) |
| `--surface-hover` | `#22222d` | Row/item hover |
| `--input` | `#0f0f15` | Input field background |

### Borders

| Token | Value | Usage |
|---|---|---|
| `--border` | `#292933` | Default borders |
| `--border-strong` | `#3a3947` | Emphasized borders (hover, focus) |

### Text

| Token | Value | Usage |
|---|---|---|
| `--text` | `#f2f0f7` | Primary text (near-white, lilac tint) |
| `--text-soft` | `#c8c4d2` | Secondary text |
| `--muted` | `#8d899a` | Labels, tertiary text |
| `--muted-2` | `#686574` | Icons, very muted text |

### Accent

| Token | Value | Usage |
|---|---|---|
| `--accent` | `#a970ff` | Primary accent (vivid purple) |
| `--accent-strong` | `#8b4df2` | Deeper purple (buttons, gradients) |
| `--accent-soft` | `rgba(169,112,255,0.12)` | Accent background tint |
| `--accent-border` | `rgba(169,112,255,0.32)` | Accent border tint |
| `--support` | `#ff738c` | Secondary accent (pink/coral) |

### Semantic Colors

| Token | Value | Usage |
|---|---|---|
| `--success` | `#68d8a0` | Success states |
| `--success-soft` | `rgba(104,216,160,0.11)` | Success background |
| `--warning` | `#f0bc68` | Warning states |
| `--warning-soft` | `rgba(240,188,104,0.12)` | Warning background |
| `--error` | `#ff7f8f` | Error states |
| `--error-soft` | `rgba(255,127,143,0.11)` | Error background |
| `--cyan` | `#66d9e8` | Processing status |

### Shadows

| Token | Value |
|---|---|
| `--shadow` | `0 20px 60px rgba(0,0,0,0.28)` |

### Light Theme Override

All tokens are redefined under `:root[data-theme="light"]`. Key shift: `--accent` becomes `#7f3fe3` (darker purple for contrast on light surfaces).

---

## Typography

### Font Stack

```css
font-family: SF Pro Text, -apple-system, BlinkMacSystemFont, Segoe UI Variable, Segoe UI, sans-serif;
```

### Monospace

```css
ui-monospace, SFMono-Regular, Menlo, monospace
```

### Scale

| Size | Usage |
|---|---|
| `8px` | Table headers, tiny labels |
| `9px` | Metadata details, score labels |
| `10px` | Badges, status pills, eyebrow secondary |
| `11px` | Eyebrow labels, nav counts, section labels |
| `12px` | Button text, input labels, form labels |
| `13px` | Body text, descriptions, toast body |
| `14px` | Default body, row titles |
| `15px` | Brand name, section headings |
| `16px` | Queue heading, candidate score |
| `20px` | Empty state heading |
| `22px` | Drawer heading, settings heading |

### Weights

| Weight | Usage |
|---|---|
| `400` | Body text, descriptions |
| `500` | Nav buttons, badge labels |
| `550` | Metadata values |
| `600` | Headings, row titles, button text |
| `700` | Eyebrow labels, score values |

### Tracking

- Headings: `-0.025em` to `-0.005em` (tightened)
- Uppercase labels: `0.01em` to `0.1em` (spaced out)
- Tabular numerals: `font-variant-numeric: tabular-nums` on numeric counters

---

## Spacing

No strict grid. Organic values, but consistent patterns:

| Pattern | Values |
|---|---|
| Micro gaps (icon-to-label) | `2px`-`5px` |
| Small gaps (pills, chips) | `6px`-`9px` |
| Medium gaps (list items, grid columns) | `10px`-`12px` |
| Section spacing | `14px`-`20px` |
| Page-level padding | `18px`-`30px` |
| Row horizontal padding | `18px` |
| Row vertical padding | `10px` (desktop), `7px` (mobile) |
| Inspector content padding | `24px 30px 52px` |
| Drawer section padding | `0 20px 30px` |

### Fixed Heights

| Element | Height |
|---|---|
| Topbar | `64px` |
| Queue header | `58px` |
| Mobile nav | `42px` |
| Processing dock | `86px` |
| Input fields | `42px` |
| Track row min-height | `78px` |

---

## Border Radius

### Tokens

```css
--radius-sm: 7px;
--radius-md: 10px;
--radius-lg: 14px;
```

### Usage Scale

| Radius | Elements |
|---|---|
| `4px` | Changed/keep tags |
| `5px` | Skeleton items, score badges |
| `6px` | Queue count, toast close, rank badge |
| `7px` | Buttons, artwork-small, provider pills |
| `8px` | Inputs, textareas, search field |
| `9px` | Cards, candidate row, quality strip |
| `999px` | Status pills, connection badge, toggle track, progress bar |
| `10px` | Artwork-medium, settings artwork |
| `13px` | Artwork-large |
| `14px` | Toast |
| `18px` | Empty state icon container |

**Pattern:** Interactive elements 7-9px. Artwork scales: 7 (small) -> 10 (medium) -> 13 (large). Pills use 999px.

---

## Buttons

### Hierarchy

| Class | Purpose | Style |
|---|---|---|
| `.primary-action` | Main CTA | Gradient purple, white text, 42px height, 9px radius, inset highlight + drop shadow. Hover: brightness(1.08) + translateY(-1px) |
| `.compact-button` | Secondary actions | Surface-2 bg, border, 38px height, 8px radius, 12px font |
| `.compact-button.accent` | Accent secondary | Accent-soft bg, accent border/text |
| `.icon-button` | Toolbar icons | 34x34px, transparent, 7px radius. Hover: border + surface-3 bg |
| `.accept-button` | Candidate accept | Accent-soft bg, accent border, 36px height, 8px radius. Hover: solid accent bg, white text |
| `.text-button` | Inline text actions | No border/bg, accent color, 11px font |
| `.remove-file-button` | Destructive action | Error-soft bg, error border/text |
| `.topbar-scan-action` | Topbar primary | Surface-2 bg, border-strong, 36px height. `.prominent` variant: solid accent bg |

### States

- **Disabled:** `opacity: 0.42; cursor: not-allowed` (global rule)
- **Focus visible:** `outline: 2px solid var(--accent); outline-offset: 2px`
- **Loading:** Spinner replaces icon, text updates (e.g., "Save" -> "Saving...")

### Spinner

```css
.spinner {
  width: 14px; height: 14px;
  border: 2px solid rgba(255,255,255,0.35);
  border-top-color: #fff;
  border-radius: 50%;
  animation: spin 700ms linear infinite;
}
```

---

## Inputs

```css
input, textarea {
  height: 42px;
  padding: 0 13px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--input);
}
```

**States:**
- Hover: `border-color: var(--border-strong)`
- Focus: `border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft)`
- Placeholder: `color: var(--muted-2)`

### Labels

```css
label {
  display: grid;
  gap: 6px;
  color: var(--muted);
  font-size: 12px;
  font-weight: 600;
}
```

---

## Status Pills

Pill-shaped badges with icon + label. All use `border-radius: 999px`, `font-size: 10px`, `font-weight: 600`.

| Class | Color |
|---|---|
| `.success` | Green text/border/bg |
| `.review` | Warning/orange text/border/bg |
| `.error` | Red text/border/bg |
| `.processing` | Cyan text/border/bg |
| `.muted` | Gray text/border/bg |

---

## Cards

### Candidate Row

- Border: `1px solid var(--border)`, `border-radius: 11px`
- Background: `color-mix(in srgb, var(--surface-2) 70%, transparent)`
- Grid: `grid-template-columns: 24px 58px minmax(155px, 1fr) 102px auto`
- Recommended: green left-border inset + green-tinted border
- Hover: border strengthens, background becomes opaque

### Decision Note (Alert Card)

- `border-radius: 10px`, `padding: 14px 15px`
- `.review` variant (warning) and `.problem` variant (error)
- Contains icon + bold title + description + optional expandable technical details

### Quality Strip

- 3-column info grid with `gap: 1px` and border-based cell separation
- Each cell: icon + label + value

---

## Layout

### Overall Structure

```
.studio-app
  .topbar          (64px fixed, 3-column grid)
  .mobile-nav      (42px, hidden > 920px)
  .workspace       (fills viewport, 2-column grid)
    .queue-panel   (left, scrollable)
    .inspector     (right, max-width 900px)
  .processing-dock (86px fixed, 3-column grid)
  [SettingsDrawer] (conditional overlay)
```

### Topbar Grid

```css
grid-template-columns: minmax(370px, 1fr) auto minmax(190px, 1fr);
```

### Workspace Grid

```css
grid-template-columns: minmax(520px, 0.9fr) minmax(560px, 1.1fr);
```

### Track Row Grid

```css
grid-template-columns: 52px minmax(145px, 1fr) minmax(95px, 0.72fr) 108px 18px;
```

### Glassmorphism

Topbar and dock use `backdrop-filter: blur()` with semi-transparent backgrounds via `color-mix(in srgb, ...)`.

---

## Responsive Breakpoints

| Breakpoint | Behavior |
|---|---|
| `> 1180px` | Full desktop: 2-panel workspace, 3-column topbar, full grid |
| `≤ 1180px` | Compressed topbar, connection state hidden, file column drops |
| `≤ 920px` | Mobile: single column, inspector becomes full-screen overlay, mobile nav appears |
| `≤ 650px` | Small mobile: icon-only buttons, smaller artwork (38px), 1-column editor grids |

### Mobile Inspector

At `≤ 920px`, the inspector becomes a fixed full-height overlay with `drawer-in` animation. Close button floats.

---

## Icons

Custom SVG system via `<Icon name="..." size={18} />` in `src/app/Icons.tsx`.

### 26 Icons

album, alert, arrow, check, chevron, disc, edit, folder, info, layers, menu, moon, more, music, pause, play, refresh, search, settings, shield, skip, sparkles, sun, trash, waveform, x

### Styling

- ViewBox: `24x24`
- Style: stroke-based (`fill="none"`, `stroke="currentColor"`, `strokeWidth="1.8"`, `strokeLinecap="round"`, `strokeLinejoin="round"`)
- Size: configurable via `size` prop (default 18px)
- Color: inherits from parent via `color` CSS property
- Accessibility: `aria-hidden="true"` on all icons

---

## Animations

### Transitions

| Speed | Duration | Usage |
|---|---|---|
| Fast | `130ms ease` | Hover transitions, input focus, row hover |
| Default | `140ms ease` | Nav buttons, scan action |
| Toggle | `150ms ease` | Toggle knob movement |
| Toast in | `180ms ease-out` | Toast entrance (opacity + translateY) |
| Drawer in | `180ms ease-out` | Drawer slide from right |
| Progress | `300ms ease` | Progress bar width |

### Keyframes

```css
@keyframes spin { to { rotate: 360deg } }
@keyframes shimmer { to { background-position: -220% 0 } }
@keyframes equalize { /* height oscillation for dock bars */ }
@keyframes toast-in { from { opacity:0; translateY:-5px } }
@keyframes drawer-in { from { opacity:0; translateX(25px) } }
```

### Micro-interactions

- Primary button hover: `filter: brightness(1.08)` + `translateY(-1px)` (lift)
- Primary button active: `translateY(0)` (press)
- Candidate row hover: border color + background transition
- Chevron rotation: `rotate(90deg)` on details open

### Reduced Motion

Full `@media (prefers-reduced-motion: reduce)` support. Kills all animations and transitions.

---

## Interactive States

### Buttons

- Default: `color: var(--muted)`, transparent bg, no border, 7px radius
- Hover: `border-color: var(--border)`, `color: var(--text-soft)`, `background: var(--surface-3)`
- Focus: `outline: 2px solid var(--accent)`, `outline-offset: 2px`
- Disabled: `opacity: 0.42`, `cursor: not-allowed`

### Track Rows

- Default: transparent bg, `border-bottom: 1px solid var(--border)`
- Hover: `background: var(--surface-hover)`
- Selected: purple gradient bg + `inset 3px 0 var(--accent)` left border

### Toggle Switches

- Off: `background: var(--border-strong)`, knob left, color `var(--muted)`
- On (automation): accent colors, knob slides `translateX(12px)`
- On (danger): error colors

---

## Modals / Drawers

**No center modals.** Destructive confirmations use native `window.confirm()`.

### Settings Drawer

- Right-sliding panel: `width: min(480px, 100vw); height: 100%`
- Backdrop: `rgba(4, 4, 7, 0.58)` + `backdrop-filter: blur(3px)`
- Animation: `drawer-in 180ms ease-out`
- z-index: 50

---

## Toasts

- Fixed position: `top: calc(var(--topbar) + 12px); right: 18px`
- z-index: 30
- Grid: `auto 1fr auto` (icon, text, dismiss)
- Width: `min(380px, calc(100vw - 36px))`
- Variants: `.error-toast` (red), `.notice-toast` (green)
- Auto-dismiss: 5 seconds for notices

---

## Empty States

Reusable centered layout:
1. 60x60px icon container (border + rounded + accent gradient bg)
2. `<h2>` title (20px, letter-spacing -0.025em)
3. `<p>` description (13px, muted, max-width 380px)
4. Optional action button (compact-button)

Context-sensitive: different icon/text for offline, processing, complete, idle, filtered-empty.

---

## Loading States

### Skeleton

5 placeholder rows with shimmer animation:
```css
background: linear-gradient(90deg, var(--surface-2), var(--surface-3), var(--surface-2));
background-size: 220% 100%;
animation: shimmer 1.5s infinite;
```

### Progress Bar

Native `<progress>` with gradient fill:
```css
background: linear-gradient(90deg, var(--accent), var(--support));
height: 6px;
```

### Equalizer

Three animated bars in the processing dock when busy.

---

## Image Handling

### Artwork Sizes

| Class | Size | Radius |
|---|---|---|
| `.artwork-small` | 52x52px | 7px |
| `.artwork-medium` | 64x64px | 9px |
| `.artwork-large` | 136x136px | 13px + drop shadow |

### Styling

- `object-fit: cover`
- `border: 1px solid rgba(255,255,255,0.06)`
- `background: var(--surface-3)`
- `loading="lazy"`

### Missing Artwork

Gradient bg with music note icon:
```css
.artwork-missing {
  display: grid;
  place-items: center;
  background: linear-gradient(145deg, var(--surface-3), var(--surface-2));
}
```

---

## Accessibility

- Focus-visible outlines: `2px solid var(--accent)` on all interactive elements
- `aria-label` on icon buttons
- `aria-current="page"` on active nav items
- `role="listitem"` on track rows
- `role="alert"` on errors, `role="status"` on success feedback
- `aria-modal="true"` on drawer dialog
- `.sr-only` utility class for screen-reader-only content
- `prefers-reduced-motion` support disables all animations

---

## Dark/Light Mode

- Toggle via `data-theme` attribute on `<html>`
- Default: dark. Light: `data-theme="light"`
- Persisted to `localStorage` key `"ununknown-theme"`
- System preference detection: `window.matchMedia("(prefers-color-scheme: light)")`
- `color-scheme: dark` / `color-scheme: light` affects native form controls
- Toggle UI: sun/moon icon button in topbar
- Transition: instant (no animation on theme switch)

---

## Rules for New Components

1. **Never hardcode colors.** Always use CSS custom properties (`var(--accent)`, `var(--surface-2)`, etc.).
2. **Never import external UI libraries.** Build from scratch using the existing token system.
3. **Follow the button hierarchy.** Use `.primary-action` for main CTA, `.compact-button` for secondary, `.icon-button` for toolbar.
4. **Use existing border-radius tokens.** Reference `--radius-sm/md/lg` or the established scale.
5. **Match transition speeds.** 130-140ms ease for most interactions, 180ms ease-out for enter animations.
6. **Include focus-visible states.** All interactive elements need `outline: 2px solid var(--accent); outline-offset: 2px`.
7. **Include reduced-motion fallback.** Wrap animations in `@media (prefers-reduced-motion: no-preference)`.
8. **Use semantic HTML.** `<button>` for actions, `<nav>` for navigation, `<aside>` for panels, `role` attributes where appropriate.
9. **Keep density tight.** This is a metadata tool, not a marketing site. Information density is a feature.
10. **Test at all breakpoints.** Components must work at 650px, 920px, 1180px, and full desktop.
11. **Use existing icon names.** Check `src/app/Icons.tsx` before creating new icons. If you need a new icon, add it to the `Icon` component's switch statement.
12. **Support both themes.** Every new component must work in both dark and light mode. Use the token system and it happens automatically.
