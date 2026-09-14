import assert from 'node:assert/strict';
import test from 'node:test';

import { aggregateDailyTokenCandles } from '../static/chart-utils.js';

test('daily candle costs remain isolated for equal session IDs from different sources', () => {
  const sharedIdentity = {
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    session_id: 'shared-session',
  };
  const entries = [
    {
      ...sharedIdentity,
      source_dir_key: 'aa',
      timestamp: '2026-09-14T08:00:00Z',
      turn_no: 1,
      delta_tokens: { input: 80, output: 20, total: 100 },
    },
    {
      ...sharedIdentity,
      source_dir_key: 'bb',
      timestamp: '2026-09-14T09:00:00Z',
      turn_no: 1,
      delta_tokens: { input: 240, output: 60, total: 300 },
    },
  ];
  const sessions = [
    { ...sharedIdentity, source_dir_key: 'aa', total_tokens: 100, cost_usd: 1 },
    { ...sharedIdentity, source_dir_key: 'bb', total_tokens: 300, cost_usd: 6 },
  ];

  const candles = aggregateDailyTokenCandles(
    entries,
    sessions,
    60,
    '2026-09-14',
    new Date('2026-09-15T00:00:00Z')
  );

  assert.equal(candles[8].total, 100);
  assert.equal(candles[8].cost, 1);
  assert.equal(candles[9].total, 300);
  assert.equal(candles[9].cost, 6);
  assert.equal(candles.reduce((total, candle) => total + candle.cost, 0), 7);
});
