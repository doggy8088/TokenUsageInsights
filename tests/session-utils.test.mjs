import assert from 'node:assert/strict';
import test from 'node:test';

import {
  compareSessionRows,
  filterEntriesBySessionIdentity,
  matchesSessionIdentity,
  parentSessionIdentityKey,
  resolveParentSession,
  sessionIdentityKey,
  sortAndGetFlatSessions,
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

  assert.equal(
    parentSessionIdentityKey(child),
    sessionIdentityKey({ ...child, session_id: 'parent' }),
  );
  assert.notEqual(
    parentSessionIdentityKey(child),
    sessionIdentityKey({
      ...child,
      session_id: 'parent',
      source_dir_key: 'bb',
    }),
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

test('sortAndGetFlatSessions nests subagents in a tree structure like Claude Code', () => {
  const sessions = [
    {
      session_id: 'root-1',
      session_name: 'Main Task',
      assistant_type: 'gemini',
      source_kind: 'gemini-cli',
      timestamp: '2026-10-14 10:00:00',
    },
    {
      session_id: 'sub-1',
      parent_session_id: 'root-1',
      session_name: 'Sub Task 1',
      assistant_type: 'gemini',
      source_kind: 'gemini-cli',
      timestamp: '2026-10-14 10:05:00',
    },
    {
      session_id: 'sub-1-1',
      parent_session_id: 'sub-1',
      session_name: 'Nested Sub Task',
      assistant_type: 'gemini',
      source_kind: 'gemini-cli',
      timestamp: '2026-10-14 10:10:00',
    },
  ];

  const flat = sortAndGetFlatSessions(sessions, 'timestamp', 'desc');
  assert.equal(flat.length, 3);
  assert.equal(flat[0].session_id, 'root-1');
  assert.equal(flat[0].depth, 0);
  assert.equal(flat[0].isSubagent, false);

  assert.equal(flat[1].session_id, 'sub-1');
  assert.equal(flat[1].depth, 1);
  assert.equal(flat[1].isSubagent, true);
  assert.equal(flat[1].parentName, 'Main Task');

  assert.equal(flat[2].session_id, 'sub-1-1');
  assert.equal(flat[2].depth, 2);
  assert.equal(flat[2].isSubagent, true);
  assert.equal(flat[2].parentName, 'Sub Task 1');
});

test('sortAndGetFlatSessions matches parent across different source_kind or source_dir_key', () => {
  const sessions = [
    {
      session_id: 'parent-ide',
      session_name: 'Antigravity IDE Session',
      assistant_type: 'antigravity-ide',
      source_kind: 'antigravity-ide-session',
      source_dir_key: 'dir-a',
      timestamp: '2026-10-14 09:00:00',
    },
    {
      session_id: 'child-cli',
      parent_session_id: 'parent-ide',
      session_name: 'Subagent Worker',
      assistant_type: 'antigravity',
      source_kind: 'antigravity-session',
      source_dir_key: null,
      timestamp: '2026-10-14 09:05:00',
    },
  ];

  const flat = sortAndGetFlatSessions(sessions, 'timestamp', 'desc');
  assert.equal(flat.length, 2);
  assert.equal(flat[0].session_id, 'parent-ide');
  assert.equal(flat[0].depth, 0);
  assert.equal(flat[1].session_id, 'child-cli');
  assert.equal(flat[1].depth, 1);
  assert.equal(flat[1].isSubagent, true);
  assert.equal(flat[1].parentName, 'Antigravity IDE Session');
});

test('sortAndGetFlatSessions handles orphan subagents whose parent is absent', () => {
  const sessions = [
    {
      session_id: 'orphan-sub',
      parent_session_id: 'missing-parent',
      session_name: 'Detached Worker',
      assistant_type: 'pi',
      source_kind: 'pi-session',
      timestamp: '2026-10-14 11:00:00',
    },
  ];

  const flat = sortAndGetFlatSessions(sessions, 'timestamp', 'desc');
  assert.equal(flat.length, 1);
  assert.equal(flat[0].session_id, 'orphan-sub');
  assert.equal(flat[0].isSubagent, true);
  assert.equal(flat[0].parentName, 'missing-parent');
});

test('sortAndGetFlatSessions prevents circular references from causing infinite loops', () => {
  const sessions = [
    {
      session_id: 'cycle-a',
      parent_session_id: 'cycle-b',
      assistant_type: 'codex',
      source_kind: 'codex-desktop',
      timestamp: '2026-10-14 12:00:00',
    },
    {
      session_id: 'cycle-b',
      parent_session_id: 'cycle-a',
      assistant_type: 'codex',
      source_kind: 'codex-desktop',
      timestamp: '2026-10-14 12:01:00',
    },
  ];

  const flat = sortAndGetFlatSessions(sessions, 'timestamp', 'desc');
  assert.equal(flat.length, 2);
});

