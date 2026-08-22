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
        {/*
          `href="/"` rather than `route('home')`. The typed helper is
          generated into `resources/js/generated/`, which is derived from the
          Rust graph and never committed, so importing it from the scaffold
          would leave a fresh clone with an unresolved import on line one.

          Switch to `route('home')` once the project is yours -- `just
          check-ts` runs `arc typegen` before `tsc`, so route names are
          checked against the live graph and renaming one in Rust turns every
          stale call site red. Arcature ADR 0006, at
          https://arcaturelabs.github.io/Arcature/decisions.html, records why it
          is not the default.
        */}
        <Link className="navbar-brand" href="/">
          {app_name}
        </Link>
      </nav>

      <main className="content">{children}</main>
    </>
  )
}
