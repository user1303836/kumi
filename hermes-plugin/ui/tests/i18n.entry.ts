// For card.test.mjs: the shared views and the translator context in one bundle, so the context a test provides is the
// one the views read.
export { FocusView, HistoryView } from '../src/shared/HistoryView.tsx'
export { translator } from '../src/shared/i18n.ts'
export { I18n } from '../src/shared/ui.tsx'
