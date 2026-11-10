// Colours as numbers, for what the atlas paints itself (a canvas takes no CSS variable, and a glow needs its colour's
// channels): the map's group colours (`rgb(r, g, b)` or `#rrggbb`, as `groupColor` writes them) and the theme's, once
// the canvas has resolved them.

export type Rgb = readonly [number, number, number]

/** A colour's channels (0–255): `#rgb`, `#rrggbb`, `rgb(r, g, b)` or `rgba(r, g, b, a)`; null for anything else. */
export function rgbOf(color: string): Rgb | null {
  const text = color.trim()
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(text)
  if (hex) {
    const digits = hex[1].length === 3 ? [...hex[1]].map(digit => digit + digit).join('') : hex[1]
    const value = parseInt(digits, 16)
    return [value >> 16, (value >> 8) & 255, value & 255]
  }
  const rgb = /^rgba?\(\s*(\d{1,3})[\s,]+(\d{1,3})[\s,]+(\d{1,3})\s*(?:[,/]\s*[\d.]+%?\s*)?\)$/i.exec(text)
  if (!rgb) return null
  const channels = [Number(rgb[1]), Number(rgb[2]), Number(rgb[3])] as const
  return channels.every(channel => channel <= 255) ? channels : null
}

export function rgba(color: Rgb, alpha: number): string {
  return `rgba(${color[0]}, ${color[1]}, ${color[2]}, ${alpha})`
}

/** The colour moved `amount` (0–1) of the way to white: a sound's own shape, bright over its family's glow. */
export function lighten(color: Rgb, amount: number): string {
  const toward = (channel: number): number => Math.round(channel + (255 - channel) * amount)
  return `rgb(${toward(color[0])}, ${toward(color[1])}, ${toward(color[2])})`
}

/** Whether a ground of this colour is a dark one (its glows add light; on a light ground shapes keep their colour). */
export function isDark(color: Rgb): boolean {
  return 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2] < 128
}
