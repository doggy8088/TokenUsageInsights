#!/usr/bin/env node
// 每日定價檢查：直接解析各供應商「官方定價文件」（以官方站台提供的 Markdown 版本為資料來源），
// 與 pricing.csv 的基礎列（無長上下文分級）比對，回報價格差異、促銷價以及尚未收錄的新模型。
// 預設僅產生報告（Markdown）；設定 autoAdd 的供應商（目前為 OpenAI）可用 --apply 依官方價格表
// 為新模型產生價格列並寫入 pricing.csv，由 workflow 開 PR 供人工審核。價格差異仍需人工處理。
// 若某個官方頁面抓取失敗或解析不到任何價格（通常代表頁面改版），會列為錯誤。
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const EPSILON = 1e-9;
// 非文字模型（圖像、語音、嵌入等）不在 pricing.csv 的追蹤範圍，列新模型時略過。
const NON_TEXT = /tts|image|embedding|live|lyria|transcribe|robotics|veo|imagen|omni|nano banana|gemma|audio|imagine|video|speech/i;
// Antigravity 清單中非 Google 自研（Anthropic Claude / OpenAI OSS）模型不查 Google AI Studio 價表。
const NON_GOOGLE_ANTIGRAVITY_MODEL = /^(?:claude|gpt)(?:\s|$)/i;

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
  return normalizeName(name.replace(/\s*\((?:low|medium|high|xhigh|extra high|extra|max|minimal|thinking|non-thinking|reasoning)\)/gi, ''));
}

// 去除 Markdown 連結與 HTML 標籤（反覆處理直到穩定，最後移除殘留的角括號），僅用於比對與顯示名稱。
export function stripMarkup(text) {
  let out = text.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1');
  let prev;
  do {
    prev = out;
    out = out.replace(/<[^<>]*>/g, '');
  } while (out !== prev);
  return out.replace(/[<>]/g, '');
}

export function officialKey(name) {
  let cleaned = stripMarkup(name).replace(/[^\x20-\x7E]/g, '');
  let prev;
  do {
    prev = cleaned;
    cleaned = cleaned.replace(/\([^()]*\)/g, '');
  } while (cleaned !== prev);
  return normalizeName(cleaned);
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
      name: stripMarkup(cells[0]).replace(/\s*\(.*$/, '').trim(),
      input,
      output,
      cached: cols.cached >= 0 ? parsePrice(cells[cols.cached]) : null,
    });
  }
  return entries;
}

// Google 定價頁以「## 模型名稱 → {% tab title="Standard" %} / {% table %}」（或舊版「### Standard」Markdown 表格）呈現，價格在 Paid Tier 欄。
export function parseGeminiProvider(md) {
  const entries = new Map();
  const lines = md.split(/\r?\n/);
  let model = null;
  let inStandard = false;
  let inMarkdocTable = false;
  let row = {};
  let cells = [];
  const applyRow = (labelText, paidText) => {
    const label = labelText.toLowerCase();
    const paid = paidText.replace(/\s+/g, ' ').trim();
    if (label.startsWith('input price')) row.input = parsePrice(paid);
    else if (label.startsWith('output price')) row.output = parsePrice(paid);
    else if (label.startsWith('context caching price')) row.cached = parsePrice(paid);
  };
  const flushMarkdocRow = () => {
    if (cells.length >= 2) applyRow(cells[0], cells[cells.length - 1]);
    cells = [];
  };
  const flush = () => {
    flushMarkdocRow();
    if (model && row.input && row.output) {
      const key = officialKey(model);
      if (!entries.has(key)) entries.set(key, { name: model, input: row.input, output: row.output, cached: row.cached || null });
    }
    row = {};
  };
  for (const line of lines) {
    const trimmed = line.trim();
    const h2 = line.match(/^##\s+(.+)$/);
    if (h2) {
      flush();
      model = h2[1]
        .replace(/\{%[^%]*%\}/g, '')
        .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
        .replace(/[^\x20-\x7E]/g, '')
        .trim();
      inStandard = false;
      inMarkdocTable = false;
      continue;
    }
    const h3 = line.match(/^###\s+(.+)$/);
    if (h3) {
      if (inStandard) flush();
      inStandard = h3[1].trim() === 'Standard';
      inMarkdocTable = false;
      continue;
    }
    const tabOpen = trimmed.match(/^\{%\s*tab\s+title="([^"]+)"\s*%\}$/);
    if (tabOpen) {
      if (inStandard) flush();
      inStandard = tabOpen[1] === 'Standard';
      inMarkdocTable = false;
      continue;
    }
    if (/^\{%\s*\/tab\s*%\}$/.test(trimmed)) {
      if (inStandard) flush();
      inStandard = false;
      inMarkdocTable = false;
      continue;
    }
    if (!inStandard) continue;
    if (/^\{%\s*table\s*%\}$/.test(trimmed)) {
      inMarkdocTable = true;
      cells = [];
      continue;
    }
    if (/^\{%\s*\/table\s*%\}$/.test(trimmed)) {
      flushMarkdocRow();
      inMarkdocTable = false;
      continue;
    }
    if (inMarkdocTable) {
      if (trimmed === '---') {
        flushMarkdocRow();
      } else if (trimmed.startsWith('* ')) {
        cells.push(trimmed.slice(2).trim());
      } else if (trimmed && cells.length > 0) {
        cells[cells.length - 1] += ` ${trimmed}`;
      }
      continue;
    }
    if (!trimmed.startsWith('|')) continue;
    const mdCells = splitRow(line);
    applyRow(mdCells[0] || '', mdCells[mdCells.length - 1] || '');
  }
  flush();
  return entries;
}

function cleanAntigravityCell(cell) {
  return stripMarkup(cell || '')
    .replace(/\\?\*+/g, '')
    .replace(/[^\x20-\x7E]/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

// 新版 Antigravity 文件不再列出固定的「選項 → Planner」對應表，而是以
// 「Role | Responsibilities | Supported models」表格列出可選的 planner，
// CLI 識別名稱則為 `Orchestrator (Neon)`、`Orchestrator (Carbon)` 等。
// 此處把每個 Gemini planner 展開成 `Orchestrator (<代號>)` 選項；
// 模型表中單獨的 `Orchestrator` 只是選擇器，展開後即不再追蹤。
// Claude 等非 Gemini planner 僅限 App 自訂選擇，沒有對應的 CLI 識別名稱，不展開。
function addRoleTablePlanners(orchLines, antigravityModels, orchestratorPlanners) {
  let cols = null;
  let added = 0;
  for (let i = 0; i < orchLines.length; i++) {
    const line = orchLines[i];
    if (!line.trim().startsWith('|')) { cols = null; continue; }
    const next = orchLines[i + 1] || '';
    if (/^\s*\|?\s*:?-{2,}/.test(next) && !cols) {
      const headers = splitRow(line).map(cleanAntigravityCell);
      const role = headers.findIndex((h) => /^role$/i.test(h));
      const supported = headers.findIndex((h) => /supported models/i.test(h));
      cols = role >= 0 && supported >= 0 ? { role, supported } : null;
      i++; // 略過分隔列
      continue;
    }
    if (!cols) continue;
    const cells = splitRow(line);
    if (!/orchestrator model|planner/i.test(cleanAntigravityCell(cells[cols.role]))) continue;
    for (const part of (cells[cols.supported] || '').split(',')) {
      const planner = cleanAntigravityCell(part).replace(/\s*\([^)]*\)\s*$/, '').trim();
      const codename = planner.match(/^gemini\s+[\d.]+\s+(.+)$/i)?.[1];
      if (!codename) continue;
      const optionName = `Orchestrator (${codename})`;
      const optionKey = csvKey(optionName);
      if (!antigravityModels.has(optionKey)) antigravityModels.set(optionKey, optionName);
      if (!orchestratorPlanners.has(optionKey)) orchestratorPlanners.set(optionKey, csvKey(planner));
      added++;
    }
  }
  if (added > 0 && !orchestratorPlanners.has('orchestrator')) antigravityModels.delete('orchestrator');
}

function parseAntigravityTables(md, modelsHeading, orchestratorHeading) {
  const antigravityModels = new Map();
  let inModelsTable = false;
  const modelLines = sectionLines(md, modelsHeading);
  for (let i = 0; i < modelLines.length; i++) {
    const line = modelLines[i];
    if (!line.trim().startsWith('|')) { inModelsTable = false; continue; }
    const next = modelLines[i + 1] || '';
    if (/^\s*\|?\s*:?-{2,}/.test(next) && !inModelsTable) {
      const headers = splitRow(line);
      inModelsTable = /^model$/i.test(cleanAntigravityCell(headers[0]));
      i++; // 略過分隔列
      continue;
    }
    if (!inModelsTable) continue;
    const cells = splitRow(line);
    const cleaned = cleanAntigravityCell(cells[0]);
    const name = cleaned.replace(/\s*\((?:thinking|non-thinking|reasoning)\)/gi, '').trim();
    if (!name || name === '-') continue;
    const key = csvKey(name);
    if (key && !antigravityModels.has(key)) antigravityModels.set(key, name);
  }
  if (antigravityModels.size === 0) {
    throw new Error('解析不到任何 Antigravity 模型，官方模型頁面格式可能已變更');
  }

  const orchestratorPlanners = new Map();
  let orchCols = null;
  const orchLines = sectionLines(md, orchestratorHeading);
  for (let i = 0; i < orchLines.length; i++) {
    const line = orchLines[i];
    if (!line.trim().startsWith('|')) { orchCols = null; continue; }
    const next = orchLines[i + 1] || '';
    if (/^\s*\|?\s*:?-{2,}/.test(next) && !orchCols) {
      const headers = splitRow(line).map(cleanAntigravityCell);
      const option = headers.findIndex((h) => /model selector option/i.test(h));
      const planner = headers.findIndex((h) => /planner/i.test(h));
      orchCols = option >= 0 && planner >= 0 ? { option, planner } : null;
      i++; // 略過分隔列
      continue;
    }
    if (!orchCols) continue;
    const cells = splitRow(line);
    const optionName = cleanAntigravityCell(cells[orchCols.option]);
    const optionKey = csvKey(optionName);
    const plannerKey = csvKey(cleanAntigravityCell(cells[orchCols.planner]));
    if (optionKey && optionName !== '-') {
      if (!antigravityModels.has(optionKey)) antigravityModels.set(optionKey, optionName);
      if (plannerKey && !orchestratorPlanners.has(optionKey)) {
        orchestratorPlanners.set(optionKey, plannerKey);
      }
    }
  }
  addRoleTablePlanners(orchLines, antigravityModels, orchestratorPlanners);
  const unmappedOrchestrators = [...antigravityModels.entries()]
    .filter(([key]) => /^orchestrator(?:\s|$)/i.test(key) && !orchestratorPlanners.has(key))
    .map(([, name]) => name);
  if (unmappedOrchestrators.length > 0) {
    throw new Error(
      `解析不到 Antigravity Orchestrator 對應表（缺少：${unmappedOrchestrators.join('、')}），官方模型頁面格式可能已變更`,
    );
  }

  return { antigravityModels, orchestratorPlanners };
}

// Antigravity 官方模型頁列出支援的推理模型與 Orchestrator 對應，價格取自 Google AI Studio 官方定價文件。
export function parseAntigravityProvider(modelsMd, pricingMd, options = {}) {
  const {
    modelsHeading = '## Reasoning model',
    orchestratorHeading = '## Orchestrator',
    unpricedModels = [],
  } = options;
  const { antigravityModels, orchestratorPlanners } = parseAntigravityTables(modelsMd, modelsHeading, orchestratorHeading);
  const geminiPrices = parseGeminiProvider(pricingMd);
  if (geminiPrices.size === 0) {
    throw new Error('解析不到任何價格，官方定價頁面格式可能已變更');
  }

  const unpriced = new Set(unpricedModels.flatMap((m) => [officialKey(m), csvKey(m)]));
  const resolveGeminiPrice = (key) => {
    const targetKey = orchestratorPlanners.get(key) || key;
    return geminiPrices.get(targetKey) || geminiPrices.get(`${targetKey} preview`) || null;
  };

  const entries = new Map();
  for (const [key, name] of antigravityModels) {
    const isOrchestrator = /^orchestrator(?:\s|$)/i.test(key);
    const plannerKey = orchestratorPlanners.get(key);
    if (isOrchestrator && !plannerKey) {
      throw new Error(
        `解析不到 Antigravity Orchestrator 對應表（缺少：${name}），官方模型頁面格式可能已變更`,
      );
    }
    const price = resolveGeminiPrice(key);
    if (isOrchestrator && (!price?.input || !price?.output)) {
      throw new Error(
        `解析不到 Antigravity Orchestrator「${name}」（Planner: ${plannerKey}）的官方價格，定價頁面格式可能已變更`,
      );
    }
    if (!isOrchestrator && !NON_GOOGLE_ANTIGRAVITY_MODEL.test(key) && !unpriced.has(key) && (!price?.input || !price?.output)) {
      throw new Error(
        `解析不到 Antigravity 模型「${name}」的官方價格，定價頁面格式可能已變更`,
      );
    }
    entries.set(key, {
      name,
      input: price?.input ?? null,
      output: price?.output ?? null,
      cached: price?.cached ?? null,
      trackMissing: true,
    });
  }
  for (const [key, price] of geminiPrices) {
    if (!entries.has(key)) {
      entries.set(key, { ...price, trackMissing: false });
    }
  }
  return entries;
}

export function parseProvider(provider, md) {
  if (provider.parser === 'antigravity') {
    const { modelsMd, pricingMd } = typeof md === 'object' && md !== null ? md : {};
    return parseAntigravityProvider(modelsMd, pricingMd, {
      modelsHeading: provider.modelsSectionHeading,
      orchestratorHeading: provider.orchestratorSectionHeading,
      unpricedModels: provider.unpricedModels,
    });
  }
  return provider.parser === 'gemini' ? parseGeminiProvider(md) : parseTableProvider(md, provider.sectionHeading);
}

// ---- 新模型自動產生價格列（目前僅供 OpenAI 使用，見 pricing-sources.json 的 autoAdd） ----

// 只接受單一明確金額（"$2.00"）；"-"、促銷文字或其他格式一律視為無法自動產生。
function plainPrice(cell) {
  const m = (cell || '').trim().match(/^\$\s*([\d,]*\.?\d+)$/);
  return m ? Number(m[1].replace(/,/g, '')) : null;
}

// 解析 OpenAI 價格表的完整欄位（短上下文與長上下文），以小寫模型 ID 為鍵。
// 帶有括號備註的列（如 "gpt-5.5 (<272K context length)"）為舊格式，不作自動產生。
export function parseTieredTable(md, heading) {
  const lines = sectionLines(md, heading);
  const entries = new Map();
  let cols = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim().startsWith('|')) { cols = null; continue; }
    const next = lines[i + 1] || '';
    if (/^\s*\|?\s*:?-{2,}/.test(next) && !cols) {
      const headers = splitRow(line);
      const find = (re) => headers.findIndex((h) => re.test(h));
      const shortPrefix = headers.some((h) => /^short context/i.test(h)) ? 'short context ' : '';
      cols = {
        short: {
          input: find(new RegExp(`^${shortPrefix}input$`, 'i')),
          output: find(new RegExp(`^${shortPrefix}output$`, 'i')),
          cached: find(new RegExp(`^${shortPrefix}cached input$`, 'i')),
        },
        long: {
          input: find(/^long context input$/i),
          output: find(/^long context output$/i),
          cached: find(/^long context cached input$/i),
        },
      };
      i++; // 略過分隔列
      continue;
    }
    if (!cols) continue;
    const cells = splitRow(line);
    const id = stripMarkup(cells[0] || '').trim().toLowerCase();
    if (!/^[a-z0-9][a-z0-9.-]*$/.test(id) || entries.has(id)) continue;
    const pick = (c) => ({
      input: c.input >= 0 ? plainPrice(cells[c.input]) : null,
      output: c.output >= 0 ? plainPrice(cells[c.output]) : null,
      cached: c.cached >= 0 ? plainPrice(cells[c.cached]) : null,
    });
    const long = pick(cols.long);
    entries.set(id, { id, short: pick(cols.short), long: long.input != null || long.output != null ? long : null });
  }
  return entries;
}

// 從官方模型頁讀取長上下文門檻，例如 "more than 272K input tokens" → 272。
export function parseContextThresholdK(modelMd) {
  const m = (modelMd || '').match(/more than\s+([\d,]+)\s*K\s+input tokens/i);
  return m ? Number(m[1].replace(/,/g, '')) : null;
}

// CSV 慣例：至少兩位小數，需要更多位數時保留原值（0.075、0.0375）。
export function formatPrice(n) {
  const fixed = n.toFixed(2);
  return Number(fixed) === n ? fixed : String(n);
}

// gpt-6.2-sol → GPT-6.2 Sol；gpt-7 → GPT-7；其他形狀維持官方 ID（計價比對時會正規化名稱）。
export function openaiDisplayName(id) {
  const m = id.match(/^gpt-(\d+(?:\.\d+)*)(?:-([a-z]+))?$/);
  if (!m) return id;
  return m[2] ? `GPT-${m[1]} ${m[2][0].toUpperCase()}${m[2].slice(1)}` : `GPT-${m[1]}`;
}

const complete = (p) => p && p.input != null && p.output != null && p.cached != null;
const batchCell = (p) => (complete(p) ? `${formatPrice(p.input)}/${formatPrice(p.cached)}/${formatPrice(p.output)}` : 'N/A');
const priceCells = (p) => `${formatPrice(p.input)},${formatPrice(p.cached)},${formatPrice(p.output)}`;

// 依官方價格產生 pricing.csv 價格列；資料不完整時回傳 { reason } 交由人工處理。
export function buildOpenAIRows({ id, standard, batch, thresholdK, deployment = 'Global', cursor = true }) {
  if (!standard || !complete(standard.short)) return { reason: '官方標準價缺少輸入、快取輸入或輸出價格' };
  const name = openaiDisplayName(id);
  const unit = '1M Tokens';
  const global = [];
  if (standard.long) {
    if (!complete(standard.long)) return { reason: '官方長上下文價格不完整' };
    if (!thresholdK) return { reason: '官方模型頁找不到長上下文門檻' };
    global.push(
      `${name} (<${thresholdK}k),${deployment},${unit},${priceCells(standard.short)},${batchCell(batch?.short)}`,
      `${name} (>${thresholdK}k),${deployment},${unit},${priceCells(standard.long)},${batchCell(batch?.long)}`,
    );
  }
  global.push(`${name},${deployment},${unit},${priceCells(standard.short)},${batchCell(batch?.short)}`);
  return {
    name,
    global,
    cursor: cursor ? [`${id},Cursor,${unit},${priceCells(standard.short)},N/A`] : [],
  };
}

// 新的 Global 列插在同部署第一個同前綴（如 "GPT-"）列之前，讓最新模型排在該系列最前面；
// Cursor 列接在最後一個同前綴 Cursor 列之後。找不到位置時附加於檔尾。
export function insertCsvRows(csvText, { global = [], cursor = [], globalPrefix = 'GPT-', deployment = 'Global' }) {
  const lines = csvText.replace(/\n$/, '').split('\n');
  const lower = globalPrefix.toLowerCase();
  if (global.length) {
    const at = lines.findIndex((l) => l.startsWith(globalPrefix) && l.split(',')[1] === deployment);
    lines.splice(at < 0 ? lines.length : at, 0, ...global);
  }
  if (cursor.length) {
    let last = -1;
    lines.forEach((l, i) => { if (l.toLowerCase().startsWith(lower) && l.split(',')[1] === 'Cursor') last = i; });
    lines.splice(last < 0 ? lines.length : last + 1, 0, ...cursor);
  }
  return `${lines.join('\n')}\n`;
}

// pricing.csv 依慣例記錄標準牌價：有促銷時以促銷後價格（after）為準，促銷價僅作報告提醒。
const standardPrice = (p) => (p.after != null ? p.after : p.now);
const matchesPrice = (local, p) => Math.abs(local - standardPrice(p)) <= EPSILON;

export function compareProvider(csvRows, official, ignoreModels = [], options = {}) {
  const { priceDeployments } = options;
  const ignored = new Set(ignoreModels.flatMap((m) => [officialKey(m), csvKey(m)]));
  const mismatches = [];
  const promos = [];
  const known = new Set();
  for (const row of csvRows) {
    known.add(csvKey(row.model));
    if (priceDeployments && !priceDeployments.includes(row.deployment)) continue;
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
        diffs.push(`${label}：CSV ${local} → 官方標準價 ${standardPrice(price)}${price.after != null ? `（目前促銷價 ${price.now}，至 ${price.until}）` : ''}`);
      } else if (price.after != null) {
        promos.push(`${csvKey(row.model)} ${label}：官方目前促銷價 ${price.now}，${price.until} 後為 ${price.after}（CSV 已採後者）`);
      }
    }
    if (diffs.length) mismatches.push({ model: row.model, diffs });
  }
  const missing = [...official.entries()]
    .filter(([key, e]) => e.trackMissing !== false && !known.has(key) && !ignored.has(key) && !NON_TEXT.test(e.name))
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

// 針對設定 autoAdd 的供應商，為尚未收錄的新模型產生價格列；資料不完整的模型回傳原因，留給人工處理。
async function buildAutoAddRows(p, md, missing, fetcher) {
  const standard = parseTieredTable(md, p.sectionHeading);
  const batch = p.autoAdd.batchSectionHeading ? parseTieredTable(md, p.autoAdd.batchSectionHeading) : new Map();
  const generated = [];
  const skipped = [];
  for (const id of missing) {
    const key = id.toLowerCase();
    let thresholdK = null;
    if (standard.get(key)?.long) {
      try {
        thresholdK = parseContextThresholdK(await fetcher(p.autoAdd.modelPageUrl.replace('{id}', encodeURIComponent(key))));
      } catch {
        thresholdK = null;
      }
    }
    const result = buildOpenAIRows({ id: key, standard: standard.get(key), batch: batch.get(key), thresholdK });
    if (result.reason) skipped.push({ id, reason: result.reason });
    else generated.push({ id, modelPageUrl: p.autoAdd.modelPageUrl.replace('{id}', key).replace(/\.md$/, ''), ...result });
  }
  return { generated, skipped };
}

export async function run({
  csvPath = path.join(root, 'pricing.csv'),
  reportPath,
  apply = false,
  fetcher = fetchText,
} = {}) {
  const sources = JSON.parse(readFileSync(path.join(root, 'scripts', 'pricing-sources.json'), 'utf8'));
  const csvText = readFileSync(csvPath, 'utf8');
  const csvRows = parseCsv(csvText);
  const lines = ['# 模型定價每日檢查報告', '', '價格來源：各供應商官方定價文件（權威來源）。', ''];
  let drift = 0;
  let errors = 0;
  let added = 0;
  const generated = [];

  for (const p of sources.providers) {
    if (p.pricingPageUrl) {
      lines.push(`## ${p.name}`, '', `官方模型清單：<${p.pageUrl}>`, `官方定價文件：<${p.pricingPageUrl}>`, '');
    } else {
      lines.push(`## ${p.name}`, '', `官方文件：<${p.pageUrl}>`, '');
    }
    let official;
    let sourceMd = null;
    try {
      if (p.parser === 'antigravity') {
        const [modelsMd, pricingMd] = await Promise.all([fetcher(p.sourceUrl), fetcher(p.pricingSourceUrl)]);
        official = parseAntigravityProvider(modelsMd, pricingMd, {
          modelsHeading: p.modelsSectionHeading,
          orchestratorHeading: p.orchestratorSectionHeading,
          unpricedModels: p.unpricedModels,
        });
      } else {
        sourceMd = await fetcher(p.sourceUrl);
        official = parseProvider(p, sourceMd);
      }
      if (official.size === 0) throw new Error('解析不到任何價格，官方頁面格式可能已變更');
    } catch (err) {
      errors++;
      lines.push(`> ⚠️ 無法取得或解析官方價格：${err.message}`, '');
      continue;
    }
    if (p.parser === 'antigravity') {
      const tracked = [...official.values()].filter((e) => e.trackMissing !== false);
      const pricedCount = tracked.filter((e) => e.input && e.output).length;
      lines.push(`解析到 ${tracked.length} 個 Antigravity 官方支援模型（其中 ${pricedCount} 個對應 Gemini 官方標準價）。`, '');
    } else {
      lines.push(`解析到 ${official.size} 個官方模型價格。`, '');
    }
    const { mismatches, promos, missing: allMissing } = compareProvider(
      csvRows.filter((r) => p.deployments.includes(r.deployment)),
      official,
      p.ignoreModels,
      { priceDeployments: p.priceDeployments },
    );
    let missing = allMissing;
    let auto = { generated: [], skipped: [] };
    if (p.autoAdd && sourceMd && allMissing.length) {
      auto = await buildAutoAddRows(p, sourceMd, allMissing, fetcher);
      const generatedIds = new Set(auto.generated.map((g) => g.id));
      missing = allMissing.filter((id) => !generatedIds.has(id));
      generated.push(...auto.generated);
    }
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
    if (auto.generated.length) {
      lines.push(`### 已依官方價格自動產生價格列${apply ? '（已寫入 pricing.csv）' : ''}`, '');
      for (const g of auto.generated) lines.push(`- ${g.id}（${g.global.length + g.cursor.length} 筆）`);
      lines.push('');
    }
    if (missing.length) {
      lines.push('### pricing.csv 尚未收錄的新模型', '');
      const reasons = new Map(auto.skipped.map((s) => [s.id, s.reason]));
      for (const name of missing) lines.push(`- ${name}${reasons.has(name) ? `（無法自動產生：${reasons.get(name)}）` : ''}`);
      lines.push('');
    }
    if (!mismatches.length && !missing.length && !auto.generated.length) lines.push('與官方價格一致，無差異。', '');
  }

  if (apply && generated.length) {
    // 一次插入，維持官方價格表的模型順序。
    writeFileSync(csvPath, insertCsvRows(csvText, {
      global: generated.flatMap((g) => g.global),
      cursor: generated.flatMap((g) => g.cursor),
    }));
  }

  const report = lines.join('\n');
  if (reportPath) writeFileSync(reportPath, report);
  return { report, drift, errors, added, generated };
}

// 自動更新 PR 的內文：列出每個模型產生的價格列與官方來源，供審核時逐列對照。
export function buildPrBody(generated, sources = JSON.parse(readFileSync(path.join(root, 'scripts', 'pricing-sources.json'), 'utf8'))) {
  const pricingPage = sources.providers.find((p) => p.autoAdd)?.pageUrl;
  const lines = [
    '## 自動新增模型定價',
    '',
    '每日定價檢查在官方定價文件中偵測到 `pricing.csv` 尚未收錄的新模型，並依官方價格表自動產生下列價格列。',
    '',
  ];
  for (const g of generated) {
    lines.push(`### ${g.name}（\`${g.id}\`）`, '', `- 官方模型頁：<${g.modelPageUrl}>`, '', '```csv', ...g.global, ...g.cursor, '```', '');
  }
  lines.push(
    '## 審核清單',
    '',
    `- [ ] 對照官方定價文件（<${pricingPage}>）確認短上下文、長上下文與 Batch 價格`,
    '- [ ] 確認長上下文門檻與官方模型頁一致',
    '- [ ] 視需要調整顯示名稱，並在 `CHANGELOG.md` 的未發行區補上紀錄',
    '',
    '此 PR 由 Daily Pricing Check workflow 建立；workflow 已在建立前執行 `node --test tests/pricing-check.test.mjs` 與 `cargo test --locked pricing`。',
    '',
  );
  return lines.join('\n');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const arg = (name) => {
    const idx = process.argv.indexOf(name);
    return idx > -1 ? process.argv[idx + 1] : undefined;
  };
  const reportPath = arg('--report');
  const prBodyPath = arg('--pr-body');
  const csvPath = arg('--csv');
  const apply = process.argv.includes('--apply');
  const { report, drift, errors, added, generated } = await run({
    reportPath,
    apply,
    ...(csvPath ? { csvPath: path.resolve(csvPath) } : {}),
  });
  console.log(report);
  const applied = apply ? generated.length : 0;
  console.log(`\ndrift=${drift} added=${added} errors=${errors} generated=${generated.length} applied=${applied}`);
  if (prBodyPath && applied) writeFileSync(prBodyPath, buildPrBody(generated));
  if (process.env.GITHUB_OUTPUT) {
    writeFileSync(
      process.env.GITHUB_OUTPUT,
      `drift=${drift}\nadded=${added}\nerrors=${errors}\napplied=${applied}\napplied_models=${generated.map((g) => g.id).join(' ')}\n`,
      { flag: 'a' },
    );
  }
}
