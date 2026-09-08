'use strict';

const assert = require('node:assert/strict');
const { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const test = require('node:test');

const {
  artifactName,
  cargoTarget,
  checksumForArtifact,
  copyReleaseContents,
  findReleaseRoot,
  releaseBaseUrl,
  sha256,
  verifyChecksum,
} = require('../npm/install.cjs');
const {
  assertVersionAlignment,
  expectedReleaseUrls,
  verifyReleaseAssets,
} = require('../npm/prepublish-check.cjs');

test('maps every supported Node platform to the release Rust target', () => {
  assert.equal(cargoTarget('darwin', 'arm64'), 'aarch64-apple-darwin');
  assert.equal(cargoTarget('darwin', 'x64'), 'x86_64-apple-darwin');
  assert.equal(cargoTarget('linux', 'x64'), 'x86_64-unknown-linux-gnu');
  assert.equal(cargoTarget('win32', 'x64'), 'x86_64-pc-windows-msvc');
  assert.throws(() => cargoTarget('linux', 'arm'), /不支援的平台/);
});

test('uses the existing GitHub Release artifact contract', () => {
  assert.equal(
    artifactName('x86_64-unknown-linux-gnu', '1.2.3'),
    'token-usage-insights-v1.2.3-x86_64-unknown-linux-gnu.tar.gz',
  );
  assert.equal(
    artifactName('x86_64-pc-windows-msvc', '1.2.3'),
    'token-usage-insights-v1.2.3-x86_64-pc-windows-msvc.zip',
  );
  assert.equal(
    releaseBaseUrl('1.2.3'),
    'https://github.com/doggy8088/TokenUsageInsights/releases/download/v1.2.3',
  );
  const urls = expectedReleaseUrls('1.2.3');
  assert.equal(urls.length, 5);
  assert.ok(urls.at(-1).endsWith('/SHA256SUMS'));
});

test('selects the named checksum and rejects missing or altered archives', () => {
  const directory = mkdtempSync(join(tmpdir(), 'token-usage-insights-checksum-'));
  try {
    const archive = join(directory, 'sample.tar.gz');
    writeFileSync(archive, 'verified payload');
    const digest = sha256(archive);
    const sums = `${'0'.repeat(64)}  other.zip\n${digest}  ./sample.tar.gz\n`;
    assert.equal(checksumForArtifact(sums, 'sample.tar.gz'), digest);
    verifyChecksum(archive, sums);
    writeFileSync(archive, 'modified payload');
    assert.throws(() => verifyChecksum(archive, sums), /校驗失敗/);
    assert.throws(() => checksumForArtifact(sums, 'missing.zip'), /找不到/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('finds and copies the complete dashboard release payload', () => {
  const directory = mkdtempSync(join(tmpdir(), 'token-usage-insights-payload-'));
  try {
    const release = join(directory, 'archive', 'token-usage-insights-v1-test');
    const destination = join(directory, 'installed');
    mkdirSync(join(release, 'static'), { recursive: true });
    mkdirSync(join(release, 'shell'), { recursive: true });
    writeFileSync(join(release, 'token-usage-insights'), 'binary');
    writeFileSync(join(release, 'pricing.csv'), 'model,price');
    writeFileSync(join(release, 'static', 'index.html'), '<main></main>');
    writeFileSync(join(release, 'shell', 'statusline-token.sh'), '#!/bin/sh');

    const root = findReleaseRoot(join(directory, 'archive'), 'token-usage-insights');
    assert.equal(root, release);
    copyReleaseContents(root, destination, 'token-usage-insights');
    assert.equal(readFileSync(join(destination, 'token-usage-insights'), 'utf8'), 'binary');
    assert.equal(readFileSync(join(destination, 'static', 'index.html'), 'utf8'), '<main></main>');
    assert.ok(readFileSync(join(destination, 'shell', 'statusline-token.sh'), 'utf8'));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('keeps Cargo and npm package versions aligned', () => {
  assert.doesNotThrow(() => assertVersionAlignment());
});

test('does not rely on dependency install scripts under npm 12', () => {
  const packageJson = require('../package.json');
  assert.equal(packageJson.scripts.postinstall, undefined);
  assert.match(readFileSync(join(__dirname, '..', 'npm', 'cli.cjs'), 'utf8'), /installBinary/);
});

test('release asset verification reports all failed URLs', async () => {
  const error = await verifyReleaseAssets({
    version: '1.2.3',
    check: async (url) => ({ url, ok: false, statusCode: 404 }),
    retries: 1,
  }).catch((reason) => reason);
  assert.match(error.message, /v1\.2\.3/);
  assert.equal((error.message.match(/HTTP 404/g) ?? []).length, 5);
});
