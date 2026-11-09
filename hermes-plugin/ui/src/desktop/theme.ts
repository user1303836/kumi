// Hermes Desktop's theme variables, so Kumi's views follow every theme. The dashboard build swaps this file for
// src/dashboard/theme.ts (the same names over the dashboard's variables).
export const T = {
  text: 'var(--ui-text-primary)',
  secondary: 'var(--ui-text-secondary)',
  muted: 'var(--ui-text-tertiary)',
  faint: 'var(--ui-text-quaternary)',
  border: 'var(--ui-stroke-secondary)',
  card: 'var(--ui-bg-card)',
  // An opaque surface for what floats over other views (the Make panel over the map): the card colour is see-through.
  raised: 'var(--ui-bg-elevated)',
  chip: 'var(--ui-bg-tertiary)',
  accent: 'var(--ui-accent)',
  good: 'var(--ui-green)',
  warn: 'var(--ui-yellow)',
  bad: 'var(--ui-red)',
  // Desktop draws no focus rings of its own (styles.css), but its few that do use the theme's primary colour.
  focus: 'var(--ui-accent)'
} as const

/** The same colours as CSS variable names, for a canvas to resolve with getComputedStyle. */
export const CANVAS = {
  text: '--ui-text-secondary',
  faint: '--ui-text-quaternary',
  border: '--ui-stroke-secondary',
  accent: '--ui-accent',
  strong: '--ui-text-primary',
  // What the map's words are haloed in, so they read over the points.
  ground: '--ui-bg-elevated'
} as const
