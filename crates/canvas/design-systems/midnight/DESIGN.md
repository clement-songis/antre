# Midnight

Dark by default, precise, a little luminous. Near-black layers, crisp
light text, a violet accent and subtle glows for emphasis.

## Principles

- Depth comes from layered surfaces (`--color-layer-0` … `--color-layer-3`),
  each one step lighter, not from heavy shadows.
- The accent is for the primary action, active states and data highlights.
  Use `--glow-accent` on at most one element per screen.
- Dense but breathable: 8px rhythm (`--space-*`), compact controls.
- Monospace for anything technical: code, IDs, keyboard shortcuts, metrics.

## Typography

- UI: `--font-sans`, body `--text-sm` or `--text-base`, `--leading-normal`.
- Headings: `--weight-semibold`, `--leading-tight`, tracking `-0.02em` above
  `--text-2xl`.
- Code and numbers: `--font-mono`, `font-variant-numeric: tabular-nums`.

## Colour

| Role | Token |
|---|---|
| App background | `--color-layer-0` |
| Panels, cards | `--color-layer-1` |
| Hover, inputs | `--color-layer-2` |
| Popovers | `--color-layer-3` |
| Text | `--color-text` |
| Secondary text | `--color-text-muted` |
| Borders | `--color-border` |
| Accent | `--color-accent` (text on it: `--color-on-accent`) |
| Status | `--color-success`, `--color-warning`, `--color-danger` |

Keep body text at 4.5:1 contrast or better against its layer.

## Components

- **Button**: height 32px, `--radius-md`, primary in accent with
  `--glow-accent` on hover, secondary in `--color-layer-2` with 1px border.
- **Card**: `--color-layer-1`, 1px `--color-border`, `--radius-lg`.
- **Code block**: `--color-layer-0`, `--font-mono` `--text-sm`, padding
  `--space-4`, `--radius-md`.
- **Badge**: `--text-xs`, `--radius-full`, accent at 15% opacity background.

## Markup

Put `data-mid="<stable-id>"` on every semantic block (nav, hero, panel,
card, row) so comments and selections survive edits.
