import { parseUsageTimestamp } from './chart-utils.js?v=7';

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
  return (session?.session_id || '') === (identity?.session_id || '')
    && (session?.assistant_type || '') === (identity?.assistant_type || '')
    && (session?.source_kind || '') === (identity?.source_kind || '')
    && (session?.source_dir_key || '') === (identity?.source_dir_key || '');
}
