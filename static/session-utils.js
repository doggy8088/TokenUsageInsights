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

// 搜尋重跑門檻的資料指紋：必須涵蓋 sessionSearchMatchKey 的完整鍵空間
// （含 parent_session_id），否則父層單獨變更時指紋不變、搜尋不重跑，
// 舊命中鍵無法匹配新列。既有 5 元維持裸欄位（parity）；父層正規化
// empty==absent 鏡像 sessionIdentityKey。另納入單調成長訊號——每 session
// 的 total_tokens／duration_ms 與當日 raw usage 列數（rawUsageEntryCount）：
// 同回合（max_turn_no 不變）追加 prompt＋用量時身分六欄全不變，只有這三者
// 隨用量列追加而變動，讓 renderDashboard 重跑搜尋、不再以舊命中集合隱藏
// 已匹配的 session（comment 4433781467）。三者皆為單調彙整值，在列序
// 打亂／重複刷新相同資料時保持穩定。殘餘缺口：只改 transcript 文字且
// 零用量列變動的編輯前端無法察覺，需 server 端 transcript 修訂標記
// （如 mtime＋size 或內容 hash）才能涵蓋。
export function sessionSearchDataFingerprint(rows, rawUsageEntryCount) {
  const rawEntryCount = Number(rawUsageEntryCount) || 0;
  return JSON.stringify([
    rawEntryCount,
    (rows || [])
      .map(session => [
        session.assistant_type,
        session.source_kind,
        session.source_dir_key,
        session.session_id,
        session.max_turn_no,
        session?.parent_session_id || '',
        session?.total_tokens || 0,
        session?.duration_ms || 0,
      ])
      .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
  ]);
}

// 搜尋重跑排程狀態機：對同一查詢保留執行中的請求，資料變動（指紋成長）
// 只標記待重跑，於完成後合併成恰好一次重跑——避免即時刷新在整日掃描完成
// 前反覆取消重啟（掃描 > 刷新間隔時零完成、搜尋永久載入中，且後端
// spawn_blocking 掃描不受 fetch 取消影響而持續浪費，comment 4433863686）。
// 僅「資料變動」走此保留＋合併路徑；query／context 變更仍由 app.js 沿用
// abort＋重啟（scheduleSessionPromptSearch／resetSessionPromptSearch 送
// 'reset' 歸零）。事件：'data-changed'（指紋成長）| 'run-started' |
// 'run-completed' | 'reset'。回傳 { inFlight, dirty, startRun }：
// startRun 是唯一決策輸出，呼叫端照表執行；N 次 data-changed 在執行中
// 收斂成 dirty 布林值，完成後至多一次重跑（無重跑佇列）。
export function nextSessionSearchRunState(state, event) {
  const inFlight = Boolean(state?.inFlight);
  const dirty = Boolean(state?.dirty);
  switch (event) {
    case 'data-changed':
      // 執行中：不中斷、只標記待重跑；閒置：立即重跑（與資料變更即重跑的
      // 原本語意相同）。
      return { inFlight, dirty: dirty || inFlight, startRun: !inFlight };
    case 'run-started':
      // 掃描自當下資料起算：啟動即清除 dirty（先前變更已由本次掃描涵蓋）。
      return { inFlight: true, dirty: false, startRun: false };
    case 'run-completed':
      // 完成後合併處理：套用本次結果後，累積的資料變更併成恰好一次重跑。
      return { inFlight: false, dirty: false, startRun: dirty };
    default:
      // 'reset'：query／context 變更或輸入清空，狀態歸零待新意圖重建。
      return { inFlight: false, dirty: false, startRun: false };
  }
}

// 搜尋可見狀態生命週期（純函式，comment 4433948840）：state = { matches,
// status }，status ∈ 'idle' | 'loading' | 'complete' | 'error'。清空發生在
// 「排程」當下而非完成後，故事件含 'background-rerun'／'new-intent' 以鎖住
// 「背景重跑期間已完成結果續顯」。事件與語意：
//   { type: 'new-intent', hasQuery } — query／context 變更、輸入清空：
//     clear-and-load（新查詢的舊命中無意義），hasQuery 決定 'loading' 或
//     'idle'。
//   { type: 'background-rerun' } — 同查詢、資料驅動的背景重跑：原樣保留
//     命中與 'complete'，不清空、不切 'loading'，篩選狀態續用至新結果
//     原子替換（掃描 > 資料更新間隔時不再永久顯示搜尋中＋未過濾清單）。
//   { type: 'complete', matches } — 成功完成：以新命中原子替換（單步、
//     無空集合中間態，dirty 與否不影響可見性）。
//   { type: 'fail', background } — 失敗：背景重跑保留既有結果與狀態；
//     首跑回 null+'error'。
export function nextSessionSearchDisplayState(state, event) {
  switch (event?.type) {
    case 'new-intent':
      return { matches: null, status: event.hasQuery ? 'loading' : 'idle' };
    case 'background-rerun':
      return state;
    case 'complete':
      return { matches: event.matches, status: 'complete' };
    case 'fail':
      return event.background
        ? state
        : { matches: null, status: 'error' };
    default:
      return state;
  }
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
