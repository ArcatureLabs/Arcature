import { Link, usePage } from '@inertiajs/react'
import type { ReactNode } from 'react'

import type { LayoutProps } from '@/types'

/**
 * The application shell: navigation and the page body.
 *
 * Pages opt in by wrapping their own export, which keeps the layout mounted
 * across an Inertia visit -- remount it and any scroll position, focus, or
 * component state in the shell is lost on every navigation.
 */
export default function DefaultLayout({ children }: { children: ReactNode }) {
  const { app_name } = usePage<LayoutProps>().props

  return (
    <>
      <nav className="navbar">
        <Link className="navbar-brand" href="/">
          {app_name}
        </Link>
      </nav>

      <main className="content">{children}</main>
    </>
  )
}
