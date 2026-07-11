import type { UiNotifyEvent } from './types'

/** ponytail: console fixture for transformer E2E — no toast library */
export function emitUiNotify(event: UiNotifyEvent) {
  const cid = event.meta.correlationId ? ` cid=${event.meta.correlationId}` : ''
  console.debug(
    `[config-ui][${event.subject.component}@${event.meta.sourceModule}] ${event.subject.summary}${cid}`,
  )
}
