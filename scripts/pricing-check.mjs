#!/usr/bin/env node
// 每日定價檢查：直接解析各供應商「官方定價文件」（以官方站台提供的 Markdown 版本為資料來源），
// 與 pricing.csv 的基礎列（無長上下文分級）比對，回報價格差異、促銷價以及尚未收錄的新模型。
// 僅產生報告（Markdown），不會直接改寫 pricing.csv，需人工對照官方文件確認後再更新。
// 若某個官方頁面抓取失敗或解析不到任何價格（通常代表頁面改版），會列為錯誤。
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const EPSILON = 1e-9;
// 非文字模型（圖像、語音、嵌入等）不在 pricing.csv 的追蹤範圍，列新模型時略過。
const NON_TEXT = /tts|image|embedding|live|lyria|transcribe|robotics|veo|imagen|omni|nano banana|gemma|audio|imagine|video|speech/i;

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

// 分級列（如 "(<200k)"）屬於長上下文費率，官方基礎價不適用，因此排除。
export function isBaseRow(name) {
  return !/\((?:<|>)\s*\d+k\)/i.test(name);
}

export function normalizeName(name) {
  return name.toLowerCase().replace(/[^a-z0-9.]+/g, ' ').trim();
}

// CSV 的推理強度／Thinking 後綴與官方基礎模型共用同一組價格。
export function csvKey(name) {
  return normalizeName(name.replace(/\s*\((?:low|medium|high|xhigh|minimal|thinking|non-thinking|reasoning)\)/gi, ''));
}

export function officialKey(name) {
  return normalizeName(
    name
      .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/<[^>]+>/g, '')
      .replace(/\([^)]*\)/g, '')
      .replace(/[^\x20-\x7E]/g, ''),
  );
}

// 取出價格文字中的第一個金額；若為促銷格式（"$0.75 through Dec 31, 2026. $1.50 starting ..."）一併回傳促銷後價格。
export function parsePrice(cell) {
  if (!cell) return null;
  const promo = cell.match(/\$([\d.]+)\s+through\s+(.+?\d{4})\.\s*\$([\d.]+)\s+starting/i);
  if (promo) return { now: Number(promo[1]), after: Number(promo[3]), until: promo[2] };
  const m = cell.match(/\$\s*([\d,]*\.?\d+)/);
  return m ? { now: Number(m[1].replace(/,/g, '')), after: null, until: null } : null;
}

function splitRow(line) {
  return line.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
}

function sectionLines(md, heading) {
  const lines = md.split(/\r?\n/);
  if (!heading) return lines;
  const level = heading.match(/^#+/)[0].length;
  const start = lines.findIndex((l) => l.trim() === heading);
  if (start < 0) return [];
  const out = [];
  for (let i = start + 1; i < lines.length; i++) {
    const h = lines[i].match(/^(#+)\s/);
    if (h && h[1].length <= level) break;
    out.push(lines[i]);
  }
  return out;
}

// 解析 Markdown 表格：以表頭文字尋找輸入／快取輸入／輸出欄位，同名模型只保留第一列（短上下文基礎價）。
export function parseTableProvider(md, heading) {
  const lines = sectionLines(md, heading);
  const entries = new Map();
  let cols = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim().startsWith('|')) { cols = null; continue; }
    const next = lines[i + 1] || '';
    if (/^\s*\|?\s*:?-{2,}/.test(next) && !cols) {
      const headers = splitRow(line);
      const find = (re, not) => headers.findIndex((h) => re.test(h) && !(not && not.test(h)));
      cols = {
        input: find(/input/i, /cache|write|audio/i),
        cached: find(/cache hits|cached input/i),
        output: find(/output/i),
      };
      i++; // 略過分隔列
      continue;
    }
    if (!cols || cols.input < 0 || cols.output < 0) continue;
    const cells = splitRow(line);
    const key = officialKey(cells[0] || '');
    if (!key || entries.has(key)) continue;
    const input = parsePrice(cells[cols.input]);
    const output = parsePrice(cells[cols.output]);
    if (!input || !output) continue;
    entries.set(key, {
      name: cells[0].replace(/\[([^\]]*)\]\([^)]*\)/g, '$1').replace(/<[^>]+>/g, '').replace(/\s*\(.*$/, '').trim(),
      input,
      output,
      cached: cols.cached >= 0 ? parsePrice(cells[cols.cached]) : null,
    });
  }
  return entries;
}

// Google 定價頁以「## 模型名稱 → ### Standard 表格」呈現，價格在 Paid Tier 欄。
export function parseGeminiProvider(md) {
  const entries = new Map();
  const lines = md.split(/\r?\n/);
  let model = null;
  let inStandard = false;
  let row = {};
  const flush = () => {
    if (model && row.input && row.output) {
      const key = officialKey(model);
      if (!entries.has(key)) entries.set(key, { name: model, input: row.input, output: row.output, cached: row.cached || null });
    }
    row = {};
  };
  for (const line of lines) {
    const h2 = line.match(/^##\s+(.+)$/);
    if (h2) {
      flush();
      model = h2[1].replace(/\[([^\]]*)\]\([^)]*\)/g, '$1').replace(/[^\x20-\x7E]/g, '').trim();
      inStandard = false;
      continue;
    }
    const h3 = line.match(/^###\s+(.+)$/);
    if (h3) {
      if (inStandard) flush();
      inStandard = h3[1].trim() === 'Standard';
      continue;
    }
    if (!inStandard || !line.trim().startsWith('|')) continue;
    const cells = splitRow(line);
    const label = (cells[0] || '').toLowerCase();
    const paid = cells[cells.length - 1];
    if (label.startsWith('input price')) row.input = parsePrice(paid);
    else if (label.startsWith('output price')) row.output = parsePrice(paid);
    else if (label.startsWith('context caching price')) row.cached = parsePrice(paid);
  }
  flush();
  return entries;
}

export function parseProvider(provider, md) {
  return provider.parser === 'gemini' ? parseGeminiProvider(md) : parseTableProvider(md, provider.sectionHeading);
}

const matchesPrice = (local, p) =>
  Math.abs(local - p.now) <= EPSILON || (p.after != null && Math.abs(local - p.after) <= EPSILON);

export function compareProvider(csvRows, official, ignoreModels = []) {
  const ignored = new Set(ignoreModels.map(officialKey));
  const mismatches = [];
  const promos = [];
  const known = new Set();
  for (const row of csvRows) {
    known.add(csvKey(row.model));
    if (!isBaseRow(row.model)) continue;
    const remote = official.get(csvKey(row.model));
    if (!remote) continue;
    const diffs = [];
    for (const [label, local, price] of [
      ['輸入', row.input, remote.input],
      ['輸出', row.output, remote.output],
      ['快取輸入', row.cached, remote.cached],
    ]) {
      if (local == null || !price) continue;
      if (!matchesPrice(local, price)) {
        diffs.push(`${label}：CSV ${local} → 官方 ${price.now}${price.after != null ? `（${price.until} 後 ${price.after}）` : ''}`);
      } else if (price.after != null && Math.abs(local - price.now) > EPSILON) {
        promos.push(`${csvKey(row.model)} ${label}：官方目前促銷價 ${price.now}，${price.until} 後為 ${price.after}（CSV 已採後者）`);
      }
    }
    if (diffs.length) mismatches.push({ model: row.model, diffs });
  }
  const missing = [...official.entries()]
    .filter(([key, e]) => !known.has(key) && !ignored.has(key) && !NON_TEXT.test(e.name))
    .map(([, e]) => e.name);
  return { mismatches, promos: [...new Set(promos)], missing };
}

async function fetchText(url) {
  let lastError;
  for (let attempt = 1; attempt <= 3; attempt++) {
    try {
      const res = await fetch(url, {
        signal: AbortSignal.timeout(30000),
        headers: { 'user-agent': 'Mozilla/5.0 (compatible; TokenUsageInsights-pricing-check)' },
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return await res.text();
    } catch (err) {
      lastError = err;
      await new Promise((r) => setTimeout(r, attempt * 1000));
    }
  }
  throw lastError;
}

export async function run({ csvPath = path.join(root, 'pricing.csv'), reportPath } = {}) {
  const sources = JSON.parse(readFileSync(path.join(root, 'scripts', 'pricing-sources.json'), 'utf8'));
  const csvRows = parseCsv(readFileSync(csvPath, 'utf8'));
  const lines = ['# 模型定價每日檢查報告', '', '價格來源：各供應商官方定價文件（權威來源）。', ''];
  let drift = 0;
  let errors = 0;
  let added = 0;

  for (const p of sources.providers) {
    lines.push(`## ${p.name}`, '', `官方文件：<${p.pageUrl}>`, '');
    let official;
    try {
      official = parseProvider(p, await fetchText(p.sourceUrl));
      if (official.size === 0) throw new Error('解析不到任何價格，官方頁面格式可能已變更');
    } catch (err) {
      errors++;
      lines.push(`> ⚠️ 無法取得或解析官方價格：${err.message}`, '');
      continue;
    }
    lines.push(`解析到 ${official.size} 個官方模型價格。`, '');
    const { mismatches, promos, missing } = compareProvider(
      csvRows.filter((r) => p.deployments.includes(r.deployment)),
      official,
      p.ignoreModels,
    );
    drift += mismatches.length;
    added += missing.length;
    if (mismatches.length) {
      lines.push('### 價格差異（USD / 1M tokens）', '');
      for (const m of mismatches) lines.push(`- **${m.model}**：${m.diffs.join('；')}`);
      lines.push('');
    }
    if (promos.length) {
      lines.push('### 促銷價提醒', '');
      for (const t of promos) lines.push(`- ${t}`);
      lines.push('');
    }
    if (missing.length) {
      lines.push('### pricing.csv 尚未收錄的新模型', '');
      for (const name of missing) lines.push(`- ${name}`);
      lines.push('');
    }
    if (!mismatches.length && !missing.length) lines.push('與官方價格一致，無差異。', '');
  }

  const report = lines.join('\n');
  if (reportPath) writeFileSync(reportPath, report);
  return { report, drift, errors, added };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const idx = process.argv.indexOf('--report');
  const reportPath = idx > -1 ? process.argv[idx + 1] : undefined;
  const { report, drift, errors, added } = await run({ reportPath });
  console.log(report);
  console.log(`\ndrift=${drift} added=${added} errors=${errors}`);
  if (process.env.GITHUB_OUTPUT) {
    writeFileSync(process.env.GITHUB_OUTPUT, `drift=${drift}\nadded=${added}\nerrors=${errors}\n`, { flag: 'a' });
  }
}
