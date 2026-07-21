import type { ReactNode } from 'react'
import { AppShell } from './AppShell'

export function NavBar() {
  // ponytail: kept for exports; AppShell owns navigation now
  return null
}

export function PageShell({
  title,
  subtitle,
  children,
  actions,
}: {
  title: string
  subtitle?: string
  children: ReactNode
  actions?: ReactNode
}) {
  return (
    <AppShell>
      <header className="config-topbar">
        <div>
          <span className="config-topbar__brand">mcp-adjutant</span>
          <span className="config-topbar__sep">/</span>
          <span className="config-topbar__page">{title}</span>
        </div>
        {actions}
      </header>
      <div className="config-canvas">
        {subtitle && <p className="config-canvas__subtitle">{subtitle}</p>}
        {children}
      </div>
    </AppShell>
  )
}
