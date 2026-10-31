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

export function filterEntriesBySessionIdentity(entries, sessions) {
  const sessionKeys = new Set((sessions || []).map(sessionIdentityKey));
  return (entries || []).filter(entry => sessionKeys.has(sessionIdentityKey(entry)));
}
