import test from 'node:test';
import assert from 'node:assert/strict';
import { parseCsv, isBaseRow, compareProvider } from '../scripts/pricing-check.mjs';

const csv = `模型名稱,部署類型,單位,輸入價格(美金),快取輸入價格(美金),輸出價格(美金),批次 API 價格(美金)
Gemini 3.5 Flash,Google AI,1M Tokens,1.50,0.375,9.00,N/A
Gemini 3.1 Pro (<200k),Google AI,1M Tokens,2.00,0.20,12.00,N/A
`;

test('parseCsv 解析價格欄位', () => {
  const rows = parseCsv(csv);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].cached, 0.375);
});

test('isBaseRow 排除分級列', () => {
  assert.equal(isBaseRow('Gemini 3.1 Pro (<200k)'), false);
  assert.equal(isBaseRow('Gemini 3.5 Flash'), true);
});

test('compareProvider 偵測差異與新模型', () => {
  const rows = parseCsv(csv);
  const result = compareProvider(rows, {
    a: { name: 'Gemini 3.5 Flash', cost: { input: 1.5, output: 9, cache_read: 0.15 } },
    b: { name: 'Gemini 9 Nova', cost: { input: 1, output: 2 } },
  });
  assert.equal(result.mismatches.length, 1);
  assert.match(result.mismatches[0].diffs[0], /快取輸入/);
  assert.deepEqual(result.missing, ['Gemini 9 Nova']);
});

test('parseCsv 保留部署類型供篩選', () => {
  assert.equal(parseCsv(csv)[0].deployment, 'Google AI');
});
