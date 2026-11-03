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

export function resolveParentSession(item, sessionsMap, bySessionId) {
  if (!item?.parent_session_id) return null;
  const parentKey = parentSessionIdentityKey(item);
  if (parentKey && sessionsMap.has(parentKey)) {
    return sessionsMap.get(parentKey);
  }
  if (bySessionId) {
    const candidates = bySessionId.get(item.parent_session_id);
    if (candidates && candidates.length > 0) {
      const same = candidates.find(c => c.assistant_type === item.assistant_type);
      if (same) return same;
      return candidates[0];
    }
  }
  return null;
}

export function sortAndGetFlatSessions(sessions, sortCol, sortDir) {
  const map = new Map();
  const bySessionId = new Map();

  sessions.forEach(s => {
    const key = sessionIdentityKey(s);
    const node = { ...s, children: [] };
    map.set(key, node);

    const sid = s.session_id;
    if (sid) {
      if (!bySessionId.has(sid)) {
        bySessionId.set(sid, []);
      }
      bySessionId.get(sid).push(node);
    }
  });

  function wouldCreateCycle(parent, child) {
    let curr = parent;
    const visited = new Set();
    while (curr) {
      if (curr === child) return true;
      const key = sessionIdentityKey(curr);
      if (visited.has(key)) return true;
      visited.add(key);
      curr = resolveParentSession(curr, map, bySessionId);
    }
    return false;
  }

  const nodes = [...map.values()];
  const roots = [];
  nodes.forEach(item => {
    const parent = resolveParentSession(item, map, bySessionId);
    if (parent && parent !== item && !wouldCreateCycle(parent, item)) {
      parent.children.push(item);
    } else {
      roots.push(item);
    }
  });

  const compare = (a, b) => compareSessionRows(a, b, sortCol, sortDir);

  // 排序 Root 節點
  roots.sort(compare);
  nodes.sort(compare);

  // 扁平化
  const flat = [];
  const visited = new Set();
  const traverse = (node, depth, parentName) => {
    const nodeKey = sessionIdentityKey(node);
    if (visited.has(nodeKey)) return;
    visited.add(nodeKey);
    flat.push({
      ...node,
      depth,
      isSubagent: depth > 0 || Boolean(node.parent_session_id),
      parentName: parentName || node.parent_session_id || null,
    });
    node.children
      .sort(compare)
      .forEach(child => traverse(child, depth + 1, node.session_name));
  };
  roots.forEach(r => traverse(r, 0, null));
  nodes.forEach(node => {
    if (!visited.has(sessionIdentityKey(node))) {
      traverse(node, 0, null);
    }
  });

  return flat;
}

