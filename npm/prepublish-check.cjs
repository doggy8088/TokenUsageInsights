#!/usr/bin/env node
'use strict';

const { spawnSync } = require('node:child_process');
const { readFileSync } = require('node:fs');
const { request } = require('node:https');
const { join } = require('node:path');
const { URL } = require('node:url');
const { artifactName, releaseBaseUrl, TARGETS } = require('./install.cjs');

const PACKAGE_ROOT = join(__dirname, '..');
const MAX_REDIRECTS = 5;

function packageVersion() {
  return require('../package.json').version;
}

function cargoVersion() {
  const cargo = readFileSync(join(PACKAGE_ROOT, 'Cargo.toml'), 'utf8');
  const packageSection = cargo.match(/^\[package\]\s*([\s\S]*?)(?=^\[|\Z)/m)?.[1] ?? '';
  const version = packageSection.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error('無法讀取 Cargo.toml 的 package.version');
  return version;
}

function expectedReleaseUrls(version = packageVersion()) {
  const base = releaseBaseUrl(version);
  return [
    ...Object.values(TARGETS).map((target) => `${base}/${artifactName(target, version)}`),
    `${base}/SHA256SUMS`,
  ];
}

function checkUrl(url, redirectsRemaining = MAX_REDIRECTS) {
  return new Promise((resolve) => {
    const req = request(
      url,
      { method: 'HEAD', headers: { 'User-Agent': 'token-usage-insights-publish-check' } },
      (response) => {
        const { statusCode, headers } = response;
        response.resume();
        if (statusCode >= 300 && statusCode < 400 && headers.location && redirectsRemaining > 0) {
          const nextUrl = new URL(headers.location, url).toString();
          checkUrl(nextUrl, redirectsRemaining - 1).then((result) => resolve({ ...result, url }));
          return;
        }
        resolve({ url, ok: statusCode >= 200 && statusCode < 300, statusCode });
      },
    );
    req.setTimeout(30_000, () => req.destroy(new Error(`request timed out: ${url}`)));
    req.on('error', (error) => resolve({ url, ok: false, errorMessage: error.message }));
    req.end();
  });
}

function retryCountFromEnv() {
  return Number.parseInt(process.env.TOKEN_USAGE_INSIGHTS_RELEASE_ASSET_RETRIES ?? '1', 10);
}

function retryDelayMsFromEnv() {
  return Number.parseInt(
    process.env.TOKEN_USAGE_INSIGHTS_RELEASE_ASSET_RETRY_DELAY_MS ?? '1000',
    10,
  );
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function assertVersionAlignment(version = packageVersion()) {
  const rustVersion = cargoVersion();
  if (version !== rustVersion) {
    throw new Error(`版本不一致：package.json=${version}，Cargo.toml=${rustVersion}`);
  }
}

function assertExactReleaseTag(version = packageVersion()) {
  const result = spawnSync('git', ['describe', '--tags', '--exact-match', 'HEAD'], {
    cwd: PACKAGE_ROOT,
    encoding: 'utf8',
  });
  const actual = result.status === 0 ? result.stdout.trim() : '';
  if (actual !== `v${version}`) {
    throw new Error(`npm 僅能從對應 Release commit 發布；預期目前 tag 為 v${version}`);
  }
}

async function verifyReleaseAssets({
  version = packageVersion(),
  check = checkUrl,
  retries = retryCountFromEnv(),
  retryDelayMs = retryDelayMsFromEnv(),
} = {}) {
  const urls = expectedReleaseUrls(version);
  let failures = [];
  for (let attempt = 1; attempt <= retries; attempt += 1) {
    const results = await Promise.all(urls.map((url) => check(url)));
    failures = results.filter((result) => !result.ok);
    if (failures.length === 0) return urls;
    if (attempt < retries) await sleep(retryDelayMs);
  }
  const details = failures.map((failure) => {
    const reason = failure.statusCode ? `HTTP ${failure.statusCode}` : failure.errorMessage;
    return `- ${failure.url}：${reason}`;
  });
  throw new Error(
    [`v${version} 的 GitHub Release 資產尚未備妥：`, ...details].join('\n'),
  );
}

async function main() {
  const version = packageVersion();
  assertVersionAlignment(version);
  assertExactReleaseTag(version);
  const urls = await verifyReleaseAssets({ version });
  console.log(`已驗證 v${version} 的 ${urls.length} 個 GitHub Release 下載項目。`);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}

module.exports = {
  assertVersionAlignment,
  cargoVersion,
  checkUrl,
  expectedReleaseUrls,
  verifyReleaseAssets,
};
