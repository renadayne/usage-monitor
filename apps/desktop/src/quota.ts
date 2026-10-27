import type { AccountQuota } from './types';

export function quotaFreshness(
  quota: AccountQuota,
  now: number,
  clockTrusted?: boolean,
): AccountQuota['freshness'] {
  if (!clockTrusted || !Number.isFinite(now)) return 'unknown';
  const s = quota.snapshot;
  const observed = Date.parse(s.observedAt);
  if (!Number.isFinite(observed) || observed > now) return 'unknown';
  for (const time of [s.windowStart, s.windowEnd, s.resetsAt, s.staleAfter]) {
    if (time !== null && !Number.isFinite(Date.parse(time))) return 'unknown';
  }
  if (s.windowStart && Date.parse(s.windowStart) > now) return 'unknown';
  if ([s.windowEnd, s.resetsAt].some((t) => t !== null && Date.parse(t) <= now)) return 'expired';
  if (s.staleAfter && Date.parse(s.staleAfter) <= now) return 'stale';
  if (quota.freshness === 'fresh' && (!s.staleAfter || quota.confidence !== 'direct')) return 'unknown';
  return quota.freshness;
}

export const quotaNumber = (value: number | null): string =>
  value === null || !Number.isFinite(value) ? 'Chưa có dữ liệu' : value.toLocaleString('vi-VN');
