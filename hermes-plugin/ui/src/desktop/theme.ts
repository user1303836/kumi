// Hermes Desktop's theme variables, so Kumi's views follow every theme. The dashboard build swaps this file for
// src/dashboard/theme.ts (the same names over the dashboard's variables).
import * as sdk from '@hermes/plugin-sdk'

export const T = {
  text: 'var(--ui-text-primary)',
  secondary: 'var(--ui-text-secondary)',
  muted: 'var(--ui-text-tertiary)',
  faint: 'var(--ui-text-quaternary)',
  border: 'var(--ui-stroke-secondary)',
  // Hermes's in-panel divider (DESIGN.md: "flat, not boxed"): what separates rows and sections inside one surface.
  hairline: 'var(--ui-stroke-tertiary)',
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

/** Something that changes whenever Desktop's theme does (its theme hook, v0.23 and later), for a canvas that paints the
 *  theme's colours itself; null on a Desktop that can't say, where the canvas looks at its colours once a second. */
export const useThemeKey: () => unknown = typeof sdk.useTheme === 'function' ? sdk.useTheme : () => null
