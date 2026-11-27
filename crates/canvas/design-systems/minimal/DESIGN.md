# Minimal

Quiet, functional, content first. Grayscale surfaces, a single blue accent
for actions and focus, generous whitespace.

## Principles

- One primary action per view, in `--color-accent`. Everything else is
  neutral.
- Hierarchy comes from size and weight, not colour.
- Align to the 4px spacing scale (`--space-*`). Prefer `--space-4` to
  `--space-8` between blocks.
- Borders before shadows. Use `--shadow-1` only for floating elements
  (menus, dialogs).

## Typography

- Font: `--font-sans` for everything, `--font-mono` for code and numbers in
  tables.
- Scale: `--text-xs` … `--text-4xl`. Body is `--text-base` at
  `--leading-normal`. Headings use `--weight-semibold` and
  `--leading-tight`.
- Line length 60–75 characters (`max-width: 65ch`).

## Colour

| Role | Token |
|---|---|
| Page background | `--color-bg` |
| Cards, panels | `--color-surface` |
| Default text | `--color-text` |
| Secondary text | `--color-text-muted` |
| Borders, dividers | `--color-border` |
| Primary action, links, focus ring | `--color-accent` |
| Text on accent | `--color-on-accent` |
| Success / warning / danger | `--color-success`, `--color-warning`, `--color-danger` |

Text contrast must stay at WCAG AA (4.5:1). Never put `--color-text-muted`
on `--color-surface-alt` for body copy.

## Components

- **Button**: height 36px, padding `0 var(--space-4)`, `--radius-md`,
  `--weight-medium`. Primary: accent background. Secondary: surface with
  border. Focus: `outline: 2px solid var(--color-accent); outline-offset: 2px`.
- **Card**: `--color-surface`, 1px `--color-border`, `--radius-lg`, padding
  `--space-6`.
- **Input**: height 36px, 1px border, `--radius-md`, focus ring as buttons.
- **Table**: no vertical rules, `--text-sm`, row height 40px, header in
  `--color-text-muted` uppercase `--text-xs`.

## Markup

Put `data-mid="<stable-id>"` on every semantic block (header, hero, card,
section, nav item) so comments and selections survive edits.
