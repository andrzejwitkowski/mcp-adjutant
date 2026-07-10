import { emitUiNotify } from './uiLog'

export function demo() {
  emitUiNotify({
    headline: {
      component: 'demo',
      message: 'hello',
    },
    meta: { tags: ['ui'], correlationId: '1' },
  })
  emitUiNotify({
    headline: {
      component: 'demo',
      message: 'again',
    },
    meta: { tags: ['ui'], correlationId: null },
  })
}
