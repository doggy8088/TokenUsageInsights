export function parseUsageTimestamp(timestamp) {
  const value = String(timestamp || '').trim();
  if (!value) return null;

  const hasExplicitTimezone = /(?:Z|[+-]\d{2}:?\d{2})$/i.test(value);
  const isSqlOrIsoTimestamp = /^\d{4}-\d{2}-\d{2}[T\s]\d{2}:\d{2}/.test(value);
  const normalized = isSqlOrIsoTimestamp && !hasExplicitTimezone
    ? `${value.replace(' ', 'T')}Z`
    : value;
  const parsed = new Date(normalized);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}
