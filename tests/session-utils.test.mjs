import assert from 'node:assert/strict';
import test from 'node:test';

import {
  buildSessionParentIndex,
  compareSessionRows,
  filterEntriesBySessionIdentity,
  matchesSessionIdentity,
  nextSessionSearchDisplayState,
  nextSessionSearchRunState,
  nextSessionSearchSearchedFingerprint,
  parentSessionIdentityKey,
  rootSessionIdentityKey,
  sessionIdentityKey,
  sessionSearchDataFingerprint,
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

test('session parent index matches uncached lookups across edge fixtures', () => {
  // 覆蓋：獨立 `a__b`、合成子 Agent、巢狀鏈、父列缺席、多候選歧義、
  // 重複列、成環（含環前尾巴）。快取／索引結果必須與未快取的
  // parentSessionIdentityKey／rootSessionIdentityKey 完全一致。
  const standalone = {
    session_id: 'a__b',
    parent_session_id: null,
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
  };
  const parentRow = {
    session_id: 'a',
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
  const chainRoot = {
    session_id: 'parent-uuid',
    parent_session_id: null,
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const chainChild = {
    session_id: 'agent-a1',
    parent_session_id: 'parent-uuid',
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const chainGrandchild = {
    session_id: 'agent-a2',
    parent_session_id: 'agent-a1',
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const orphan = {
    session_id: 'orphan',
    parent_session_id: 'ghost',
    assistant_type: 'claude',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  // 多候選歧義：兩列同 session_id `p`、同來源三欄，父層解析只能退回
  // 「父層為 root」鍵。
  const ambiguousOne = {
    session_id: 'p',
    parent_session_id: 'x',
    assistant_type: 'pi',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const ambiguousTwo = {
    session_id: 'p',
    parent_session_id: 'y',
    assistant_type: 'pi',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const ambiguousChild = {
    session_id: 'p__c',
    parent_session_id: 'p',
    assistant_type: 'pi',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  // 重複列（同身分鍵兩列）依舊版語意算「多候選」→ parentAsRootKey，
  // 索引不得去重後誤走「唯一候選」分支。
  const duplicateParent = {
    session_id: 'dup',
    parent_session_id: 'z',
    assistant_type: 'mcode',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const duplicateChild = {
    session_id: 'dup__c',
    parent_session_id: 'dup',
    assistant_type: 'mcode',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  // 成環 a1 ⇄ a2，外加環前尾巴 tail → a1。
  const cycleA1 = {
    session_id: 'a1',
    parent_session_id: 'a2',
    assistant_type: 'grok',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const cycleA2 = {
    session_id: 'a2',
    parent_session_id: 'a1',
    assistant_type: 'grok',
    source_kind: 'legacy',
    source_dir_key: null,
  };
  const cycleTail = {
    session_id: 'tail',
    parent_session_id: 'a1',
    assistant_type: 'grok',
    source_kind: 'legacy',
    source_dir_key: null,
  };

  const sessions = [
    standalone,
    parentRow,
    syntheticChild,
    chainRoot,
    chainChild,
    chainGrandchild,
    orphan,
    ambiguousOne,
    ambiguousTwo,
    ambiguousChild,
    duplicateParent,
    { ...duplicateParent },
    duplicateChild,
    cycleA1,
    cycleA2,
    cycleTail,
  ];

  const index = buildSessionParentIndex(sessions);
  for (const row of sessions) {
    assert.equal(
      index.parentKeyOf(row),
      parentSessionIdentityKey(row, sessions),
      `parentKeyOf 與未快取結果不一致: ${row.session_id}`,
    );
    assert.equal(
      index.rootKeyOf(row),
      rootSessionIdentityKey(row, sessions),
      `rootKeyOf 與未快取結果不一致: ${row.session_id}`,
    );
    // 重複查找結果必須穩定（快取不得改變語意）。
    assert.equal(index.parentKeyOf(row), parentSessionIdentityKey(row, sessions));
    assert.equal(index.rootKeyOf(row), rootSessionIdentityKey(row, sessions));
  }

  // 關鍵語意鎖定（獨立於兩個實作的固定值）：
  // 獨立 session `a__b` 與合成子 Agent `a__b` 是兩個身分，但子 Agent 仍
  // 必須連回父列的完整 5 元鍵（f743b2e／1043bcd 不變量）。
  assert.notEqual(sessionIdentityKey(standalone), sessionIdentityKey(syntheticChild));
  assert.equal(index.parentKeyOf(syntheticChild), sessionIdentityKey(parentRow));
  assert.equal(index.rootKeyOf(syntheticChild), sessionIdentityKey(parentRow));
  assert.equal(index.rootKeyOf(standalone), sessionIdentityKey(standalone));

  // 巢狀鏈：孫列連到「父列的完整身分鍵」（含父列自己的 parent_session_id）。
  assert.equal(index.parentKeyOf(chainGrandchild), sessionIdentityKey(chainChild));
  assert.equal(index.rootKeyOf(chainGrandchild), sessionIdentityKey(chainRoot));

  // 父列缺席 → 「父層為 root」鍵；根節點仍是自己。
  assert.equal(
    index.parentKeyOf(orphan),
    sessionIdentityKey({ ...orphan, session_id: 'ghost', parent_session_id: null }),
  );
  assert.equal(index.rootKeyOf(orphan), sessionIdentityKey(orphan));

  // 多候選歧義與重複列 → parentAsRootKey。
  assert.equal(
    index.parentKeyOf(ambiguousChild),
    sessionIdentityKey({ ...ambiguousChild, session_id: 'p', parent_session_id: null }),
  );
  assert.equal(
    index.parentKeyOf(duplicateChild),
    sessionIdentityKey({ ...duplicateChild, session_id: 'dup', parent_session_id: null }),
  );

  // 成環：環上每個節點（含環前尾巴）都結案到環上最小字序身分鍵
  // （getRootParentKey 既有處理，app.js hover 依賴此行為）。
  const cycleMin = [sessionIdentityKey(cycleA1), sessionIdentityKey(cycleA2)]
    .sort((a, b) => a.localeCompare(b))[0];
  assert.equal(index.rootKeyOf(cycleA1), cycleMin);
  assert.equal(index.rootKeyOf(cycleA2), cycleMin);
  assert.equal(index.rootKeyOf(cycleTail), cycleMin);
});

test('session parent index lookups never rescan the sessions array', () => {
  // 以 Proxy 計算 sessions 陣列的「元素讀取次數」（掃描次數）作為決定性
  // 量測：建索引 O(N) 走訪一次，之後每次 parentKeyOf／rootKeyOf 都是
  // O(1) 查表、0 次陣列走訪。實作退化回線性掃描（f743b2e 的二次方回歸，
  // comment 4436837586）即紅燈。不使用任何計時斷言。
  const rows = [
    {
      session_id: 'parent',
      parent_session_id: null,
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
    },
    ...Array.from({ length: 6 }, (_, i) => ({
      session_id: `parent__agent-${i}`,
      parent_session_id: 'parent',
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
    })),
    {
      session_id: 'a1',
      parent_session_id: 'a2',
      assistant_type: 'grok',
      source_kind: 'legacy',
      source_dir_key: null,
    },
    {
      session_id: 'a2',
      parent_session_id: 'a1',
      assistant_type: 'grok',
      source_kind: 'legacy',
      source_dir_key: null,
    },
    {
      session_id: 'tail',
      parent_session_id: 'a1',
      assistant_type: 'grok',
      source_kind: 'legacy',
      source_dir_key: null,
    },
  ];

  let elementReads = 0;
  let iterations = 0;
  const proxy = new Proxy(rows, {
    get(target, prop, receiver) {
      if (typeof prop === 'string' && /^(0|[1-9][0-9]*)$/.test(prop)) {
        elementReads += 1;
      }
      if (prop === Symbol.iterator) {
        iterations += 1;
      }
      return Reflect.get(target, prop, receiver);
    },
  });

  const index = buildSessionParentIndex(proxy);
  const buildReads = elementReads;
  assert.equal(iterations, 1, '建索引只應走訪 sessions 一次');
  assert.ok(
    buildReads <= rows.length * 2,
    `建索引應為 O(N)，實測 ${buildReads} 次元素讀取（N=${rows.length}）`,
  );

  const readsAfterBuild = elementReads;
  for (let round = 0; round < 1000; round += 1) {
    for (const row of rows) {
      index.parentKeyOf(row);
      index.rootKeyOf(row);
    }
  }
  assert.equal(
    elementReads,
    readsAfterBuild,
    '每次 parentKeyOf／rootKeyOf 都必須 O(1) 查表，不得重新掃描 sessions',
  );
  assert.equal(iterations, 1);
});

test('search data fingerprint changes when parent_session_id changes', () => {
  const rows = [
    {
      assistant_type: 'codex',
      source_kind: 'cli',
      source_dir_key: 'aa',
      session_id: 'a__b',
      max_turn_no: 3,
      parent_session_id: '',
    },
  ];
  const parentAttached = [{ ...rows[0], parent_session_id: 'a' }];

  assert.notEqual(
    sessionSearchDataFingerprint(rows),
    sessionSearchDataFingerprint(parentAttached),
  );
  assert.notEqual(
    sessionSearchDataFingerprint(parentAttached),
    sessionSearchDataFingerprint(rows),
  );
});

test('search data fingerprint is stable across refreshes and row order', () => {
  const rows = [
    {
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
      session_id: 'root',
      max_turn_no: 1,
      parent_session_id: '',
    },
    {
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
      session_id: 'root__agent-1',
      max_turn_no: 4,
      parent_session_id: 'root',
    },
    {
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'bb',
      session_id: 'other',
      max_turn_no: 2,
      parent_session_id: '',
    },
  ];
  const refreshed = rows.map(row => ({ ...row }));
  const shuffled = [rows[2], rows[0], rows[1]].map(row => ({ ...row }));

  assert.equal(
    sessionSearchDataFingerprint(rows),
    sessionSearchDataFingerprint(refreshed),
  );
  assert.equal(
    sessionSearchDataFingerprint(rows),
    sessionSearchDataFingerprint(rows),
  );
  assert.equal(
    sessionSearchDataFingerprint(rows),
    sessionSearchDataFingerprint(shuffled),
  );
});

test('search data fingerprint treats empty and absent parent_session_id as the same', () => {
  const baseRow = {
    assistant_type: 'copilot',
    source_kind: 'copilot-app',
    source_dir_key: 'aa',
    session_id: 'a__b',
    max_turn_no: 3,
  };
  const emptyParent = { ...baseRow, parent_session_id: '' };
  const nullParent = { ...baseRow, parent_session_id: null };
  const absentParent = { ...baseRow };

  assert.equal(
    sessionSearchDataFingerprint([emptyParent]),
    sessionSearchDataFingerprint([nullParent]),
  );
  assert.equal(
    sessionSearchDataFingerprint([nullParent]),
    sessionSearchDataFingerprint([absentParent]),
  );
  assert.equal(
    sessionSearchDataFingerprint([emptyParent]),
    sessionSearchDataFingerprint([absentParent]),
  );
});

test('search data fingerprint changes when max_turn_no changes', () => {
  const rows = [
    {
      assistant_type: 'codex',
      source_kind: 'cli',
      source_dir_key: 'aa',
      session_id: 'a',
      max_turn_no: 3,
      parent_session_id: '',
    },
  ];
  const grown = [{ ...rows[0], max_turn_no: 4 }];

  assert.notEqual(
    sessionSearchDataFingerprint(rows),
    sessionSearchDataFingerprint(grown),
  );
});

test('search data fingerprint changes when same-turn usage grows with max_turn_no unchanged', () => {
  // 同回合（第一則回覆前）追加符合搜尋詞的 prompt 與用量：session 身分六欄
  // 與 max_turn_no 全不變，只有用量總量／耗時與 raw usage 列數成長。
  // 指紋必須察覺這種成長並觸發搜尋重跑（comment 4433781467）。
  const before = [
    {
      assistant_type: 'codex',
      source_kind: 'codex-cli',
      source_dir_key: null,
      session_id: 'same-turn',
      max_turn_no: 1,
      parent_session_id: '',
      total_tokens: 110,
      duration_ms: 500,
    },
  ];
  const after = [{ ...before[0], total_tokens: 220, duration_ms: 1500 }];

  assert.notEqual(
    sessionSearchDataFingerprint(before, 1),
    sessionSearchDataFingerprint(after, 2),
  );
  // 每個單調訊號都必須各自觸發重跑：用量成長、raw 列數成長。
  assert.notEqual(
    sessionSearchDataFingerprint(before, 1),
    sessionSearchDataFingerprint(after, 1),
  );
  assert.notEqual(
    sessionSearchDataFingerprint(before, 1),
    sessionSearchDataFingerprint(before, 2),
  );
});

test('search data fingerprint stays stable across row order and refresh with growth markers', () => {
  const rows = [
    {
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
      session_id: 'root',
      max_turn_no: 1,
      parent_session_id: '',
      total_tokens: 110,
      duration_ms: 500,
    },
    {
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
      session_id: 'root__agent-1',
      max_turn_no: 1,
      parent_session_id: 'root',
      total_tokens: 220,
      duration_ms: 1500,
    },
  ];
  const refreshed = rows.map(row => ({ ...row }));
  const shuffled = [rows[1], rows[0]].map(row => ({ ...row }));

  assert.equal(
    sessionSearchDataFingerprint(rows, 3),
    sessionSearchDataFingerprint(refreshed, 3),
  );
  assert.equal(
    sessionSearchDataFingerprint(rows, 3),
    sessionSearchDataFingerprint(rows, 3),
  );
  assert.equal(
    sessionSearchDataFingerprint(rows, 3),
    sessionSearchDataFingerprint(shuffled, 3),
  );
});

test('data change during an in-flight search keeps the request instead of aborting and restarting', () => {
  // 無條件重啟回歸鎖（comment 4433863686）：執行中收到 data-changed 不得
  // 產生任何取消／重啟遷移（startRun），只標記 dirty 待完成後合併處理。
  // 還原成「指紋一變就 abort＋重啟」時此測試立即 RED。
  const running = nextSessionSearchRunState(null, 'run-started');
  const afterChange = nextSessionSearchRunState(running, 'data-changed');

  assert.equal(afterChange.inFlight, true);
  assert.equal(afterChange.startRun, false);
  assert.equal(afterChange.dirty, true);
});

test('repeated data changes coalesce into exactly one rerun after the run completes', () => {
  // 執行期間 N 次資料變更必須收斂成恰好一次重跑（無限長的變更序列也只
  // 會排程一次），且重跑完成後不得再自行排程（防無限重跑）。
  let state = nextSessionSearchRunState(null, 'run-started');
  let scheduledRestarts = 0;

  for (let change = 0; change < 3; change += 1) {
    state = nextSessionSearchRunState(state, 'data-changed');
    if (state.startRun) scheduledRestarts += 1;
    assert.equal(state.startRun, false);
    assert.equal(state.inFlight, true);
  }
  assert.equal(state.dirty, true);

  state = nextSessionSearchRunState(state, 'run-completed');
  if (state.startRun) scheduledRestarts += 1;
  assert.equal(state.startRun, true);
  assert.equal(state.inFlight, false);
  assert.equal(state.dirty, false);
  assert.equal(scheduledRestarts, 1);

  // 重跑啟動後於乾淨狀態完成 → 閒置，不得再排程。
  state = nextSessionSearchRunState(state, 'run-started');
  state = nextSessionSearchRunState(state, 'run-completed');
  assert.equal(state.startRun, false);
  assert.equal(state.inFlight, false);
  assert.equal(state.dirty, false);
  assert.equal(scheduledRestarts, 1);
});

test('background rerun keeps completed matches and filter state visible until the replacement run completes', () => {
  // 可見性回歸鎖（comment 4433948840）：完成+dirty 的重跑排程不得清空剛取得
  // 的命中或切回 'loading'——清空發生在排程當下，完成渲染在同一個 JS task
  // 內即被覆寫，掃描 > 資料更新間隔時使用者永遠只看到搜尋中＋未過濾清單。
  // 還原成 clear-and-load 重跑時此測試立即 RED。
  const v1 = new Set(['session-a', 'session-b']);
  const v2 = new Set(['session-a', 'session-c']);

  let display = nextSessionSearchDisplayState(
    { matches: null, status: 'idle' },
    { type: 'new-intent', hasQuery: true },
  );
  assert.equal(display.matches, null);
  assert.equal(display.status, 'loading');

  display = nextSessionSearchDisplayState(display, { type: 'complete', matches: v1 });
  assert.equal(display.matches, v1);
  assert.equal(display.status, 'complete');

  // 完成+dirty → 背景重跑排程：命中（同參照、非 null／空）與 'complete'
  // 原樣續顯，篩選狀態維持可見。
  const kept = nextSessionSearchDisplayState(display, { type: 'background-rerun' });
  assert.equal(kept.matches, v1);
  assert.equal(kept.matches.size, 2);
  assert.equal(kept.status, 'complete');

  // 新結果直到成功完成才接手可見狀態。
  display = nextSessionSearchDisplayState(kept, { type: 'complete', matches: v2 });
  assert.equal(display.matches, v2);
  assert.equal(display.status, 'complete');
});

test('run completion swaps search results atomically with no empty intermediate state', () => {
  // 原子替換回歸鎖（comment 4433948840）：'complete' 必須單步自舊命中直上新
  // 命中，任何遷移都不得回 null／空集合（clear-then-set 變異在此測試 RED）。
  const v1 = new Set(['session-a']);
  const v2 = new Set(['session-a', 'session-b']);
  const v3 = new Set(['session-b']);

  // dirty 重跑待排時的完成：單步直上 V2，無空集合中間態。
  const swapped = nextSessionSearchDisplayState(
    { matches: v1, status: 'complete' },
    { type: 'complete', matches: v2 },
  );
  assert.equal(swapped.matches, v2);
  assert.equal(swapped.status, 'complete');

  // complete(V2) → background-rerun → complete(V3) 全程每一步皆持完整集合。
  const kept = nextSessionSearchDisplayState(swapped, { type: 'background-rerun' });
  assert.notEqual(kept.matches, null);
  assert.equal(kept.matches.size, 2);
  assert.equal(kept.status, 'complete');

  const reswapped = nextSessionSearchDisplayState(kept, { type: 'complete', matches: v3 });
  assert.equal(reswapped.matches, v3);
  assert.equal(reswapped.status, 'complete');

  // 背景重跑失敗同樣不得掏空可見結果（保留既有命中與 'complete'）。
  const keptAfterFail = nextSessionSearchDisplayState(
    reswapped,
    { type: 'fail', background: true },
  );
  assert.equal(keptAfterFail.matches, v3);
  assert.equal(keptAfterFail.status, 'complete');
});

test('search failure invalidates the searched-data fingerprint so the next identical-data refresh retries', () => {
  // 失敗死結回歸鎖（comment 4434003880）：背景重跑失敗保留舊命中後，
  // run-completed（dirty=false）不會再排程，若「已搜尋」指紋仍記著這次資料，
  // 同資料的自動刷新永不觸發搜尋——舊命中集合凍結、新符合 session 永久隱藏。
  // 失敗必須使指紋標記失效（''＝既有「未搜尋」哨兵），顯示原樣不動，下次
  // 資料刷新視為未搜尋而重試。
  const rows = [
    {
      assistant_type: 'copilot',
      source_kind: 'copilot-app',
      source_dir_key: 'aa',
      session_id: 'root',
      max_turn_no: 1,
      parent_session_id: '',
      total_tokens: 110,
      duration_ms: 500,
    },
  ];
  const fingerprint = sessionSearchDataFingerprint(rows, 1);
  const keptMatches = new Set(['session-a', 'session-b']);

  // 失敗當下顯示原樣保留（round-7 語意）：舊命中與 'complete' 續顯，
  // 指紋失效不得動到顯示層。
  const display = nextSessionSearchDisplayState(
    { matches: keptMatches, status: 'complete' },
    { type: 'fail', background: true },
  );
  assert.equal(display.matches, keptMatches);
  assert.equal(display.status, 'complete');

  // renderDashboard 已記錄 F1（先記後搜）→ 失敗使標記失效。
  let searched = nextSessionSearchSearchedFingerprint('', { type: 'searched', fingerprint });
  assert.equal(searched, fingerprint);
  searched = nextSessionSearchSearchedFingerprint(searched, { type: 'fail' });
  assert.equal(searched, '');

  // 下一次 renderDashboard 的比對（nextSearchFingerprint !== 標記）必為真 →
  // 重試；同資料的自動刷新（指紋跨刷新穩定）也仍觸發。
  assert.notEqual(searched, fingerprint);
  assert.notEqual(
    searched,
    sessionSearchDataFingerprint(rows.map(row => ({ ...row })), 1),
  );
});

test('searched fingerprint marker advances only on successful completion and never on failure', () => {
  // 標記遷移邊界鎖（comment 4434003880）：'fail' 不得記錄新值、也不得保留舊值
  // （保留＝同資料永不再試的死結復活）；失效語意只屬於失敗路徑——失敗後下一次
  // 'searched' 記錄必須正常前進到新指紋，'reset' 仍歸零、未知事件原樣保留。
  const fixture = {
    assistant_type: 'codex',
    source_kind: 'cli',
    source_dir_key: 'aa',
    session_id: 'a',
    parent_session_id: '',
  };
  const f1 = sessionSearchDataFingerprint([
    { ...fixture, max_turn_no: 1, total_tokens: 100, duration_ms: 400 },
  ], 1);
  const f2 = sessionSearchDataFingerprint([
    { ...fixture, max_turn_no: 2, total_tokens: 250, duration_ms: 900 },
  ], 2);

  // fail 不得記錄、也不得保留（任意前狀態皆失效為 ''）。
  assert.equal(nextSessionSearchSearchedFingerprint(f1, { type: 'fail' }), '');
  assert.equal(nextSessionSearchSearchedFingerprint('', { type: 'fail' }), '');

  // fail → searched(F2) 後 marker=F2。
  const searched = nextSessionSearchSearchedFingerprint(
    nextSessionSearchSearchedFingerprint(f1, { type: 'fail' }),
    { type: 'searched', fingerprint: f2 },
  );
  assert.equal(searched, f2);

  // 成功記錄不得被本修正誤傷成清空；reset 歸零；未知事件原樣保留。
  assert.equal(
    nextSessionSearchSearchedFingerprint(f1, { type: 'searched', fingerprint: f2 }),
    f2,
  );
  assert.equal(nextSessionSearchSearchedFingerprint(searched, { type: 'reset' }), '');
  assert.equal(
    nextSessionSearchSearchedFingerprint(searched, { type: 'unknown' }),
    searched,
  );
});
