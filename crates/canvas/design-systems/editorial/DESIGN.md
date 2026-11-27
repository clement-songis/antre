# Editorial

Reading first. Paper-toned background, serif display type, a sans for UI
labels, a terracotta accent used sparingly.

## Principles

- Typography carries the design: big serif headlines, comfortable body
  measure, real hierarchy (kicker, headline, standfirst, body).
- Thin rules (`--rule`) separate sections instead of boxes.
- Asymmetric grids are welcome: a 12-column grid with content on 7–8
  columns and notes or images in the margin.
- The accent appears at most twice per screen (a kicker, a link).

## Typography

- Display and headings: `--font-serif`, `--weight-regular` or
  `--weight-semibold`, `--leading-tight`, slight negative tracking
  (`letter-spacing: -0.01em`).
- Body: `--font-serif` at `--text-lg`, `--leading-relaxed`, `max-width: 68ch`.
- UI, captions, kickers: `--font-sans`, `--text-sm`, uppercase with
  `letter-spacing: 0.08em` for kickers.
- Scale: `--text-sm` … `--text-6xl`.

## Colour

| Role | Token |
|---|---|
| Page | `--color-paper` |
| Inset blocks, pull quotes | `--color-paper-alt` |
| Text | `--color-ink` |
| Captions, metadata | `--color-ink-muted` |
| Rules | `--color-rule` |
| Accent (kickers, links) | `--color-accent` |

## Components

- **Pull quote**: `--font-serif` `--text-3xl` italic, left rule 3px in
  `--color-accent`, padding-left `--space-6`.
- **Figure**: full-bleed or column width, caption in `--font-sans`
  `--text-sm` `--color-ink-muted`.
- **Button**: text button with underline offset 4px, or outlined with 1px
  `--color-ink`, square corners (`--radius-none`).
- **Drop cap**: first letter `--text-6xl`, float left, `--color-accent`.

## Markup

Put `data-mid="<stable-id>"` on every semantic block (header, article,
section, figure, aside) so comments and selections survive edits.
