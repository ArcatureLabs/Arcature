// The default layout wrapping every Inertia page.

export default function DefaultLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <head>
        <title>__PROJECT_NAME__</title>
      </head>
      <body>
        <nav><a href="/">__PROJECT_NAME__</a></nav>
        <main>{children}</main>
      </body>
    </html>
  );
}
