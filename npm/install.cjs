#!/usr/bin/env node
'use strict';

const { createHash } = require('node:crypto');
const { spawnSync } = require('node:child_process');
const {
  chmodSync,
  copyFileSync,
  cpSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
} = require('node:fs');
const { get } = require('node:https');
const { tmpdir } = require('node:os');
const { basename, join } = require('node:path');
const { pipeline } = require('node:stream/promises');
const { URL } = require('node:url');

const PACKAGE_ROOT = join(__dirname, '..');
const BINARY_NAME = 'token-usage-insights';
const GITHUB_OWNER = 'doggy8088';
const GITHUB_REPO = 'TokenUsageInsights';
const INSTALL_DIR = join(__dirname, `${BINARY_NAME}-bin`);
const EXECUTABLE_NAME = process.platform === 'win32' ? `${BINARY_NAME}.exe` : BINARY_NAME;
const TARGETS = Object.freeze({
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
  'win32-x64': 'x86_64-pc-windows-msvc',
});

function platformKey(platform = process.platform, arch = process.arch) {
  return `${platform}-${arch}`;
}

function cargoTarget(platform = process.platform, arch = process.arch) {
  const target = TARGETS[platformKey(platform, arch)];
  if (!target) {
    throw new Error(
      `不支援的平台：${platform}/${arch}。` +
        '目前支援 Windows x64、Linux x64、Intel Mac 與 Apple Silicon Mac。',
    );
  }
  return target;
}

function packageVersion() {
  return require(join(PACKAGE_ROOT, 'package.json')).version;
}

function artifactName(target, version = packageVersion()) {
  const extension = target.includes('windows') ? 'zip' : 'tar.gz';
  return `${BINARY_NAME}-v${version}-${target}.${extension}`;
}

function releaseBaseUrl(version = packageVersion()) {
  return `https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}/releases/download/v${version}`;
}

function sha256(filePath) {
  return createHash('sha256').update(readFileSync(filePath)).digest('hex');
}

function checksumForArtifact(checksumText, artifact) {
  for (const line of checksumText.split(/\r?\n/)) {
    const match = line.match(/^([a-fA-F0-9]{64})\s+\*?(.+?)\s*$/);
    if (match && basename(match[2]) === artifact) return match[1].toLowerCase();
  }
  throw new Error(`SHA256SUMS 找不到 ${artifact}`);
}

function verifyChecksum(filePath, checksumText, artifact = basename(filePath)) {
  const expected = checksumForArtifact(checksumText, artifact);
  const actual = sha256(filePath);
  if (actual !== expected) {
    throw new Error(`${artifact} 校驗失敗：預期 ${expected}，實際 ${actual}`);
  }
}

function download(url, destination, redirectsRemaining = 5) {
  return new Promise((resolve, reject) => {
    const request = get(url, { headers: { 'User-Agent': `${BINARY_NAME}-npm-installer` } }, (response) => {
      const { statusCode, headers } = response;
      if (statusCode >= 300 && statusCode < 400 && headers.location && redirectsRemaining > 0) {
        response.resume();
        const nextUrl = new URL(headers.location, url).toString();
        download(nextUrl, destination, redirectsRemaining - 1).then(resolve, reject);
        return;
      }
      if (statusCode !== 200) {
        response.resume();
        reject(new Error(`下載失敗 HTTP ${statusCode}：${url}`));
        return;
      }
      pipeline(response, createWriteStream(destination, { mode: 0o600 })).then(resolve, reject);
    });
    request.setTimeout(30_000, () => request.destroy(new Error(`下載逾時：${url}`)));
    request.on('error', reject);
  });
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { stdio: 'inherit', ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`命令執行失敗：${command}`);
}

function extract(archive, destination) {
  mkdirSync(destination, { recursive: true });
  if (archive.endsWith('.zip') && process.platform === 'win32') {
    run(
      'powershell.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        'Expand-Archive -LiteralPath $env:TUI_ARCHIVE -DestinationPath $env:TUI_DESTINATION -Force',
      ],
      { env: { ...process.env, TUI_ARCHIVE: archive, TUI_DESTINATION: destination } },
    );
    return;
  }
  run('tar', [archive.endsWith('.tar.gz') ? '-xzf' : '-xf', archive, '-C', destination]);
}

function isReleaseRoot(directory, executableName = EXECUTABLE_NAME) {
  return (
    existsSync(join(directory, executableName)) &&
    existsSync(join(directory, 'static')) &&
    existsSync(join(directory, 'pricing.csv'))
  );
}

function findReleaseRoot(directory, executableName = EXECUTABLE_NAME) {
  if (isReleaseRoot(directory, executableName)) return directory;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const candidate = join(directory, entry.name);
    if (isReleaseRoot(candidate, executableName)) return candidate;
  }
  throw new Error('GitHub Release 壓縮包缺少執行檔、static 或 pricing.csv');
}

function copyReleaseContents(source, destination, executableName = EXECUTABLE_NAME) {
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });
  for (const entry of readdirSync(source, { withFileTypes: true })) {
    cpSync(join(source, entry.name), join(destination, entry.name), {
      recursive: entry.isDirectory(),
      force: true,
    });
  }
  chmodSync(join(destination, executableName), 0o755);
}

function installFromLocalBuild() {
  const localBinary = join(PACKAGE_ROOT, 'target', 'release', EXECUTABLE_NAME);
  if (!existsSync(localBinary)) return false;

  rmSync(INSTALL_DIR, { recursive: true, force: true });
  mkdirSync(INSTALL_DIR, { recursive: true });
  copyFileSync(localBinary, join(INSTALL_DIR, EXECUTABLE_NAME));
  chmodSync(join(INSTALL_DIR, EXECUTABLE_NAME), 0o755);
  for (const item of ['static', 'shell', 'scripts', 'pricing.csv', 'README.md', 'LICENSE']) {
    const source = join(PACKAGE_ROOT, item);
    if (existsSync(source)) {
      cpSync(source, join(INSTALL_DIR, item), { recursive: true, force: true });
    }
  }
  console.log(`已安裝本機建置的 ${BINARY_NAME}`);
  return true;
}

async function installFromRelease() {
  const version = packageVersion();
  const target = cargoTarget();
  const artifact = artifactName(target, version);
  const baseUrl = releaseBaseUrl(version);
  const temporaryDirectory = mkdtempSync(join(tmpdir(), `${BINARY_NAME}-`));
  const archive = join(temporaryDirectory, artifact);
  const checksums = join(temporaryDirectory, 'SHA256SUMS');
  const extracted = join(temporaryDirectory, 'extracted');

  try {
    console.log(`正在下載 ${BINARY_NAME} v${version}：${target}`);
    await download(`${baseUrl}/${artifact}`, archive);
    await download(`${baseUrl}/SHA256SUMS`, checksums);
    verifyChecksum(archive, readFileSync(checksums, 'utf8'), artifact);
    extract(archive, extracted);
    copyReleaseContents(findReleaseRoot(extracted), INSTALL_DIR);
    console.log(`已安裝 ${BINARY_NAME} v${version}`);
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}

async function installBinary() {
  if (installFromLocalBuild()) return;
  if (existsSync(join(PACKAGE_ROOT, 'Cargo.toml'))) {
    console.log('偵測到原始碼工作目錄；略過 npm 原生執行檔下載。');
    return;
  }
  await installFromRelease();
}

if (require.main === module) {
  installBinary().catch((error) => {
    rmSync(INSTALL_DIR, { recursive: true, force: true });
    console.error(`安裝 ${BINARY_NAME} 失敗：${error.message}`);
    process.exit(1);
  });
}

module.exports = {
  TARGETS,
  artifactName,
  cargoTarget,
  checksumForArtifact,
  copyReleaseContents,
  findReleaseRoot,
  installBinary,
  isReleaseRoot,
  platformKey,
  releaseBaseUrl,
  sha256,
  verifyChecksum,
};
