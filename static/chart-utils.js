export function normalizeEntryTokenParts(entry) {
  const tokens = entry?.delta_tokens || (entry?.turn_no === 1 ? entry.tokens : null);
  if (!tokens) {
    return { input: 0, output: 0, cache: 0, total: 0 };
  }

  const rawInput = Math.max(0, Number(tokens.input) || 0);
  const rawOutput = Math.max(0, Number(tokens.output) || 0);
  const rawCache = Math.max(0, Number(tokens.cache_read) || 0)
    + Math.max(0, Number(tokens.cache_write) || 0);
  const reportedTotal = Math.max(0, Number(tokens.total) || 0);
  const total = reportedTotal || Math.max(rawInput + rawOutput, rawCache + rawOutput);
  const output = Math.min(rawOutput, total);
  const cache = Math.min(rawCache, Math.max(0, total - output));
  const input = Math.max(0, total - output - cache);

  return { input, output, cache, total };
}

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

export function getUsageEntryUtcMinute(timestamp) {
  const parsed = parseUsageTimestamp(timestamp);
  if (parsed) {
    return parsed.getUTCHours() * 60 + parsed.getUTCMinutes();
  }

  const timeMatch = String(timestamp || '').match(/(?:T|\s|^)(\d{1,2}):(\d{2})/);
  if (!timeMatch) return null;
  const hours = Number(timeMatch[1]);
  const minutes = Number(timeMatch[2]);
  if (hours > 23 || minutes > 59) return null;
  return hours * 60 + minutes;
}

export function formatMinuteOfDay(totalMinutes) {
  if (totalMinutes >= 1440) return '24:00';
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}`;
}

export function formatUtcMinuteInBrowserTime(utcDate, totalMinutes, includeDate = false) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(String(utcDate || ''))) {
    return formatMinuteOfDay(totalMinutes);
  }

  const localDate = new Date(`${utcDate}T00:00:00Z`);
  localDate.setUTCMinutes(totalMinutes);
  if (Number.isNaN(localDate.getTime())) return formatMinuteOfDay(totalMinutes);

  const pad = value => String(value).padStart(2, '0');
  const time = `${pad(localDate.getHours())}:${pad(localDate.getMinutes())}`;
  if (!includeDate) return time;
  return `${pad(localDate.getMonth() + 1)}/${pad(localDate.getDate())} ${time}`;
}

export function isUtcBucketFuture(utcDate, startMinute, now = new Date()) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(String(utcDate || ''))) return false;
  const dayStart = Date.parse(`${utcDate}T00:00:00Z`);
  const nowValue = now instanceof Date ? now.getTime() : new Date(now).getTime();
  if (!Number.isFinite(dayStart) || !Number.isFinite(nowValue)) return false;
  return dayStart + startMinute * 60_000 > nowValue;
}

export function calculateCandleViewport(candles, maxVisible = 24, requestedStart = null) {
  const candleCount = Array.isArray(candles) ? candles.length : 0;
  const visibleCount = Math.min(Math.max(1, maxVisible), Math.max(1, candleCount));
  let lastAvailableIndex = -1;
  for (let index = candleCount - 1; index >= 0; index -= 1) {
    if (!candles[index].isFuture) {
      lastAvailableIndex = index;
      break;
    }
  }

  const latestWindowEnd = lastAvailableIndex >= 0
    ? lastAvailableIndex
    : Math.min(candleCount - 1, visibleCount - 1);
  const maxStart = Math.max(0, latestWindowEnd - visibleCount + 1);
  const requested = requestedStart !== null && Number.isFinite(Number(requestedStart))
    ? Math.round(Number(requestedStart))
    : maxStart;
  const start = Math.max(0, Math.min(maxStart, requested));
  const end = candleCount > 0
    ? Math.min(candleCount - 1, start + visibleCount - 1)
    : 0;

  return {
    start,
    end,
    visibleCount: candleCount > 0 ? end - start + 1 : 0,
    candleCount,
    lastAvailableIndex,
    maxStart,
    canPan: maxStart > 0,
  };
}

export function calculateCandleViewportYRange(candles, movingAverageValues, viewport) {
  if (!Array.isArray(candles) || candles.length === 0 || !viewport) {
    return { min: 0, max: 1 };
  }

  const values = [];
  for (let index = viewport.start; index <= viewport.end; index += 1) {
    const candle = candles[index];
    if (!candle || candle.isFuture) continue;
    values.push(candle.open, candle.close);
    const movingAverage = movingAverageValues?.[index];
    if (Number.isFinite(movingAverage)) values.push(movingAverage);
  }
  if (values.length === 0) return { min: 0, max: 1 };

  const dataMin = Math.min(...values);
  const dataMax = Math.max(...values);
  const spread = Math.max(0, dataMax - dataMin);
  const padding = Math.max(1, spread * 0.12, dataMax * 0.015);
  return {
    min: dataMin <= padding ? 0 : Math.max(0, dataMin - padding),
    max: Math.max(1, dataMax + padding),
  };
}

export function getChartDataPointX(chart, dataIndex) {
  for (let datasetIndex = 0; datasetIndex < chart.data.datasets.length; datasetIndex += 1) {
    const element = chart.getDatasetMeta(datasetIndex)?.data?.[dataIndex];
    if (element && Number.isFinite(element.x)) {
      return element.x;
    }
  }
  return chart.scales.x.getPixelForValue(chart.data.labels[dataIndex], dataIndex);
}

export function calculateCumulativeMovingAverage(candles, windowSize = 5) {
  const values = Array.from({ length: candles.length }, () => null);
  const firstActiveIndex = candles.findIndex(candle => candle.total > 0);
  let lastActiveIndex = -1;
  for (let index = candles.length - 1; index >= 0; index -= 1) {
    if (candles[index].total > 0) {
      lastActiveIndex = index;
      break;
    }
  }
  if (firstActiveIndex < 0 || lastActiveIndex < 0) return values;

  for (let index = firstActiveIndex; index <= lastActiveIndex; index += 1) {
    const startIndex = Math.max(firstActiveIndex, index - windowSize + 1);
    const window = candles.slice(startIndex, index + 1);
    values[index] = window.reduce((sum, candle) => sum + candle.close, 0) / window.length;
  }
  return values;
}

function calculateRegressionSlope(values, endIndex, intervalMinutes, windowSize) {
  const points = [];
  for (let index = endIndex; index >= 0 && points.length < windowSize; index -= 1) {
    if (Number.isFinite(values[index])) {
      points.unshift({ index, value: values[index] });
    }
  }
  if (points.length < 2) return null;

  const intervalHours = intervalMinutes / 60;
  const originIndex = points[0].index;
  const meanX = points.reduce(
    (sum, point) => sum + (point.index - originIndex) * intervalHours,
    0
  ) / points.length;
  const meanY = points.reduce((sum, point) => sum + point.value, 0) / points.length;
  let numerator = 0;
  let denominator = 0;
  points.forEach(point => {
    const x = (point.index - originIndex) * intervalHours;
    numerator += (x - meanX) * (point.value - meanY);
    denominator += (x - meanX) ** 2;
  });
  return denominator > 0 ? numerator / denominator : null;
}

function summarizeMovingAverageTrend(
  values,
  intervalMinutes,
  windowSize,
  startIndex = 0,
  endIndex = values.length - 1
) {
  let lastIndex = -1;
  for (let index = Math.min(values.length - 1, endIndex); index >= startIndex; index -= 1) {
    if (Number.isFinite(values[index])) {
      lastIndex = index;
      break;
    }
  }

  const slopeTokensPerHour = lastIndex >= 0
    ? calculateRegressionSlope(values, lastIndex, intervalMinutes, windowSize)
    : null;
  const previousSlopeTokensPerHour = lastIndex > 0
    ? calculateRegressionSlope(values, lastIndex - 1, intervalMinutes, windowSize)
    : null;
  const momentumChangePercent = Number.isFinite(slopeTokensPerHour)
    && Number.isFinite(previousSlopeTokensPerHour)
    && Math.abs(previousSlopeTokensPerHour) > 0.000001
    ? ((slopeTokensPerHour - previousSlopeTokensPerHour) / Math.abs(previousSlopeTokensPerHour)) * 100
    : null;

  let momentum = 'steady';
  if (Number.isFinite(momentumChangePercent) && momentumChangePercent > 5) {
    momentum = 'accelerating';
  } else if (Number.isFinite(momentumChangePercent) && momentumChangePercent < -5) {
    momentum = 'cooling';
  }

  return {
    values,
    windowSize,
    lastIndex,
    slopeTokensPerHour,
    previousSlopeTokensPerHour,
    momentumChangePercent,
    momentum,
  };
}

export function calculateMovingAverageTrend(candles, intervalMinutes, windowSize = 5) {
  const values = calculateCumulativeMovingAverage(candles, windowSize);
  return summarizeMovingAverageTrend(values, intervalMinutes, windowSize);
}

export function calculateMovingAverageViewportTrend(
  values,
  intervalMinutes,
  viewport,
  windowSize = 5
) {
  return summarizeMovingAverageTrend(
    values,
    intervalMinutes,
    windowSize,
    viewport?.start || 0,
    viewport?.end ?? values.length - 1
  );
}

export function aggregateDailyTokenCandles(
  rawEntries,
  sessions,
  intervalMinutes,
  utcDate,
  now = new Date()
) {
  const bucketCount = Math.ceil(1440 / intervalMinutes);
  const buckets = Array.from({ length: bucketCount }, (_, index) => {
    const startMinute = index * intervalMinutes;
    const endMinute = Math.min(1440, startMinute + intervalMinutes);
    const rangeStart = formatUtcMinuteInBrowserTime(utcDate, startMinute, true);
    const rangeEnd = formatUtcMinuteInBrowserTime(utcDate, endMinute, true);
    return {
      label: formatUtcMinuteInBrowserTime(utcDate, startMinute),
      rangeLabel: `${rangeStart}–${rangeEnd}`,
      startLabel: rangeStart,
      endLabel: rangeEnd,
      input: 0,
      output: 0,
      cache: 0,
      total: 0,
      cost: 0,
      open: 0,
      close: 0,
      direction: 0,
      changePercent: null,
      labelRow: 0,
      isFuture: isUtcBucketFuture(utcDate, startMinute, now),
    };
  });

  const entries = Array.isArray(rawEntries) ? rawEntries : [];
  const sessionTokenTotals = new Map();
  entries.forEach(entry => {
    const parts = normalizeEntryTokenParts(entry);
    if (parts.total <= 0) return;
    const sessionId = String(entry.session_id || '');
    sessionTokenTotals.set(sessionId, (sessionTokenTotals.get(sessionId) || 0) + parts.total);
  });

  const sessionCostPerToken = new Map();
  (Array.isArray(sessions) ? sessions : []).forEach(session => {
    const sessionId = String(session.session_id || '');
    const tokenTotal = sessionTokenTotals.get(sessionId) || Number(session.total_tokens) || 0;
    const cost = Math.max(0, Number(session.cost_usd) || 0);
    sessionCostPerToken.set(sessionId, tokenTotal > 0 ? cost / tokenTotal : 0);
  });

  entries.forEach(entry => {
    const parts = normalizeEntryTokenParts(entry);
    if (parts.total <= 0) return;
    const minute = getUsageEntryUtcMinute(entry.timestamp);
    if (minute === null) return;
    const bucket = buckets[Math.min(bucketCount - 1, Math.floor(minute / intervalMinutes))];
    if (bucket.isFuture) return;
    bucket.input += parts.input;
    bucket.output += parts.output;
    bucket.cache += parts.cache;
    bucket.total += parts.total;
    bucket.cost += parts.total * (sessionCostPerToken.get(String(entry.session_id || '')) || 0);
  });

  let cumulative = 0;
  let previousActiveTotal = null;
  let activeIndex = 0;
  buckets.forEach(bucket => {
    bucket.open = cumulative;
    cumulative += bucket.total;
    bucket.close = cumulative;
    if (bucket.total <= 0) return;

    if (previousActiveTotal !== null) {
      bucket.direction = bucket.total === previousActiveTotal
        ? 0
        : bucket.total > previousActiveTotal ? 1 : -1;
      bucket.changePercent = previousActiveTotal > 0
        ? ((bucket.total - previousActiveTotal) / previousActiveTotal) * 100
        : null;
    }
    bucket.labelRow = activeIndex % 3;
    activeIndex += 1;
    previousActiveTotal = bucket.total;
  });

  return buckets;
}
