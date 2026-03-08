import PhantomScreenDemo from './phantom-screen-demo';

export default function Page() {
  return (
    <main
      style={{
        minHeight: '100vh',
        padding: '32px',
        background: '#0f172a',
        color: '#e2e8f0',
        fontFamily: 'system-ui, sans-serif',
      }}
    >
      <h1 style={{ marginTop: 0 }}>Phantom Screen Next.js smoke test</h1>
      <p style={{ maxWidth: '720px', lineHeight: 1.6 }}>
        This fixture only verifies that the published client tarball installs cleanly and can be
        imported into a Next.js app without build-time errors.
      </p>
      <PhantomScreenDemo />
    </main>
  );
}
