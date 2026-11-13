import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseLitellmJson, isBaseRow, parsePrice, parseTableProvider, parseGeminiProvider, compareProvider,
} from '../scripts/pricing-check.mjs';

// 以 [模型, litellm_provider, 輸入, 快取輸入, 輸出]（USD / 1M tokens）組出 LiteLLM 格式 JSON。
const litellmJson = (entries) => JSON.stringify({
  sample_spec: { litellm_provider: 'one of https://docs.litellm.ai/docs/providers', mode: 'one of chat' },
  ...Object.fromEntries(entries.map(([model, provider, input, cached, output]) => [
    `${provider}/${model}`,
    {
      litellm_provider: provider,
      mode: 'chat',
      input_cost_per_token: input / 1e6,
      cache_read_input_token_cost: cached / 1e6,
      output_cost_per_token: output / 1e6,
    },
  ])),
  'bedrock/gpt-6.1-sol': { litellm_provider: 'bedrock', mode: 'chat', input_cost_per_token: 9e-6 },
});

const pricingJson = litellmJson([
  ['gemini-3.8-flash', 'gemini', 1.5, 0.15, 7.5],
  ['gemini-3.1-pro', 'gemini', 2, 0.2, 12],
  ['gpt-6.1-sol', 'openai', 2, 0.1, 10],
  ['gpt-5.6-luna', 'openai', 1, 0.1, 6],
]);

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

test('parseLitellmJson 只取原廠直連條目並換算為每百萬 Token 價格', () => {
  const rows = parseLitellmJson(pricingJson);
  assert.equal(rows.length, 4); // sample_spec 與 bedrock 轉售價不納入
  const sol = rows.find((r) => r.model === 'gpt-6.1-sol');
  assert.equal(sol.deployment, 'Global');
  assert.deepEqual([sol.input, sol.cached, sol.output], [2, 0.1, 10]);
  assert.equal(rows.find((r) => r.model === 'gemini-3.8-flash').deployment, 'Google AI');
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
  const rows = parseLitellmJson(pricingJson);
  const openai = compareProvider(rows.filter((r) => r.deployment === 'Global'), parseTableProvider(openaiMd, '### Standard pricing data'));
  assert.equal(openai.mismatches.length, 1);
  assert.equal(openai.mismatches[0].model, 'gpt-5.6-luna');
  assert.deepEqual(openai.missing.sort(), ['gpt-5.5', 'gpt-9-nova']);

  const ignored = compareProvider(rows.filter((r) => r.deployment === 'Global'), parseTableProvider(openaiMd, '### Standard pricing data'), ['gpt-5.5', 'gpt-9-nova']);
  assert.deepEqual(ignored.missing, []);

  const google = compareProvider(rows.filter((r) => r.deployment === 'Google AI'), parseGeminiProvider(geminiMd));
  assert.equal(google.mismatches.length, 0); // 價格檔採促銷後價格，視為一致
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

test('價格檔誤填促銷價時視為差異', () => {
  const promoRows = parseLitellmJson(litellmJson([['gemini-3.8-flash', 'gemini', 0.75, 0.075, 3.75]]));
  const result = compareProvider(promoRows, parseGeminiProvider(geminiMd));
  assert.equal(result.mismatches.length, 1);
  assert.match(result.mismatches[0].diffs[0], /標準價/);
});
