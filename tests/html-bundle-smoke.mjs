import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const bundlePath = process.argv[2];
if (!bundlePath) {
  throw new Error('Usage: node tests/html-bundle-smoke.mjs <bundle-path>');
}

const source = await readFile(bundlePath, 'utf8');
const context = {
  console,
  setTimeout,
  clearTimeout,
};

context.globalThis = context;
context.window = context;
context.self = context;

vm.runInNewContext(source, context, { filename: bundlePath });

const bundle = context.PhantomScreenClient;
assert.ok(bundle, 'expected IIFE bundle to expose globalThis.PhantomScreenClient');
assert.equal(typeof bundle.mountPhantomScreen, 'function');
assert.equal(typeof bundle.createServerCertificateHashes, 'function');

console.log('HTML bundle exports verified');
