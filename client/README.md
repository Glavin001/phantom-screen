# `@phantom-screen/web-client`

Embeddable Phantom Screen browser client package.

## Install from a release tarball

```bash
npm install ./phantom-screen-web-client-0.1.0.tgz
```

Release workflows publish a tarball for each `main` commit SHA, so you can install a specific build directly from the matching GitHub Release asset.

## Use in React / Next.js

```tsx
'use client';

import { useEffect, useRef } from 'react';
import { mountPhantomScreen } from '@phantom-screen/web-client';

export function RemoteDesktop() {
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!ref.current) return;

    const client = mountPhantomScreen(ref.current, {
      serverUrl: 'https://127.0.0.1:4443',
      serverCertificateHash: process.env.NEXT_PUBLIC_PHANTOM_CERT_HASH,
    });

    return () => client.destroy();
  }, []);

  return <div ref={ref} style={{ width: '100%', height: '70vh' }} />;
}
```

## Use from a plain HTML page

Download the HTML bundle archive from the release, then load `phantom-screen-client.iife.js`:

```html
<div id="remote-desktop" style="height:70vh"></div>
<script src="./phantom-screen-client.iife.js"></script>
<script>
  window.PhantomScreenClient.mountPhantomScreen(
    document.getElementById('remote-desktop'),
    {
      serverUrl: 'https://127.0.0.1:4443',
      serverCertificateHash: 'paste-your-sha256-cert-hash-here',
    },
  );
</script>
```

See `examples/embed.html` for a complete example page.
