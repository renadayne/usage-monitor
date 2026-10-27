import type { Evaluation, Session } from './types';

export type Freshness = 'unknown' | 'expired' | 'fresh';

/** A UI freshness hint; durable evidence and policy evaluation remain backend responsibilities. */
export function evidenceFreshness(session: Session, now: number, clockTrusted = true): Freshness {
  if (!clockTrusted || session.modelEvidenceKind !== 'served_response' || !session.modelObserved?.trim())
    return 'unknown';
  const observed = Date.parse(session.modelObservedAt ?? '');
  const expires = Date.parse(session.modelEvidenceExpiresAt ?? '');
  if (![now, observed, expires].every(Number.isFinite) || expires <= observed || observed > now)
    return 'unknown';
  return now >= expires ? 'expired' : 'fresh';
}

/** A cached positive decision must stop displaying as current proof after its evidence expires. */
export function currentEvaluation(
  evaluation: Evaluation,
  session: Session,
  now: number,
  clockTrusted = true,
): Evaluation {
  if (!clockTrusted && evaluation.decision !== 'not_applicable') {
    return {
      ...evaluation,
      decision: 'indeterminate',
      reason: 'clock_untrusted',
      shouldNotify: false,
      selectionMismatchWarning: false,
    };
  }
  if (evaluation.overrideExpiresAtMs != null && now >= evaluation.overrideExpiresAtMs) {
    return {
      ...evaluation,
      decision: 'indeterminate',
      reason: 'override_expired_refresh_required',
      shouldNotify: false,
      selectionMismatchWarning: false,
    };
  }
  if (!['compliant', 'violation'].includes(evaluation.decision)) return evaluation;
  const freshness = evidenceFreshness(session, now, clockTrusted);
  if (freshness === 'fresh') return evaluation;
  return {
    ...evaluation,
    decision: 'indeterminate',
    reason: !clockTrusted
      ? 'clock_untrusted'
      : freshness === 'expired'
        ? 'stale_or_future_evidence'
        : 'missing_served_evidence',
    shouldNotify: false,
    selectionMismatchWarning: false,
  };
}
