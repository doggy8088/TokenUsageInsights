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

export function getUsageEntryMinute(timestamp) {
  const parsed = new Date(timestamp);
  if (!Number.isNaN(parsed.getTime())) {
    return parsed.getHours() * 60 + parsed.getMinutes();
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

export function calculateMovingAverageTrend(candles, intervalMinutes, windowSize = 5) {
  const values = calculateCumulativeMovingAverage(candles, windowSize);
  let lastIndex = -1;
  for (let index = values.length - 1; index >= 0; index -= 1) {
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

export function aggregateDailyTokenCandles(rawEntries, sessions, intervalMinutes) {
  const bucketCount = Math.ceil(1440 / intervalMinutes);
  const buckets = Array.from({ length: bucketCount }, (_, index) => {
    const startMinute = index * intervalMinutes;
    const endMinute = Math.min(1440, startMinute + intervalMinutes);
    return {
      label: formatMinuteOfDay(startMinute),
      rangeLabel: `${formatMinuteOfDay(startMinute)}–${formatMinuteOfDay(endMinute)}`,
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
    const minute = getUsageEntryMinute(entry.timestamp);
    if (minute === null) return;
    const bucket = buckets[Math.min(bucketCount - 1, Math.floor(minute / intervalMinutes))];
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
