import { describe, it, expect } from 'vitest';
import { quotaFreshness, quotaNumber } from './quota';
import type { AccountQuota } from './types';

const quota: AccountQuota = {
  snapshot: {
    quotaKey: 'q',
    provider: 'codex',
    accountKey: 'a',
    scope: 'account',
    bucket: 'five_hour',
    unit: 'percentage',
    windowKey: 'w',
    windowStart: null,
    windowEnd: null,
    resetsAt: '2026-10-28T02:00:00Z',
    used: null,
    limit: null,
    remaining: null,
    usedPercent: 125,
    observedAt: '2026-10-28T01:00:00Z',
    staleAfter: '2026-10-28T01:05:00Z',
  },
  sourceIntegrationId: 'i',
  source: 'synthetic',
  confidence: 'direct',
  freshness: 'fresh',
};
describe('quota presentation', () => {
  it('expires cached facts at exact TTL/reset boundaries and never upgrades unknown evidence', () => {
    expect(quotaFreshness(quota, Date.parse('2026-10-28T01:04:59Z'), true)).toBe('fresh');
    expect(quotaFreshness(quota, Date.parse('2026-10-28T01:05:00Z'), true)).toBe('stale');
    expect(quotaFreshness(quota, Date.parse('2026-10-28T02:00:00Z'), true)).toBe('expired');
    expect(quotaFreshness(quota, Date.parse('2026-10-28T01:00:00Z'), false)).toBe('unknown');
    expect(quotaFreshness({ ...quota, freshness: 'unknown' }, Date.parse('2026-10-28T01:00:00Z'), true)).toBe(
      'unknown',
    );
  });
  it('preserves absence, explicit zero and overage', () => {
    expect(quotaNumber(null)).toBe('Chưa có dữ liệu');
    expect(quotaNumber(0)).toBe('0');
    expect(quotaNumber(-50)).toBe('-50');
    expect(quotaNumber(quota.snapshot.usedPercent)).toBe('125');
  });
});
