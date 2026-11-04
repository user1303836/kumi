// `react/jsx-runtime` in the dashboard bundle, over the dashboard's React: JSX compiles to these calls.
import type { ElementType, ReactElement } from 'react'

import React from './react.ts'

type Props = Record<string, unknown> | null

export const Fragment = React.Fragment

export function jsx(type: ElementType, props: Props, key?: string): ReactElement {
  return React.createElement(type, key === undefined ? props : { ...props, key })
}

export const jsxs = jsx
