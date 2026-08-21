import { Head, Link } from '@inertiajs/react'

/**
 * 419 -- the CSRF token did not match.
 *
 * In practice this means the session expired while a form sat open. Reloading
 * mints a fresh `XSRF-TOKEN` cookie, so the retry succeeds.
 */
export default function PageExpired() {
  return (
    <div className="error-page">
      <Head title="Page expired" />
      <p className="status">419</p>
      <h1>The page expired</h1>
      <p className="muted">Your session timed out. Reload and try again.</p>
      <Link className="button" href="/">
        Go home
      </Link>
    </div>
  )
}
