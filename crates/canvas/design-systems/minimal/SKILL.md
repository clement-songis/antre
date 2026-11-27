---
name: design-system-minimal
description: Neutral grayscale design system with one blue accent, for product UI, dashboards and landing pages. Use with design_create(design_system="minimal").
---

# Minimal design system

Use this system when the user picks "minimal" or asks for a clean, neutral
product look.

1. Read `DESIGN.md` in this folder before writing markup.
2. Create the design with `design_create` and `design_system: "minimal"`: it
   copies `tokens.css` into the design and links it from `index.html`.
3. Use only the CSS custom properties from `tokens.css` for colour, type,
   spacing, radius and shadow. Do not hard-code hex values or pixel sizes
   that have a token.
