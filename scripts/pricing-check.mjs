#!/usr/bin/env node
// 每日定價檢查：以官方文件網址為權威來源登錄，並用 models.dev 作為機器可讀索引，
// 比對 pricing.csv 的基礎列（無分級／無推理強度後綴）是否有價格差異。
// 僅產生報告（Markdown），不會直接改寫 pricing.csv，需人工對照官方文件確認後再更新。
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const MODELS_DEV_URL = 'https://models.dev/api.json';
const EPSILON = 1e-9;

export function parseCsv(text) {
  const rows = [];
  for (const line of text.replace(/^\uFEFF/, '').split(/\r?\n/).slice(1)) {
    if (!line.trim()) continue;
    const c = line.split(',');
    if (c.length < 6) continue;
    rows.push({
      model: c[0].trim(),
      deployment: c[1].trim(),
      input: toNumber(c[3]),
      cached: toNumber(c[4]),
      output: toNumber(c[5]),
    });
  }
  return rows;
}

function toNumber(value) {
  const n = Number.parseFloat(value);
  return Number.isFinite(n) ? n : null;
}

// 分級列（如 "(<200k)"）屬於長上下文費率，models.dev 只有基礎價，因此排除。
export function isBaseRow(name) {
  return !/\((?:<|>)\s*\d+k\)/i.test(name);
}

export function normalizeName(name) {
  return name.toLowerCase().replace(/[^a-z0-9.]+/g, ' ').trim();
}

export function compareProvider(csvRows, modelsDevModels) {
  const index = new Map();
  for (const m of Object.values(modelsDevModels)) {
    if (m.cost && m.cost.input != null && m.cost.output != null) {
      index.set(normalizeName(m.name), m);
    }
  }
  const mismatches = [];
  const matched = new Set();
  for (const row of csvRows.filter((r) => isBaseRow(r.model))) {
    const key = normalizeName(row.model);
    const remote = index.get(key);
    if (!remote) continue;
    matched.add(key);
    const diffs = [];
    const check = (label, local, upstream) => {
      if (upstream == null || local == null) return;
      if (Math.abs(local - upstream) > EPSILON) diffs.push(`${label}: CSV ${local} → 上游 ${upstream}`);
    };
    check('輸入', row.input, remote.cost.input);
    check('輸出', row.output, remote.cost.output);
    check('快取輸入', row.cached, remote.cost.cache_read);
    if (diffs.length) mismatches.push({ model: row.model, diffs });
  }
  const missing = [...index.entries()]
    .filter(([key]) => !matched.has(key) && !csvRows.some((r) => normalizeName(r.model) === key))
    .map(([, m]) => m.name);
  return { mismatches, missing };
}

async function fetchWithTimeout(url, init = {}) {
  return fetch(url, { ...init, signal: AbortSignal.timeout(30000), headers: { 'user-agent': 'TokenUsageInsights-pricing-check' } });
}

export async function run({ csvPath = path.join(root, 'pricing.csv'), reportPath } = {}) {
  const sources = JSON.parse(readFileSync(path.join(root, 'scripts', 'pricing-sources.json'), 'utf8'));
  const csvRows = parseCsv(readFileSync(csvPath, 'utf8'));
  const lines = ['# 模型定價每日檢查報告', '', `產生時間：${new Date().toISOString()}`, ''];
  let drift = 0;
  let errors = 0;

  let api;
  try {
    const res = await fetchWithTimeout(MODELS_DEV_URL);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    api = await res.json();
    if (api === null || typeof api !== 'object' || Array.isArray(api)) throw new Error('回應格式不符（預期為物件）');
  } catch (err) {
    errors++;
    lines.push(`> ⚠️ 無法取得 models.dev 索引：${err.message}。本次略過比對（保留現有 pricing.csv）。`, '');
  }

  for (const p of sources.providers) {
    lines.push(`## ${p.name}`, '', '官方定價文件（權威來源，請以此為準）：');
    for (const url of p.officialUrls) {
      let status;
      try {
        const res = await fetchWithTimeout(url, { method: 'GET' });
        if (!res.ok) errors++;
        status = res.ok ? '可連線' : `HTTP ${res.status}（可能為防爬蟲，請人工開啟）`;
      } catch (err) {
        errors++;
        status = `無法連線：${err.message}`;
      }
      lines.push(`- <${url}> — ${status}`);
    }
    lines.push('');
    const models = api?.[p.modelsDevProvider]?.models;
    if (!models) {
      if (api) { errors++; lines.push(`> ⚠️ models.dev 缺少 provider \`${p.modelsDevProvider}\`。`, ''); }
      continue;
    }
    const { mismatches, missing } = compareProvider(
      csvRows.filter((r) => p.deployments.includes(r.deployment)),
      models,
    );
    drift += mismatches.length; // 新模型僅列入報告，不單獨觸發 issue（含大量非文字模型）
    if (mismatches.length) {
      lines.push('### 價格差異（USD / 1M tokens）', '');
      for (const m of mismatches) lines.push(`- **${m.model}**：${m.diffs.join('；')}`);
      lines.push('');
    }
    if (missing.length) {
      lines.push('### pricing.csv 尚未收錄的新模型', '');
      for (const name of missing) lines.push(`- ${name}`);
      lines.push('');
    }
    if (!mismatches.length && !missing.length) lines.push('與索引一致，無差異。', '');
  }

  const report = lines.join('\n');
  if (reportPath) writeFileSync(reportPath, report);
  return { report, drift, errors };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const idx = process.argv.indexOf('--report');
  const reportPath = idx > -1 ? process.argv[idx + 1] : undefined;
  const { report, drift, errors } = await run({ reportPath });
  console.log(report);
  console.log(`\ndrift=${drift} errors=${errors}`);
  if (process.env.GITHUB_OUTPUT) {
    writeFileSync(process.env.GITHUB_OUTPUT, `drift=${drift}\nerrors=${errors}\n`, { flag: 'a' });
  }
}
