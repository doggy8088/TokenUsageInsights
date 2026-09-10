'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const test = require('node:test');

const {
  WINDOWS_ZIP_SCRIPT,
  artifactName,
  cargoTarget,
  checksumForArtifact,
  copyReleaseContents,
  findReleaseRoot,
  releaseBaseUrl,
  sha256,
  verifyChecksum,
  windowsPowerShellEnvironment,
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

test('strips inherited PSModulePath so Windows PowerShell 5.1 uses its own modules', () => {
  const env = windowsPowerShellEnvironment('C:\\a.zip', 'C:\\out', {
    PATH: 'x',
    PSModulePath: 'C:\\Program Files\\PowerShell\\7\\Modules',
    PsModulePath: 'mixed-case',
  });
  assert.equal(env.PATH, 'x');
  assert.equal(env.TUI_ARCHIVE, 'C:\\a.zip');
  assert.equal(env.TUI_DESTINATION, 'C:\\out');
  assert.ok(!Object.keys(env).some((key) => key.toLowerCase() === 'psmodulepath'));
});

test('Windows zip fallback script does not rely on Microsoft.PowerShell.Archive', () => {
  assert.doesNotMatch(WINDOWS_ZIP_SCRIPT, /Expand-Archive|Import-Module/);
  assert.match(WINDOWS_ZIP_SCRIPT, /System\.IO\.Compression\.ZipFile/);
});

const pwsh = spawnSync('pwsh', ['-NoProfile', '-NonInteractive', '-Command', '$PSVersionTable.PSVersion.Major'], {
  encoding: 'utf8',
});
const hasPwsh = !pwsh.error && pwsh.status === 0;

test('Windows zip fallback script extracts archives and rejects unsafe entries', { skip: !hasPwsh && 'pwsh not installed' }, () => {
  const directory = mkdtempSync(join(tmpdir(), 'tui-zip-'));
  try {
    const source = join(directory, 'src', 'token-usage-insights-v1.2.3-x86_64-pc-windows-msvc');
    mkdirSync(join(source, 'static'), { recursive: true });
    writeFileSync(join(source, 'token-usage-insights.exe'), 'bin');
    writeFileSync(join(source, 'static', 'app.css'), 'css');
    writeFileSync(join(source, 'pricing.csv'), 'pricing');
    const archive = join(directory, 'release.zip');
    const zip = spawnSync(
      'pwsh',
      ['-NoProfile', '-NonInteractive', '-Command', 'Compress-Archive -Path $env:SRC -DestinationPath $env:ZIP'],
      { env: { ...process.env, SRC: join(directory, 'src', '*'), ZIP: archive }, encoding: 'utf8' },
    );
    assert.equal(zip.status, 0, zip.stderr);

    const destination = join(directory, 'out');
    mkdirSync(destination);
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const result = spawnSync('pwsh', ['-NoProfile', '-NonInteractive', '-Command', WINDOWS_ZIP_SCRIPT], {
        env: windowsPowerShellEnvironment(archive, destination),
        encoding: 'utf8',
      });
      assert.equal(result.status, 0, result.stderr);
    }
    const root = findReleaseRoot(destination, 'token-usage-insights.exe');
    assert.equal(readFileSync(join(root, 'static', 'app.css'), 'utf8'), 'css');

    const unsafe = join(directory, 'unsafe.zip');
    const forge = spawnSync(
      'pwsh',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        [
          'Add-Type -AssemblyName System.IO.Compression.FileSystem',
          '$zip = [System.IO.Compression.ZipFile]::Open($env:ZIP, [System.IO.Compression.ZipArchiveMode]::Create)',
          "$entry = $zip.CreateEntry('../escaped.txt')",
          '$stream = $entry.Open(); $stream.WriteByte(65); $stream.Dispose(); $zip.Dispose()',
        ].join('\n'),
      ],
      { env: { ...process.env, ZIP: unsafe }, encoding: 'utf8' },
    );
    assert.equal(forge.status, 0, forge.stderr);
    const rejected = spawnSync('pwsh', ['-NoProfile', '-NonInteractive', '-Command', WINDOWS_ZIP_SCRIPT], {
      env: windowsPowerShellEnvironment(unsafe, destination),
      encoding: 'utf8',
    });
    assert.notEqual(rejected.status, 0);
    assert.ok(!existsSync(join(directory, 'escaped.txt')));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
