interface PagerProps {
  page: number
  totalPages: number
  loading: boolean
  label: string
  onPageChange: (page: number) => void
}

export function Pager({ page, totalPages, loading, label, onPageChange }: PagerProps) {
  if (totalPages <= 1) return null

  return (
    <nav className="pager" aria-label={label}>
      <button
        type="button"
        className="config-btn"
        disabled={page <= 1 || loading}
        onClick={() => onPageChange(Math.max(1, page - 1))}
      >
        Previous
      </button>
      <span className="pager__status">
        Page {page} of {totalPages}
      </span>
      <button
        type="button"
        className="config-btn"
        disabled={page >= totalPages || loading}
        onClick={() => onPageChange(page + 1)}
      >
        Next
      </button>
    </nav>
  )
}
