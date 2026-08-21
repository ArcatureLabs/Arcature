import { Head, Link } from '@inertiajs/react'

export default function NotFound() {
  return (
    <div className="error-page">
      <Head title="Not found" />
      <p className="status">404</p>
      <h1>Nothing here</h1>
      <p className="muted">That address does not match any route.</p>
      <Link className="button" href="/">
        Go home
      </Link>
    </div>
  )
}
