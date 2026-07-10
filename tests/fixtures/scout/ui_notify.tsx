import { emitUiNotify } from './uiLog'

export function demo() {
subject: {
    headline: {
      component: 'demo',
      message: 'hello',
    },
    meta: { tags: ['ui'], correlationId: '1' },
  })
subject: {
    headline: {
      component: 'demo',
      message: 'again',
    },
    meta: { tags: ['ui'], correlationId: null },
  })
}
