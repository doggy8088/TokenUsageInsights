import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseCsv, isBaseRow, parsePrice, parseTableProvider, parseGeminiProvider, parseAntigravityProvider, compareProvider, csvKey,
  parseTieredTable, parseContextThresholdK, formatPrice, openaiDisplayName, buildOpenAIRows, insertCsvRows, buildPrBody,
} from '../scripts/pricing-check.mjs';

const csv = `模型名稱,部署類型,單位,輸入價格(美金),快取輸入價格(美金),輸出價格(美金),批次 API 價格(美金)
Gemini 3.8 Flash,Google AI,1M Tokens,1.50,0.15,7.50,N/A
Gemini 3.8 Flash (High),Google AI,1M Tokens,1.50,0.15,7.50,N/A
Gemini 3.1 Pro (<200k),Google AI,1M Tokens,2.00,0.20,12.00,N/A
GPT-6.1 Sol,Global,1M Tokens,2.00,0.10,10.00,N/A
GPT-5.6 Luna,Global,1M Tokens,1.00,0.10,6.00,N/A
`;

const openaiMd = `### Standard pricing data

| Model | Short context input | Short context output | Short context cached input |
| --- | --- | --- | --- |
| gpt-6.1-sol | $2.00 | $10.00 | $0.10 |
| gpt-5.6-luna | $0.20 | $1.20 | $0.02 |
| gpt-5.5 (<272K context length) | $5.00 | $30.00 | $0.50 |
| gpt-9-nova | $1.00 | $2.00 | - |

### Flex pricing data

| Model | Input | Output |
| --- | --- | --- |
| gpt-6.1-sol | $1.00 | $5.00 |
`;

const geminiMd = `## Gemini 3.8 Flash

### Standard

|   | Free Tier | Paid Tier, per 1M tokens in USD |
|---|---|---|
| Input price | Free of charge | $0.75 through December 31, 2026. $1.50 starting January 1, 2027. |
| Output price (including thinking tokens) | Free of charge | $3.75 through December 31, 2026. $7.50 starting January 1, 2027. |
| Context caching price | Free of charge | $0.075 through December 31, 2026. $0.15 starting January 1, 2027. |

### Batch

| | Free Tier | Paid Tier |
|---|---|---|
| Input price | Free of charge | $0.01 |
| Output price | Free of charge | $0.02 |

## Gemini 3.8 Flash TTS

### Standard

| | Free Tier | Paid Tier |
|---|---|---|
| Input price | Free | $1.00 |
| Output price | Free | $2.00 |
`;

test('parseCsv 解析價格與部署類型', () => {
  const rows = parseCsv(csv);
  assert.equal(rows.length, 5);
  assert.equal(rows[3].deployment, 'Global');
});

test('isBaseRow 排除分級列', () => {
  assert.equal(isBaseRow('Gemini 3.1 Pro (<200k)'), false);
  assert.equal(isBaseRow('Gemini 3.5 Flash'), true);
});

test('parsePrice 支援一般價格、"-" 與促銷格式', () => {
  assert.deepEqual(parsePrice('$2.50 / MTok<sup>1</sup>'), { now: 2.5, after: null, until: null });
  assert.equal(parsePrice('-'), null);
  assert.deepEqual(parsePrice('$0.75 through December 31, 2026. $1.50 starting January 1, 2027.'), {
    now: 0.75, after: 1.5, until: 'December 31, 2026',
  });
});

test('parseTableProvider 只讀指定段落並去除上下文後綴', () => {
  const entries = parseTableProvider(openaiMd, '### Standard pricing data');
  assert.equal(entries.size, 4);
  assert.equal(entries.get('gpt 5.5').input.now, 5);
  assert.equal(entries.get('gpt 6.1 sol').input.now, 2); // 不會被 Flex 段落覆蓋
});

test('parseGeminiProvider 只取 Standard 表格並解析促銷價', () => {
  const entries = parseGeminiProvider(geminiMd);
  const flash = entries.get('gemini 3.8 flash');
  assert.equal(flash.input.now, 0.75);
  assert.equal(flash.input.after, 1.5);
  assert.equal(flash.cached.after, 0.15);
});

test('compareProvider 偵測差異、促銷價與新模型', () => {
  const rows = parseCsv(csv);
  const openai = compareProvider(rows.filter((r) => r.deployment === 'Global'), parseTableProvider(openaiMd, '### Standard pricing data'));
  assert.equal(openai.mismatches.length, 1);
  assert.equal(openai.mismatches[0].model, 'GPT-5.6 Luna');
  assert.deepEqual(openai.missing.sort(), ['gpt-5.5', 'gpt-9-nova']);

  const ignored = compareProvider(rows.filter((r) => r.deployment === 'Global'), parseTableProvider(openaiMd, '### Standard pricing data'), ['gpt-5.5', 'gpt-9-nova']);
  assert.deepEqual(ignored.missing, []);

  const google = compareProvider(rows.filter((r) => r.deployment === 'Google AI'), parseGeminiProvider(geminiMd));
  assert.equal(google.mismatches.length, 0); // CSV 採促銷後價格，視為一致
  assert.ok(google.promos.length >= 1);
  assert.deepEqual(google.missing, []); // TTS 屬非文字模型，略過
});

test('stripMarkup 不會殘留巢狀標籤', async () => {
  const { stripMarkup } = await import('../scripts/pricing-check.mjs');
  assert.equal(stripMarkup('Claude <sup>1</sup> [x](http://a)'), 'Claude 1 x');
  assert.ok(!stripMarkup('<scr<script>ipt>').includes('<'));
});

test('csvKey 正規化 Extra High 變體', async () => {
  const { csvKey } = await import('../scripts/pricing-check.mjs');
  assert.equal(csvKey('Grok 4.7 (Extra High)'), csvKey('Grok 4.7'));
});

test('CSV 誤填促銷價時視為差異', () => {
  const promoCsv = parseCsv(`h,h,h,h,h,h,h
Gemini 3.8 Flash,Google AI,1M Tokens,0.75,0.075,3.75,N/A
`);
  const result = compareProvider(promoCsv, parseGeminiProvider(geminiMd));
  assert.equal(result.mismatches.length, 1);
  assert.match(result.mismatches[0].diffs[0], /標準價/);
});

test('parseGeminiProvider 支援 Markdoc tabs/table 與跨行促銷價', () => {
  const markdocGemini = `## Gemini 4 Carbon  {% id="gemini-4-carbon" %}

{% tabs %}
  {% tab title="Standard" %}
  {% table %}
  * Feature
  * Free Tier
  * Paid Tier, per 1M tokens in USD

  ---

  * Input price
  * Not available
  * $2.00 through November 30, 2026.
  $4.00 starting December 1, 2026.

  ---

  * Output price (including thinking tokens)
  * Not available
  * $10.00 through November 30, 2026.
  $20.00 starting December 1, 2026.

  ---

  * Context caching price
  * Not available
  * $0.10 through November 30, 2026.
  $0.20 starting December 1, 2026.
  Implicit caching only.
  {% /table %}
  {% /tab %}

  {% tab title="Batch" %}
  {% table %}
  * Input price
  * Not available
  * $1.00
  {% /table %}
  {% /tab %}
{% /tabs %}
`;
  const entries = parseGeminiProvider(markdocGemini);
  const carbon = entries.get('gemini 4 carbon');
  assert.ok(carbon);
  assert.equal(carbon.name, 'Gemini 4 Carbon');
  assert.equal(carbon.input.now, 2);
  assert.equal(carbon.input.after, 4);
  assert.equal(carbon.output.after, 20);
  assert.equal(carbon.cached.after, 0.2);
});

test('officialKey 正確剝除巢狀括號備註', async () => {
  const { officialKey } = await import('../scripts/pricing-check.mjs');
  assert.equal(
    officialKey('Claude Opus 4.1 ([retired, except on Claude Models on Google Cloud (Google managed) and Bedrock](https://example.com))'),
    'claude opus 4.1',
  );
});

const antigravityModelsMd = `## Reasoning model

| Model | Free & Google AI Plus | Google AI Pro | Google AI Ultra | Enterprise |
| --- | --- | --- | --- | --- |
| [Gemini 4 Carbon](/blog/gemini-4-carbon-in-google-antigravity) | ❌ | ❌ | ✅ | ✅ |
| [Orchestrator (Carbon)](/docs/models#orchestrator) | ❌ | ❌ | ✅ | ❌ |
| [Orchestrator (Argon)](/docs/models#orchestrator) | ❌ | ✅† | ✅ | ❌ |
| [Gemini 3.1 Pro](/blog/gemini-3-1-pro-in-google-antigravity) | ❌ | ❌ | ❌ | ✅ |
| Claude Sonnet 5.5 (thinking)\\*\\* | ❌ | ✅\\*\\* | ✅ | ✅ |
| GPT-OSS-120b\\* | ✅ | ✅ | ✅ | ❌ |

## Orchestrator

| Model selector option | Planner | Implementation | Available on |
| :-- | :-- | :-- | :-- |
| **Orchestrator (Carbon)** | [**Gemini 4 Carbon**](/blog/gemini-4-carbon-in-google-antigravity) | Gemini 4 Hydrogen | Google AI Ultra |
| **Orchestrator (Argon)** | [**Gemini 4 Argon**](/blog/gemini-4-argon-in-google-antigravity) | Gemini 4 Hydrogen | Google AI Ultra |
`;

const antigravityPricingMd = `## Gemini 4 Carbon  {% id="gemini-4-carbon" %}

{% tabs %}
  {% tab title="Standard" %}
  {% table %}
  * Feature
  * Free Tier
  * Paid Tier, per 1M tokens in USD

  ---

  * Input price
  * Not available
  * $2.00 through November 30, 2026. $4.00 starting December 1, 2026.

  ---

  * Output price (including thinking tokens)
  * Not available
  * $10.00 through November 30, 2026. $20.00 starting December 1, 2026.

  ---

  * Context caching price
  * Not available
  * $0.10 through November 30, 2026. $0.20 starting December 1, 2026.
  {% /table %}
  {% /tab %}
{% /tabs %}

## Gemini 4 Argon  {% id="gemini-4-argon" %}

{% tabs %}
  {% tab title="Standard" %}
  {% table %}
  * Feature
  * Free Tier
  * Paid Tier, per 1M tokens in USD

  ---

  * Input price
  * Not available
  * $4.00

  ---

  * Output price (including thinking tokens)
  * Not available
  * $20.00

  ---

  * Context caching price
  * Not available
  * $0.20
  {% /table %}
  {% /tab %}
{% /tabs %}

## Gemini 3.1 Pro Preview  {% id="gemini-3.1-pro-preview" %}

{% tabs %}
  {% tab title="Standard" %}
  {% table %}
  * Feature
  * Free Tier
  * Paid Tier, per 1M tokens in USD

  ---

  * Input price
  * Not available
  * $2.00

  ---

  * Output price (including thinking tokens)
  * Not available
  * $12.00

  ---

  * Context caching price
  * Not available
  * $0.20
  {% /table %}
  {% /tab %}
{% /tabs %}

## Gemini 3.5 Flash-Lite  {% id="gemini-3.5-flash-lite" %}

{% tabs %}
  {% tab title="Standard" %}
  {% table %}
  * Feature
  * Free Tier
  * Paid Tier, per 1M tokens in USD

  ---

  * Input price
  * Free
  * $0.30

  ---

  * Output price (including thinking tokens)
  * Free
  * $2.50

  ---

  * Context caching price
  * Free
  * $0.03
  {% /table %}
  {% /tab %}
{% /tabs %}
`;

test('parseAntigravityProvider 結合 Antigravity 模型表與 Gemini 標準定價', () => {
  const entries = parseAntigravityProvider(antigravityModelsMd, antigravityPricingMd);
  const carbonOrch = entries.get('orchestrator carbon');
  assert.ok(carbonOrch);
  assert.equal(carbonOrch.trackMissing, true);
  assert.equal(carbonOrch.input.now, 2);
  assert.equal(carbonOrch.input.after, 4);
  assert.equal(carbonOrch.output.after, 20);
  assert.equal(carbonOrch.cached.after, 0.2);

  const gemini31 = entries.get('gemini 3.1 pro');
  assert.ok(gemini31);
  assert.equal(gemini31.trackMissing, true);
  assert.equal(gemini31.input.now, 2);
  assert.equal(gemini31.output.now, 12);

  // 非 Antigravity 支援的 Gemini 模型僅作價格比對 (trackMissing: false)，不列入 missing
  const flashLite = entries.get('gemini 3.5 flash lite');
  assert.ok(flashLite);
  assert.equal(flashLite.trackMissing, false);
});

test('compareProvider 搭配 Antigravity 與 priceDeployments 正確篩選缺漏與價格比對', () => {
  const entries = parseAntigravityProvider(antigravityModelsMd, antigravityPricingMd);
  const partialCsv = parseCsv(`h,h,h,h,h,h,h
Gemini 4 Carbon,Google AI,1M Tokens,4.00,0.20,20.00,N/A
Orchestrator (Argon),Google AI,1M Tokens,4.00,0.20,20.00,N/A
Gemini 3.1 Pro,Google AI,1M Tokens,2.00,0.20,12.00,N/A
Claude Sonnet 5.5,Global,1M Tokens,2.00,0.10,10.00,N/A
GPT-OSS 120B (Medium),Local/OSS,1M Tokens,0.00,0.00,0.00,N/A
`);
  const missingResult = compareProvider(partialCsv, entries, [], { priceDeployments: ['Google AI'] });
  assert.deepEqual(missingResult.missing, ['Orchestrator (Carbon)']);
  assert.equal(missingResult.mismatches.length, 0);

  const completeCsv = parseCsv(`h,h,h,h,h,h,h
Gemini 4 Carbon,Google AI,1M Tokens,4.00,0.20,20.00,N/A
Orchestrator (Carbon),Google AI,1M Tokens,4.00,0.20,20.00,N/A
Orchestrator (Argon),Google AI,1M Tokens,4.00,0.20,20.00,N/A
Gemini 3.1 Pro,Google AI,1M Tokens,2.00,0.20,12.00,N/A
Claude Sonnet 5.5,Global,1M Tokens,2.00,0.10,10.00,N/A
GPT-OSS 120B (Medium),Local/OSS,1M Tokens,0.00,0.00,0.00,N/A
`);
  const completeResult = compareProvider(completeCsv, entries, [], { priceDeployments: ['Google AI'] });
  assert.deepEqual(completeResult.missing, []);
  assert.deepEqual(completeResult.mismatches, []);
  assert.ok(completeResult.promos.some((p) => p.includes('orchestrator carbon')));
});

test('csvKey 剝除 Extra/Max 後綴並保留 Orchestrator 括號區分', () => {
  assert.equal(csvKey('Gemini 4 Hydrogen (Extra)'), csvKey('Gemini 4 Hydrogen'));
  assert.equal(csvKey('Claude Opus 5.5 (Max)'), csvKey('Claude Opus 5.5'));
  assert.equal(csvKey('Orchestrator (Carbon)'), 'orchestrator carbon');
  assert.equal(csvKey('Orchestrator (Argon)'), 'orchestrator argon');
  assert.notEqual(csvKey('Orchestrator (Carbon)'), csvKey('Orchestrator (Argon)'));
});

test('parseAntigravityProvider 缺少模型表、部分 Orchestrator 對應或 Planner 價格時拋錯', () => {
  assert.throws(
    () => parseAntigravityProvider('# Empty', antigravityPricingMd),
    /解析不到任何 Antigravity 模型/,
  );
  assert.throws(
    () => parseAntigravityProvider(`## Reasoning model
| Model | Plan |
| --- | --- |
| Orchestrator (Carbon) | Ultra |
`, antigravityPricingMd),
    /解析不到 Antigravity Orchestrator 對應表/,
  );
  assert.throws(
    () => parseAntigravityProvider(`## Reasoning model
| Model | Plan |
| --- | --- |
| Orchestrator (Carbon) | Ultra |
| Orchestrator (Argon) | Ultra |

## Orchestrator
| Model selector option | Planner | Implementation | Available on |
| --- | --- | --- | --- |
| Orchestrator (Carbon) | Gemini 4 Carbon | Gemini 4 Hydrogen | Google AI Ultra |
`, antigravityPricingMd),
    /解析不到 Antigravity Orchestrator 對應表.*Orchestrator \(Argon\)/,
  );
  const carbonOnlyPricingMd = antigravityPricingMd.split('## Gemini 4 Argon')[0];
  assert.throws(
    () => parseAntigravityProvider(antigravityModelsMd, carbonOnlyPricingMd),
    /解析不到 Antigravity Orchestrator「Orchestrator \(Argon\)」.*官方價格/,
  );
});

test('parseAntigravityProvider 支援新版 Role／Supported models 的 Orchestrator 表格', () => {
  const modelsMd = `## Reasoning model

| Model | Free & Google AI Plus | Google AI Pro | Google AI Ultra | Enterprise |
| --- | --- | --- | --- | --- |
| [Gemini 4 Carbon](/blog/gemini-4-carbon-in-google-antigravity) | ❌ | ❌ | ✅ | ✅ |
| [Orchestrator](/docs/models#orchestrator) | ❌ | ✅† | ✅ | ✅‡ |

## Orchestrator

| Role | Responsibilities | Supported models |
| :-- | :-- | :-- |
| **Orchestrator Model** (planner) | Explores the codebase, writes the plan, and reviews progress | [**Gemini 4 Carbon**](#gemini-4-carbon) (default), [**Gemini 4 Argon**](/blog/gemini-4-argon-in-google-antigravity), **Claude Opus 5.5** |
| **Executor Model** | Searches, edits files, runs builds and tests | [**Gemini 4 Hydrogen**](/blog/gemini-4-hydrogen-in-google-antigravity) (default), **Claude Sonnet 5.5** |
`;
  const entries = parseAntigravityProvider(modelsMd, antigravityPricingMd);
  assert.equal(entries.has('orchestrator'), false);
  assert.equal(entries.get('orchestrator carbon').name, 'Orchestrator (Carbon)');
  assert.equal(entries.get('orchestrator carbon').output.after, 20);
  assert.equal(entries.get('orchestrator argon').input.now, 4);
  assert.equal(entries.has('orchestrator claude opus 5.5'), false);
  assert.equal(entries.has('orchestrator hydrogen'), false);

  const csv = parseCsv(`h,h,h,h,h,h,h
Gemini 4 Carbon,Google AI,1M Tokens,4.00,0.20,20.00,N/A
Orchestrator (Carbon),Google AI,1M Tokens,4.00,0.20,20.00,N/A
`);
  const result = compareProvider(csv, entries, [], { priceDeployments: ['Google AI'] });
  assert.deepEqual(result.missing, ['Orchestrator (Argon)']);
  assert.deepEqual(result.mismatches, []);
});

test('parseAntigravityProvider 對未列於 unpricedModels 的一般 Google AI 模型缺價時拋錯', () => {
  const modelsMd = `## Reasoning model
| Model | Plan |
| --- | --- |
| Gemini 4 Hydrogen | Pro |
`;
  const otherPricingMd = `## Gemini 4 Carbon

### Standard

| | Free Tier | Paid Tier |
|---|---|---|
| Input price | Free | $4.00 |
| Output price | Free | $20.00 |
`;
  assert.throws(
    () => parseAntigravityProvider(modelsMd, otherPricingMd),
    /解析不到 Antigravity 模型「Gemini 4 Hydrogen」的官方價格/,
  );
});

test('parseAntigravityProvider 允許 unpricedModels 缺價但仍透過 compareProvider 檢查收錄完整性', () => {
  const modelsMd = `## Reasoning model
| Model | Plan |
| --- | --- |
| Gemini 4 Carbon | Pro |
| Gemini 3.7 Flash | Free |
| Claude Opus 5.5 | Pro |
`;
  const pricingMd = `## Gemini 4 Carbon

### Standard

| | Free Tier | Paid Tier |
|---|---|---|
| Input price | Free | $4.00 |
| Output price | Free | $20.00 |
`;
  const entries = parseAntigravityProvider(modelsMd, pricingMd, {
    unpricedModels: ['Gemini 3.7 Flash'],
  });
  const unpricedEntry = entries.get(csvKey('Gemini 3.7 Flash'));
  assert.deepEqual(unpricedEntry, {
    name: 'Gemini 3.7 Flash',
    input: null,
    output: null,
    cached: null,
    trackMissing: true,
  });

  const csvWithoutUnpriced = parseCsv(`模型名稱,部署類型,單位,輸入價格(美金),快取輸入價格(美金),輸出價格(美金),批次 API 價格(美金)
Gemini 4 Carbon,Google AI,1M Tokens,4.00,,20.00,N/A
Claude Opus 5.5,Global,1M Tokens,5.00,,25.00,N/A
`);
  const missingResult = compareProvider(csvWithoutUnpriced, entries, [], { priceDeployments: ['Google AI'] });
  assert.deepEqual(missingResult.mismatches, []);
  assert.ok(missingResult.missing.some((name) => name.includes('Gemini 3.7 Flash')));

  const completeCsv = parseCsv(`模型名稱,部署類型,單位,輸入價格(美金),快取輸入價格(美金),輸出價格(美金),批次 API 價格(美金)
Gemini 4 Carbon,Google AI,1M Tokens,4.00,,20.00,N/A
Gemini 3.7 Flash,Google AI,1M Tokens,0.10,,0.40,N/A
Claude Opus 5.5,Global,1M Tokens,5.00,,25.00,N/A
`);
  const completeResult = compareProvider(completeCsv, entries, [], { priceDeployments: ['Google AI'] });
  assert.deepEqual(completeResult.mismatches, []);
  assert.deepEqual(completeResult.missing, []);
});

const openaiTieredMd = `### Standard pricing data

| Model | Short context input | Short context output | Short context cached input | Short context 30m write | Short context 12h write | Long context input | Long context output | Long context cached input | Long context 30m write | Long context 12h write |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-6.2-astra | $8.00 | $40.00 | $0.40 | $10.00 | $20.00 | $16.00 | $60.00 | $0.80 | $20.00 | $40.00 |
| gpt-6.2-sol | $2.00 | $10.00 | $0.10 | $2.50 | $5.00 | $4.00 | $15.00 | $0.20 | $5.00 | $10.00 |
| gpt-5.5 (<272K context length) | $5.00 | $30.00 | $0.50 | - | - | $10.00 | $45.00 | $1.00 | - | - |

| Model | Input | Output | Cached input |
| --- | --- | --- | --- |
| gpt-5.4-mini | $0.75 | $4.50 | $0.075 |
| gpt-5.2-pro | $21.00 | $168.00 | - |

### Batch pricing data

| Model | Short context input | Short context output | Short context cached input | Short context 30m write | Short context 12h write | Long context input | Long context output | Long context cached input | Long context 30m write | Long context 12h write |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-6.2-astra | $4.00 | $20.00 | $0.20 | $5.00 | $10.00 | $8.00 | $30.00 | $0.40 | $10.00 | $20.00 |
| gpt-6.2-sol | $1.00 | $5.00 | $0.05 | $1.25 | $2.50 | $2.00 | $7.50 | $0.10 | $2.50 | $5.00 |
`;

test('parseTieredTable 讀取短／長上下文欄位並略過舊格式列', () => {
  const standard = parseTieredTable(openaiTieredMd, '### Standard pricing data');
  assert.deepEqual(standard.get('gpt-6.2-sol'), {
    id: 'gpt-6.2-sol',
    short: { input: 2, output: 10, cached: 0.1 },
    long: { input: 4, output: 15, cached: 0.2 },
  });
  assert.deepEqual(standard.get('gpt-5.4-mini'), { id: 'gpt-5.4-mini', short: { input: 0.75, output: 4.5, cached: 0.075 }, long: null });
  assert.equal(standard.get('gpt-5.2-pro').short.cached, null);
  assert.equal(standard.has('gpt-5.5'), false);
  assert.equal(parseTieredTable(openaiTieredMd, '### Batch pricing data').get('gpt-6.2-astra').long.output, 30);
});

test('parseContextThresholdK、formatPrice 與 openaiDisplayName', () => {
  assert.equal(parseContextThresholdK('- For requests with more than 272K input tokens, input and cache rates double'), 272);
  assert.equal(parseContextThresholdK('no threshold here'), null);
  assert.equal(formatPrice(2), '2.00');
  assert.equal(formatPrice(0.1), '0.10');
  assert.equal(formatPrice(0.075), '0.075');
  assert.equal(formatPrice(0.0375), '0.0375');
  assert.equal(openaiDisplayName('gpt-6.2-sol'), 'GPT-6.2 Sol');
  assert.equal(openaiDisplayName('gpt-7'), 'GPT-7');
  assert.equal(openaiDisplayName('gpt-5.4-mini-high'), 'gpt-5.4-mini-high');
});

test('buildOpenAIRows 產生與人工補價相同的 GPT-6.2 價格列', () => {
  const standard = parseTieredTable(openaiTieredMd, '### Standard pricing data');
  const batch = parseTieredTable(openaiTieredMd, '### Batch pricing data');
  const sol = buildOpenAIRows({ id: 'gpt-6.2-sol', standard: standard.get('gpt-6.2-sol'), batch: batch.get('gpt-6.2-sol'), thresholdK: 272 });
  assert.deepEqual(sol.global, [
    'GPT-6.2 Sol (<272k),Global,1M Tokens,2.00,0.10,10.00,1.00/0.05/5.00',
    'GPT-6.2 Sol (>272k),Global,1M Tokens,4.00,0.20,15.00,2.00/0.10/7.50',
    'GPT-6.2 Sol,Global,1M Tokens,2.00,0.10,10.00,1.00/0.05/5.00',
  ]);
  assert.deepEqual(sol.cursor, ['gpt-6.2-sol,Cursor,1M Tokens,2.00,0.10,10.00,N/A']);
  const astra = buildOpenAIRows({ id: 'gpt-6.2-astra', standard: standard.get('gpt-6.2-astra'), batch: batch.get('gpt-6.2-astra'), thresholdK: 272 });
  assert.equal(astra.global[1], 'GPT-6.2 Astra (>272k),Global,1M Tokens,16.00,0.80,60.00,8.00/0.40/30.00');

  // 沒有長上下文欄位的模型只產生一列；缺 Batch 價時填 N/A。
  const mini = buildOpenAIRows({ id: 'gpt-5.4-mini', standard: standard.get('gpt-5.4-mini') });
  assert.deepEqual(mini.global, ['GPT-5.4 Mini,Global,1M Tokens,0.75,0.075,4.50,N/A']);

  // 資料不完整時不自動產生。
  assert.match(buildOpenAIRows({ id: 'gpt-6.2-sol', standard: standard.get('gpt-6.2-sol') }).reason, /門檻/);
  assert.match(buildOpenAIRows({ id: 'gpt-5.2-pro', standard: standard.get('gpt-5.2-pro') }).reason, /快取輸入/);
  assert.match(buildOpenAIRows({ id: 'gpt-x', standard: undefined }).reason, /標準價/);
});

test('insertCsvRows 將 Global 列放在 GPT 系列最前面、Cursor 列接在最後一個 gpt 列後', () => {
  const csvText = `h,h,h,h,h,h,h
Claude Opus 5.5,Global,1M Tokens,5.00,0.50,25.00,N/A
GPT-6 Astra,Global,1M Tokens,10.00,1.00,50.00,N/A
gpt-6-astra,Cursor,1M Tokens,10.00,1.00,50.00,N/A
gpt-6-luna,Cursor,1M Tokens,0.10,0.01,0.50,N/A
composer-2,Cursor,1M Tokens,1.00,0.10,5.00,N/A
`;
  const out = insertCsvRows(csvText, { global: ['NEW-G1', 'NEW-G2'], cursor: ['new-c'] });
  assert.deepEqual(out.trimEnd().split('\n').slice(1), [
    'Claude Opus 5.5,Global,1M Tokens,5.00,0.50,25.00,N/A',
    'NEW-G1',
    'NEW-G2',
    'GPT-6 Astra,Global,1M Tokens,10.00,1.00,50.00,N/A',
    'gpt-6-astra,Cursor,1M Tokens,10.00,1.00,50.00,N/A',
    'gpt-6-luna,Cursor,1M Tokens,0.10,0.01,0.50,N/A',
    'new-c',
    'composer-2,Cursor,1M Tokens,1.00,0.10,5.00,N/A',
  ]);
  assert.ok(out.endsWith('N/A\n'));
  // 產生的列可被 parseCsv 讀回，且經 csvKey 正規化後與官方模型 ID 相符。
  const sol = buildOpenAIRows({
    id: 'gpt-6.2-sol',
    standard: parseTieredTable(openaiTieredMd, '### Standard pricing data').get('gpt-6.2-sol'),
    thresholdK: 272,
  });
  const rows = parseCsv(insertCsvRows(csvText, sol));
  assert.ok(rows.some((r) => csvKey(r.model) === 'gpt 6.2 sol' && r.deployment === 'Global' && r.cached === 0.1));
});

test('buildPrBody 列出價格列與官方來源', () => {
  const body = buildPrBody(
    [{ id: 'gpt-6.2-sol', name: 'GPT-6.2 Sol', modelPageUrl: 'https://example.test/models/gpt-6.2-sol', global: ['ROW-G'], cursor: ['ROW-C'] }],
    { providers: [{ name: 'OpenAI', pageUrl: 'https://example.test/pricing', autoAdd: {} }] },
  );
  assert.match(body, /### GPT-6\.2 Sol（`gpt-6\.2-sol`）/);
  assert.match(body, /```csv\nROW-G\nROW-C\n```/);
  assert.match(body, /<https:\/\/example\.test\/pricing>/);
});
