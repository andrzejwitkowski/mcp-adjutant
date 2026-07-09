import type { ReactNode } from 'react'

const LINKS = [
  { hash: '#/', label: 'Configuration' },
  { hash: '#/evaluations', label: 'Evaluations' },
  { hash: '#/cache', label: 'Scout cache' },
] as const

export function NavBar() {
  const current = location.hash || '#/'

  return (
    <nav className="config-nav" aria-label="Config UI">
      {LINKS.map(({ hash, label }) => (
        <a
          key={hash}
          href={hash}
          className={current === hash ? 'is-active' : undefined}
        >
          {label}
        </a>
      ))}
    </nav>
  )
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
    <main className="config-app">
      <NavBar />
      <header className="config-app__header">
        <div className="config-app__header-row">
          <div>
            <h1>{title}</h1>
            {subtitle && <p>{subtitle}</p>}
          </div>
          {actions}
        </div>
      </header>
      {children}
    </main>
  )
}
