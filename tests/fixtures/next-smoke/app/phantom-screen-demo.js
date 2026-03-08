'use client';

import { useEffect, useRef } from 'react';

import { mountPhantomScreen } from '@phantom-screen/web-client';

export default function PhantomScreenDemo() {
  const containerRef = useRef(null);

  useEffect(() => {
    if (!containerRef.current) {
      return undefined;
    }

    const client = mountPhantomScreen(containerRef.current, {
      serverUrl: 'https://127.0.0.1:4443',
    });

    return () => {
      client.destroy();
    };
  }, []);

  return (
    <div
      ref={containerRef}
      style={{
        width: '100%',
        height: '480px',
        borderRadius: '16px',
        overflow: 'hidden',
        border: '1px solid rgba(255, 255, 255, 0.12)',
      }}
    />
  );
}
