import assert from 'node:assert/strict';
import test from 'node:test';

import {
  compareSessionRows,
  filterEntriesBySessionIdentity,
  matchesSessionIdentity,
  parentSessionIdentityKey,
  sessionIdentityKey,
  sessionSearchMatchKey,
} from '../static/session-utils.js';

const mixedTimestampSessions = [
  {
    session_id: 'sql-late',
    timestamp: '2026-07-26 10:00:00',
  },
  {
    session_id: 'iso-early',
    timestamp: '2026-07-26T09:00:00Z',
  },
  {
    session_id: 'offset-middle',
    timestamp: '2026-07-26T17:30:00+08:00',
  },
];

test('session timestamps sort chronologically across supported formats', () => {
  const ascending = [...mixedTimestampSessions]
    .sort((a, b) => compareSessionRows(a, b, 'timestamp', 'asc'));
  const descending = [...mixedTimestampSessions]
    .sort((a, b) => compareSessionRows(a, b, 'timestamp', 'desc'));

  assert.deepEqual(
    ascending.map(session => session.session_id),
    ['iso-early', 'offset-middle', 'sql-late'],
  );
  assert.deepEqual(
    descending.map(session => session.session_id),
    ['sql-late', 'offset-middle', 'iso-early'],
  );
});

test('session sorting preserves numeric and string column behavior', () => {
  const sessions = [
    { session_id: 'b', total_tokens: 2 },
    { session_id: 'a', total_tokens: 1 },
  ];

  assert.deepEqual(
    [...sessions]
      .sort((a, b) => compareSessionRows(a, b, 'total_tokens', 'desc'))
      .map(session => session.session_id),
    ['b', 'a'],
  );
  assert.deepEqual(
    [...sessions]
      .sort((a, b) => compareSessionRows(a, b, 'session_id', 'asc'))
      .map(session => session.session_id),
    ['a', 'b'],
  );
});

test('session identity includes the source directory key', () => {
  const session = {
    session_id: 'shared',
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
  };

  assert.equal(matchesSessionIdentity(session, { ...session }), true);
  assert.equal(
    matchesSessionIdentity(session, { ...session, source_dir_key: 'bb' }),
    false,
  );
  assert.notEqual(
    sessionIdentityKey(session),
    sessionIdentityKey({ ...session, source_kind: 'copilot-cli', source_dir_key: null }),
  );
});

test('parent session identity remains scoped to the child source', () => {
  const child = {
    session_id: 'parent__agent',
    parent_session_id: 'parent',
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
  };
  const parentAsRoot = { ...child, session_id: 'parent', parent_session_id: null };

  assert.equal(
    parentSessionIdentityKey(child),
    sessionIdentityKey(parentAsRoot),
  );
  assert.notEqual(
    parentSessionIdentityKey(child),
    sessionIdentityKey({
      ...parentAsRoot,
      source_dir_key: 'bb',
    }),
  );
});

test('parent session identity links nested sub-agent rows through the sessions list', () => {
  const root = {
    session_id: 'parent-uuid',
    parent_session_id: null,
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const child = {
    session_id: 'agent-a1',
    parent_session_id: 'parent-uuid',
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const grandchild = {
    session_id: 'agent-a2',
    parent_session_id: 'agent-a1',
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const rows = [root, child, grandchild];

  assert.equal(parentSessionIdentityKey(child, rows), sessionIdentityKey(root));
  assert.equal(
    parentSessionIdentityKey(grandchild, rows),
    sessionIdentityKey(child),
  );
});

test('session identity distinguishes standalone session ids from synthetic sub-agent ids', () => {
  // 真實獨立 session 恰好命名為 `a__b`（parent_session_id 為空）與
  // session `a` 的子 Agent `b`（合成 id `a__b`、parent_session_id 為 `a`）
  // 必須是兩個不同的 session 身分。
  const standalone = {
    session_id: 'a__b',
    parent_session_id: null,
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
  };
  const syntheticChild = {
    session_id: 'a__b',
    parent_session_id: 'a',
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
    agent_nickname: 'b',
  };

  assert.notEqual(
    sessionIdentityKey(standalone),
    sessionIdentityKey(syntheticChild),
  );
  assert.equal(matchesSessionIdentity(standalone, { ...standalone }), true);
  assert.equal(
    matchesSessionIdentity(syntheticChild, { ...syntheticChild }),
    true,
  );
  assert.equal(matchesSessionIdentity(standalone, syntheticChild), false);
  assert.equal(matchesSessionIdentity(syntheticChild, standalone), false);

  const entries = [
    { ...standalone, total_tokens: 100 },
    { ...syntheticChild, total_tokens: 900 },
  ];
  assert.deepEqual(
    filterEntriesBySessionIdentity([entries[1]], [standalone]),
    [],
  );
  assert.deepEqual(
    filterEntriesBySessionIdentity(entries, [syntheticChild]),
    [entries[1]],
  );

  // 子 Agent 仍必須能對應到其父層 session `a`。
  const parentRow = {
    session_id: 'a',
    parent_session_id: null,
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
  };
  assert.equal(
    parentSessionIdentityKey(syntheticChild),
    sessionIdentityKey(parentRow),
  );
});

test('raw usage filtering preserves the full session source identity', () => {
  const sessions = [{
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
    session_id: 'shared-session',
  }];
  const entries = [
    { ...sessions[0], total_tokens: 100 },
    { ...sessions[0], source_dir_key: 'bb', total_tokens: 300 },
    { ...sessions[0], assistant_type: 'codex', total_tokens: 500 },
  ];

  assert.deepEqual(filterEntriesBySessionIdentity(entries, sessions), [entries[0]]);
});

test('search match keys keep colliding standalone and synthetic child selectable', () => {
  // 搜尋結果列（後端 SessionSearchMatch）與 sessions 清單列必須共用同一把
  // 身分鍵：獨立 session `a__b` 與 session `a` 的子 Agent `b` 要能各自被
  // 選取（comment 4403291365）；兩列都符合文字時兩列都要保留（搜尋過濾
  // 不漏掉符合列），父子關聯也不受影響。
  const matchRows = [
    {
      session_id: 'a__b',
      parent_session_id: null,
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
    },
    {
      session_id: 'a__b',
      parent_session_id: 'a',
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
    },
  ];

  assert.notEqual(
    sessionSearchMatchKey(matchRows[0]),
    sessionSearchMatchKey(matchRows[1]),
    '搜尋結果的獨立 session 與合成子 Agent 必須是兩個不同的身分鍵',
  );

  // sessions 清單列帶有額外欄位，仍必須與搜尋列算出同一把鍵，Set 比對
  // 才能在清單中各自獨立選取。
  const sessionRows = [
    { ...matchRows[0], session_name: '獨立 session', total_tokens: 100 },
    { ...matchRows[1], agent_nickname: 'b', total_tokens: 900 },
  ];
  for (const match of matchRows) {
    const hitKeys = new Set([sessionSearchMatchKey(match)]);
    const selected = sessionRows.filter(row => hitKeys.has(sessionSearchMatchKey(row)));
    assert.equal(selected.length, 1, '單一搜尋命中只能選到自己的那一列');
    assert.equal(selected[0].parent_session_id ?? null, match.parent_session_id ?? null);
  }

  // 兩列都符合文字時都要保留（搜尋過濾行為不漏掉符合列）。
  const allKeys = new Set(matchRows.map(sessionSearchMatchKey));
  assert.equal(
    sessionRows.filter(row => allKeys.has(sessionSearchMatchKey(row))).length,
    2,
  );

  // 子 Agent 的父子關聯不受影響。
  const parentRow = {
    session_id: 'a',
    parent_session_id: null,
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
  };
  assert.equal(
    parentSessionIdentityKey(matchRows[1], [parentRow]),
    sessionIdentityKey(parentRow),
  );
});
