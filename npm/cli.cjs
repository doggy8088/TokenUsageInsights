#!/usr/bin/env node
'use strict';

const { spawnSync } = require('node:child_process');
const { join } = require('node:path');
const { installBinary, isReleaseRoot } = require('./install.cjs');

const BINARY_NAME = 'token-usage-insights';
const executableName = process.platform === 'win32' ? `${BINARY_NAME}.exe` : BINARY_NAME;
const installDirectory = join(__dirname, `${BINARY_NAME}-bin`);
const executable = join(__dirname, `${BINARY_NAME}-bin`, executableName);

async function main() {
  if (!isReleaseRoot(installDirectory, executableName)) await installBinary();

  if (!isReleaseRoot(installDirectory, executableName)) {
    throw new Error(
      `${BINARY_NAME} 原生執行檔尚未建置；請先執行 cargo build --release。`,
    );
  }

  const result = spawnSync(executable, process.argv.slice(2), { stdio: 'inherit' });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
}

main().catch((error) => {
  console.error(`${BINARY_NAME} 啟動失敗：${error.message}`);
  process.exitCode = 1;
});
