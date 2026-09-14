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

export function sessionIdentityKey(session) {
  return JSON.stringify([
    session?.assistant_type || '',
    session?.source_kind || '',
    session?.source_dir_key || '',
    session?.session_id || '',
  ]);
}

export function parentSessionIdentityKey(session) {
  if (!session?.parent_session_id) return null;
  return sessionIdentityKey({
    ...session,
    session_id: session.parent_session_id,
  });
}

export function filterEntriesBySessionIdentity(entries, sessions) {
  const sessionKeys = new Set((sessions || []).map(sessionIdentityKey));
  return (entries || []).filter(entry => sessionKeys.has(sessionIdentityKey(entry)));
}
