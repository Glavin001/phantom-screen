import { DEFAULT_SERVER_URL, mountPhantomScreen } from './index';

const root = document.getElementById('app');
if (!(root instanceof HTMLElement)) {
  throw new Error('Missing #app mount element');
}

document.documentElement.style.height = '100%';
document.body.style.margin = '0';
document.body.style.height = '100%';
root.style.width = '100%';
root.style.height = '100vh';

const params = new URLSearchParams(window.location.search);
mountPhantomScreen(root, {
  serverUrl: params.get('serverUrl') ?? DEFAULT_SERVER_URL,
  serverCertificateHash: params.get('certHash') ?? undefined,
  autoConnect: params.get('autoconnect') === '1',
  useShadowDom: true,
});
