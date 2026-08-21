/**
 * The props the default layout reads off the current page.
 *
 * Arcature has no server-pushed "shared props": every prop is declared on a
 * `#[page]` struct, so a page that uses the layout has to send `app_name`
 * itself. `arc typegen` writes the authoritative page types into
 * `resources/js/generated/`; this file only holds what the shell needs.
 */
export interface LayoutProps {
  app_name: string
  /**
   * Inertia constrains the type argument of `usePage<T>()` to its own
   * `PageProps`, which is an open bag (`[key: string]: unknown`). A closed
   * interface is not assignable to it, so this line is not looseness -- it is
   * the shape the adapter's type requires. The named props above still
   * type-check exactly; this only says "there may be more", which for a page
   * payload assembled on the server is simply true.
   */
  [key: string]: unknown
}
