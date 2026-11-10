// The web dashboard's theme variables (its shadcn tokens), under the names Kumi's shared views use.
export const T = {
  text: 'var(--color-foreground)',
  secondary: 'var(--color-card-foreground)',
  muted: 'var(--color-muted-foreground)',
  faint: 'var(--color-muted-foreground)',
  border: 'var(--color-border)',
  card: 'var(--color-card)',
  raised: 'var(--color-popover, var(--color-card))',
  chip: 'var(--color-muted)',
  accent: 'var(--color-primary)',
  good: 'var(--color-success)',
  warn: 'var(--color-warning)',
  bad: 'var(--color-destructive)',
  focus: 'var(--color-ring, var(--color-primary))'
} as const

export const CANVAS = {
  text: '--color-card-foreground',
  faint: '--color-muted-foreground',
  border: '--color-border',
  accent: '--color-primary',
  strong: '--color-foreground',
  ground: '--color-card'
} as const
