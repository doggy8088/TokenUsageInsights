import { parseUsageTimestamp } from './time-utils.js?v=1';

function getSessionSortValue(session, sortColumn) {
  const value = session?.[sortColumn];
  if (sortColumn === 'timestamp') {
    return parseUsageTimestamp(value)?.getTime() ?? 0;
  }
  return value ?? 0;
}

export function compareSessionRows(a, b, sortColumn, sortDirection) {
  const valueA = getSessionSortValue(a, sortColumn);
  const valueB = getSessionSortValue(b, sortColumn);
  let comparison;

  if (typeof valueA === 'string' && typeof valueB === 'string') {
    comparison = valueA.localeCompare(valueB);
  } else {
    comparison = valueA - valueB;
  }

  return sortDirection === 'asc' ? comparison : -comparison;
}

export function matchesSessionIdentity(session, identity) {
  return sessionIdentityKey(session) === sessionIdentityKey(identity);
}

// Session 身分鍵包含 parent_session_id：真實獨立 session `a__b`（無父層）與
// session `a` 的合成子 Agent `b`（合成 id 同為 `a__b`）必須是兩個身分。
export function sessionIdentityKey(session) {
  return JSON.stringify([
    session?.assistant_type || '',
    session?.source_kind || '',
    session?.source_dir_key || '',
    session?.session_id || '',
    session?.parent_session_id || '',
  ]);
}

// 搜尋結果列（後端 SessionSearchMatch）與 sessions 清單列共用同一把含
// parent_session_id 的身分鍵：真實獨立 session `a__b` 與 session `a` 的
// 合成子 Agent `b` 必須各自可被選取（comment 4403291365）。文字搜尋的
// 過濾行為不變：符合文字的每一列都以自己的鍵入選，不會漏掉符合列。
export function sessionSearchMatchKey(session) {
  return sessionIdentityKey(session);
}

export function parentSessionIdentityKey(session, sessions) {
  if (!session?.parent_session_id) return null;
  // 父層通常是主／獨立 session（Copilot 合成 `<parent>__<agent>` 約定），
  // 以「父層為 root」的鍵優先；若是巢狀子 Agent（例如 Claude
  // agent-a2 → agent-a1 → root），父層自身的 parent 無法由子列推得，
  // 改以 sessions 清單找出唯一的同名父列。
  const parentAsRootKey = sessionIdentityKey({
    ...session,
    session_id: session.parent_session_id,
    parent_session_id: null,
  });
  const parentCandidates = (sessions || []).filter(candidate => (
    candidate?.session_id === session.parent_session_id
    && (candidate?.assistant_type || '') === (session?.assistant_type || '')
    && (candidate?.source_kind || '') === (session?.source_kind || '')
    && (candidate?.source_dir_key || '') === (session?.source_dir_key || '')
  ));
  if (parentCandidates.length === 0) return parentAsRootKey;
  const candidateKeys = parentCandidates.map(sessionIdentityKey);
  if (candidateKeys.includes(parentAsRootKey)) return parentAsRootKey;
  if (parentCandidates.length === 1) return sessionIdentityKey(parentCandidates[0]);
  return parentAsRootKey;
}

// 參考實作（oracle）：根節點（父鏈頂端）查找，逐字複刻 static/app.js 舊
// getRootParentKey 的走訪與成環處理（成環時回傳環上最小字序鍵）。保留作為
// buildSessionParentIndex().rootKeyOf 快取版的行為對照基準（parity 測試
// 鎖定兩者一致）；每次查找仍走父鏈，不提供 O(1) 保證。
export function rootSessionIdentityKey(session, sessions) {
  const sessionMap = new Map((sessions || []).map(row => [sessionIdentityKey(row), row]));
  let curr = session;
  const path = [];
  const positions = new Map();

  while (curr) {
    const key = sessionIdentityKey(curr);
    if (positions.has(key)) {
      return path
        .slice(positions.get(key))
        .map(node => sessionIdentityKey(node))
        .sort((a, b) => a.localeCompare(b))[0];
    }

    positions.set(key, path.length);
    path.push(curr);

    const parentKey = parentSessionIdentityKey(curr, sessions);
    if (!parentKey || !sessionMap.has(parentKey)) {
      return key;
    }
    curr = sessionMap.get(parentKey);
  }

  return sessionIdentityKey(session);
}

// 候選父列索引鍵：以 parentSessionIdentityKey 匹配條件的 4 個欄位分桶，
// session_id 保留原值參與組鍵；組鍵正規化可能合併的型別邊角（例如 null 與
// undefined）由呼叫端重跑原始 filter 條件排除，確保與全表 filter 逐支等價。
function parentCandidateMatchKey(session, sessionId) {
  return JSON.stringify([
    session?.assistant_type || '',
    session?.source_kind || '',
    session?.source_dir_key || '',
    sessionId,
  ]);
}

// 一次 O(N) 掃描建立「父層／根節點」查找索引，取代 f743b2e 起每次查父層就
// filter 整份 sessions 的作法——建樹、逐列渲染與 hover 會反覆查找，大量子
// Agent 時退化成二次方掃描並卡住介面（review comment 4436837586 / thread
// PRRT_kwDOTOgsec6yTxj0）。parentKeyOf 與 parentSessionIdentityKey、
// rootKeyOf 與 rootSessionIdentityKey 必須逐支等價（parity 測試鎖定）。
export function buildSessionParentIndex(sessions) {
  const rows = Array.isArray(sessions) ? sessions : [];
  const rowByKey = new Map();
  const candidateRowsByMatch = new Map();

  for (const row of rows) {
    // 與 app.js 舊 sessionsMap 相同的「後者覆蓋」語意。
    rowByKey.set(sessionIdentityKey(row), row);
    const matchKey = parentCandidateMatchKey(row, row.session_id);
    const bucket = candidateRowsByMatch.get(matchKey);
    if (bucket) {
      bucket.push(row);
    } else {
      candidateRowsByMatch.set(matchKey, [row]);
    }
  }

  const parentKeyCache = new Map();
  const rootKeyCache = new Map();

  function resolveParentKey(session) {
    // 與 parentSessionIdentityKey 完全相同的決策樹：無父層 → null；先算
    // 「父層為 root」鍵；候選 0 個 → parentAsRootKey；候選鍵含
    // parentAsRootKey → parentAsRootKey；唯一候選 → 該候選的完整 5 元鍵；
    // 多候選（含重複列）→ parentAsRootKey。
    if (!session?.parent_session_id) return null;
    const parentAsRootKey = sessionIdentityKey({
      ...session,
      session_id: session.parent_session_id,
      parent_session_id: null,
    });
    const bucket = candidateRowsByMatch.get(
      parentCandidateMatchKey(session, session.parent_session_id),
    ) || [];
    const parentCandidates = bucket.filter(candidate => (
      candidate?.session_id === session.parent_session_id
      && (candidate?.assistant_type || '') === (session?.assistant_type || '')
      && (candidate?.source_kind || '') === (session?.source_kind || '')
      && (candidate?.source_dir_key || '') === (session?.source_dir_key || '')
    ));
    if (parentCandidates.length === 0) return parentAsRootKey;
    const candidateKeys = parentCandidates.map(sessionIdentityKey);
    if (candidateKeys.includes(parentAsRootKey)) return parentAsRootKey;
    if (parentCandidates.length === 1) return sessionIdentityKey(parentCandidates[0]);
    return parentAsRootKey;
  }

  function parentKeyOf(session) {
    const cacheKey = sessionIdentityKey(session);
    if (parentKeyCache.has(cacheKey)) return parentKeyCache.get(cacheKey);
    const parentKey = resolveParentKey(session);
    parentKeyCache.set(cacheKey, parentKey);
    return parentKey;
  }

  function rootKeyOf(session) {
    // 走父鏈至根、沿途快取結案；成環處理與 getRootParentKey 相同（回傳環上
    // 最小字序鍵）。父鏈是函數圖，根鍵只由身分鍵決定，同條路徑上的每個節點
    // 都結案到同一個根鍵，快取不改變語意。
    const startKey = sessionIdentityKey(session);
    if (rootKeyCache.has(startKey)) return rootKeyCache.get(startKey);

    const path = [];
    const positions = new Map();
    let curr = session;
    while (curr) {
      const key = sessionIdentityKey(curr);
      if (rootKeyCache.has(key)) {
        const resolved = rootKeyCache.get(key);
        for (const node of path) rootKeyCache.set(sessionIdentityKey(node), resolved);
        return resolved;
      }
      if (positions.has(key)) {
        const resolved = path
          .slice(positions.get(key))
          .map(node => sessionIdentityKey(node))
          .sort((a, b) => a.localeCompare(b))[0];
        for (const node of path) rootKeyCache.set(sessionIdentityKey(node), resolved);
        return resolved;
      }

      positions.set(key, path.length);
      path.push(curr);

      const parentKey = parentKeyOf(curr);
      if (!parentKey || !rowByKey.has(parentKey)) {
        for (const node of path) rootKeyCache.set(sessionIdentityKey(node), key);
        return key;
      }
      curr = rowByKey.get(parentKey);
    }

    return sessionIdentityKey(session);
  }

  return { parentKeyOf, rootKeyOf };
}

export function filterEntriesBySessionIdentity(entries, sessions) {
  const sessionKeys = new Set((sessions || []).map(sessionIdentityKey));
  return (entries || []).filter(entry => sessionKeys.has(sessionIdentityKey(entry)));
}
