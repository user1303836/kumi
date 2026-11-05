// Hermes Desktop's theme variables, so Kumi's views follow every theme. The dashboard build swaps this file for
// src/dashboard/theme.ts (the same names over the dashboard's variables).
export const T = {
  text: 'var(--ui-text-primary)',
  secondary: 'var(--ui-text-secondary)',
  muted: 'var(--ui-text-tertiary)',
  faint: 'var(--ui-text-quaternary)',
  border: 'var(--ui-stroke-secondary)',
  card: 'var(--ui-bg-card)',
  chip: 'var(--ui-bg-tertiary)',
  accent: 'var(--ui-accent)',
  good: 'var(--ui-green)',
  warn: 'var(--ui-yellow)',
  bad: 'var(--ui-red)'
} as const

/** The same colours as CSS variable names, for a canvas to resolve with getComputedStyle. */
export const CANVAS = {
  text: '--ui-text-secondary',
  faint: '--ui-text-quaternary',
  border: '--ui-stroke-secondary',
  accent: '--ui-accent',
  strong: '--ui-text-primary'
} as const
