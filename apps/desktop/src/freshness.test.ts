import { expect, it } from 'vitest';
import { evidenceFreshness, currentEvaluation } from './freshness';
import type { Evaluation, Session } from './types';

const session: Session = {
  id: 's',
  integrationId: 'i',
  sourceSessionId: 'source',
  activity: 'UNKNOWN',
  phase: 'REVIEW',
  phaseEpoch: 1,
  updatedAt: null,
  modelSelected: null,
  modelRequested: null,
  modelResolved: null,
  modelObserved: 'served-model',
  modelEvidenceKind: 'served_response',
  modelObservedAt: '2026-10-28T00:00:00Z',
  modelEvidenceExpiresAt: '2026-10-28T00:01:00Z',
};
const evaluation: Evaluation = {
  decision: 'compliant',
  reason: 'model_allowed',
  shouldNotify: false,
  selectionMismatchWarning: false,
  winningPolicyIds: ['rule'],
  shadowedPolicyIds: [],
  enforcement: 'advisory',
};

it('expires cached positive policy state at the evidence boundary without waiting for another IPC fetch', () => {
  const expiry = Date.parse(session.modelEvidenceExpiresAt!);
  expect(evidenceFreshness(session, expiry - 1)).toBe('fresh');
  expect(evidenceFreshness(session, expiry)).toBe('expired');
  expect(currentEvaluation(evaluation, session, expiry).decision).toBe('indeterminate');
  expect(
    currentEvaluation({ ...evaluation, decision: 'violation', shouldNotify: true }, session, expiry)
      .shouldNotify,
  ).toBe(false);
  expect(currentEvaluation(evaluation, session, expiry - 1, false).reason).toBe('clock_untrusted');
  expect(
    currentEvaluation(evaluation, { ...session, modelEvidenceKind: 'hook_common_metadata' }, expiry - 1)
      .decision,
  ).toBe('indeterminate');
});
