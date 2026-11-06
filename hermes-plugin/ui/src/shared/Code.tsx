// A script's code as it will run (CC-19): every line, numbered, in a scrolling block. Serve and the plugin refuse
// code with control, bidi or zero-width characters (CC-19 r7, "Trojan Source"); as a second layer, any that got
// through is drawn where it is, as its code point, so the code can't read differently here than it runs.
import { T } from '@kumi/theme'
import type { ReactNode } from 'react'

import { FOCUSABLE, useT } from './ui.tsx'

// Control characters other than tab, bidi controls and marks, and zero-width or invisible ones.
const INVISIBLE = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u061C\u200B-\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/

/** One line with every invisible character replaced by a visible `U+XXXX` mark. */
export function visibleLine(line: string): ReactNode[] {
  const parts: ReactNode[] = []
  let text = ''
  for (const char of line) {
    if (!INVISIBLE.test(char)) {
      text += char
      continue
    }
    if (text) parts.push(text)
    text = ''
    const code = `U+${char.codePointAt(0)!.toString(16).toUpperCase().padStart(4, '0')}`
    parts.push(
      <mark data-invisible={code} key={parts.length} style={{ background: 'transparent', color: T.bad, border: `1px solid ${T.bad}`, borderRadius: 3, padding: '0 2px', fontSize: '0.85em' }}>
        {code}
      </mark>
    )
  }
  if (text || !parts.length) parts.push(text)
  return parts
}

export function CodeBlock({ code, label }: { code: string; label: string }) {
  const t = useT()
  const lines = code.split(/\r?\n/)
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop()
  return (
    // Focusable, so the keyboard can scroll it.
    <div
      aria-label={label}
      className={FOCUSABLE}
      role="region"
      style={{ maxHeight: '45vh', overflow: 'auto', border: `1px solid ${T.border}`, borderRadius: 4, background: T.card }}
      tabIndex={0}
      title={t('script.codeTitle')}
    >
      <pre style={{ margin: 0, padding: '6px 0', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace', fontSize: 11.5, lineHeight: 1.5, color: T.text }}>
        {lines.map((line, index) => (
          <div key={index} style={{ display: 'flex' }}>
            <span aria-hidden style={{ flex: 'none', width: '3.2em', paddingRight: 8, textAlign: 'right', color: T.faint, userSelect: 'none' }}>
              {index + 1}
            </span>
            <span style={{ whiteSpace: 'pre', paddingRight: 8 }}>{visibleLine(line)}</span>
          </div>
        ))}
      </pre>
    </div>
  )
}
