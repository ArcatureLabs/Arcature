import { Head, Link } from '@inertiajs/react'

/**
 * 500 -- the server failed.
 *
 * Deliberately says nothing about why. In release builds the framework
 * redacts 5xx bodies, and this page must not become the one place a stack
 * trace leaks to a browser.
 */
export default function ServerError() {
  return (
    <div className="error-page">
      <Head title="Server error" />
      <p className="status">500</p>
      <h1>Something went wrong</h1>
      <p className="muted">The error has been logged. Try again in a moment.</p>
      <Link className="button" href="/">
        Go home
      </Link>
    </div>
  )
}
