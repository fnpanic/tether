# Tether design system

Tether follows an Apple-inspired visual language: quiet, generous white space, one accent colour,
precise typography, no decoration. This file adapts that language (independent analysis published at
getdesign.md, not affiliated with Apple) from marketing pages to a **desktop utility app**. Every
screen must follow it. When this file and the code disagree, fix the code.

No Apple trademarks, logos or product imagery are used anywhere.

## Principles

1. **One accent.** Action Blue is the only colour that means "you can act here". Nothing else is blue,
   and no other hue is used decoratively.
2. **Flat.** No gradients, no decorative shadows. Hierarchy comes from type, space and surface tone.
3. **Type does the work.** Large, tightly tracked semibold for the few things that matter (your ID,
   a device name); calm regular weight for everything else.
4. **Quiet chrome, loud content.** In a session the remote screen is the product; controls float on
   translucent dark glass and get out of the way.
5. **Press, don't bounce.** Interaction feedback is a 0.95 press scale and a crisp focus ring.

## Colour tokens

Defined once in `apps/desktop/src/styles.css` as CSS variables and exposed to Tailwind as
`bg-*/text-*/border-*` utilities. Never hard-code hex values in components.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `canvas` | `#ffffff` | `#000000` | Window background |
| `parchment` | `#f5f5f7` | `#1c1c1e` | Secondary surfaces: sidebars, inset wells, inputs |
| `tile` | `#ffffff` | `#272729` | Cards on canvas |
| `tile-2` | `#fafafc` | `#2a2a2c` | Pearl capsule fill, hover row |
| `ink` | `#1d1d1f` | `#f5f5f7` | Primary text |
| `ink-80` | `#333333` | `#cccccc` | Secondary text on buttons |
| `muted` | `#6e6e73` | `#98989d` | Captions, helper text |
| `faint` | `#7a7a7a` | `#6e6e73` | Disabled, fine print |
| `hairline` | `rgba(0,0,0,.08)` | `rgba(255,255,255,.10)` | 1px borders, dividers |
| `accent` | `#0066cc` | `#2997ff` | Buttons, links, selection, toggles |
| `focus` | `#0071e3` | `#2997ff` | 2px focus outline |

**Status colours** (extension; the source system has none). Only used as small dots, icons or short
inline text, never as fills for large areas or buttons:
`ok #1f9d55 / #30d158`, `warn #b25000 / #ffb340`, `danger #d70015 / #ff453a`.
Destructive buttons use `danger` text on a neutral capsule, not a red fill, except the in-session
**Disconnect** control, which is red so it can be found without reading.

Session surfaces always use the dark palette regardless of theme: stage `#000000`, glass
`rgba(39,39,41,.72)` with `backdrop-filter: saturate(180%) blur(20px)`.

## Typography

Font stack: `-apple-system, "SF Pro Text", "SF Pro Display", "Inter Variable", system-ui, sans-serif`.
macOS renders SF Pro; Windows uses bundled Inter with tightened tracking as the documented substitute.
IDs and passwords use `ui-monospace, "SF Mono", "JetBrains Mono", Consolas`.

Weights: **300, 400, 600, 700 only. Never 500** (`font-medium` is banned).

| Role | Size / line-height | Weight | Tracking | Tailwind |
| --- | --- | --- | --- | --- |
| display | 40 / 1.10 | 600 | -0.02em | `text-display` |
| title | 28 / 1.14 | 600 | -0.01em | `text-title` |
| headline | 21 / 1.19 | 600 | 0 | `text-headline` |
| body | 17 / 1.47 | 400 | -0.022em | `text-body` |
| body-strong | 17 / 1.24 | 600 | -0.022em | `text-body font-semibold` |
| callout | 15 / 1.40 | 400 | -0.01em | `text-callout` |
| caption | 14 / 1.43 | 400 | -0.016em | `text-caption` |
| caption-strong | 14 / 1.29 | 600 | -0.016em | `text-caption font-semibold` |
| footnote | 12 / 1.33 | 400 | -0.01em | `text-footnote` |

Desktop density: lists and dense controls use **caption (14)**; dialogs and explanatory copy use
**callout (15)**; primary reading text uses **body (17)**. Tracking tightens only at 17px and above
and is never applied below 12px.

## Space, size and radius

- Spacing scale (px): 4 · 8 · 12 · 16 · 24 · 32 · 48 · 80. Cards pad 24, dialogs 32, window
  gutters 32.
- Control heights: 32 (compact), 36 (default), 44 (primary CTA and inputs).
- Radius scale only: `5` chips · `8` utility buttons, thumbnails · `11` capsules, menus, inputs in
  dense forms · `18` cards, dialogs, panels · `full` primary CTAs, search, segmented controls,
  circular icon buttons. Nothing else.

## Elevation

- Default: **flat**. Cards are separated by a 1px `hairline` border and surface tone, never by shadow.
- Glass: sticky bars and the session toolbar use translucent surface plus backdrop blur.
- Floating layers (extension for an app: menus, popovers, dialogs) get one soft shadow,
  `0 12px 40px rgba(0,0,0,.18)`, the only shadow in the product. Nothing that sits in the layout gets it.

## Motion

- Press: `transform: scale(.95)` on every button, 120 ms ease-out.
- Focus: 2px solid `focus` outline, offset 2px, keyboard only (`:focus-visible`).
- Hover: tone change only (no lift, no glow).
- Enter: floating layers fade in 160 ms; panels rise 8px over 220 ms. Respect
  `prefers-reduced-motion` by disabling transforms.

## Components

| Component | Spec |
| --- | --- |
| **Primary button** | `accent` fill, white text, callout 15/400, pill, height 44 (36 compact), padding x 22. |
| **Secondary button** | Transparent, `accent` text, 1px `accent` border, pill. |
| **Utility button** | `ink` fill (`#f5f5f7` text) on light / `tile-2` on dark, caption 14, radius 8, height 32. |
| **Pearl capsule** | `tile-2` fill, `ink-80` text, 1px `hairline`, radius 11. Neutral actions (Cancel, Decline). |
| **Icon button** | Circular, 36×36 (44 for touch-first), `muted` icon → `ink` on hover, `parchment` hover fill. |
| **Text field** | `parchment` fill, 1px `hairline`, radius 11 (pill for search and the ID field), height 44, body text. Focus = `focus` ring. Error = `danger` 1px border + caption helper text below. |
| **Card** | `tile`, 1px `hairline`, radius 18, padding 24. No shadow. |
| **List row** | Height 36, radius 8, caption 14, hover `tile-2`, selected `accent` at 12% with `ink` text. |
| **Toggle** (extension) | 40×24 track, `accent` when on, `hairline`-toned `parchment` when off, white 20px knob. |
| **Segmented control** | Pill track in `parchment`, selected segment `tile` with hairline, caption text. |
| **Status pill** | `parchment` pill, 6px status dot + footnote text. |
| **Dialog** | `tile`, radius 18, padding 32, floating shadow, dim backdrop `rgba(0,0,0,.4)`. |
| **Session toolbar** | Glass pill (radius full), 44 tall, white icons at 72% → 100%, active item `rgba(255,255,255,.14)`. |

## Layout

- Main window content width caps at 980px, centred, 32px gutters.
- Two-column hero (This device / Remote device) collapses to one column under 834px.
- Recent devices: auto-fill grid, 200px minimum card width, 24px gaps.
- Title bar 44px, draggable; macOS keeps native traffic lights (left), Windows draws minimal
  controls (right).

## Writing

- Sentence case everywhere. Verbs on buttons ("Connect", "Accept", "End session").
- Errors say what happened and what to do next, in plain words. No codes in the primary text.
- Security-relevant text is explicit: who is connecting, what they can do, how to stop it.

## Don'ts

- No second accent, no gradients, no glows, no coloured backgrounds behind content.
- No shadows on cards, buttons or text.
- No `font-medium` (500).
- No radii outside the scale.
- Don't use the dark-surface blue (`#2997ff`) on light surfaces.
- Don't animate layout properties; transform and opacity only.
