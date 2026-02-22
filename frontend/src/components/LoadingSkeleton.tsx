interface LoadingSkeletonProps {
  count?: number
  layout?: 'list' | 'grid-2' | 'grid-3'
}

export default function LoadingSkeleton({ count = 3, layout = 'list' }: LoadingSkeletonProps) {
  const gridClass =
    layout === 'grid-3'
      ? 'grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4'
      : layout === 'grid-2'
        ? 'grid grid-cols-1 md:grid-cols-2 gap-4'
        : 'space-y-4'

  return (
    <div className={gridClass}>
      {[...Array(count)].map((_, i) => (
        <div key={i} className="bg-dark-800 rounded-xl border border-dark-700 p-6 animate-pulse">
          <div className="h-6 bg-dark-700 rounded w-1/3 mb-2" />
          <div className="h-4 bg-dark-700 rounded w-1/2" />
        </div>
      ))}
    </div>
  )
}
