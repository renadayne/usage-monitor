export type Phase = 'UNASSIGNED' | 'PLANNING' | 'IMPLEMENTATION' | 'REVIEW' | 'DONE';
export type Provider = 'codex' | 'antigravity' | 'copilot' | 'claude_code';
export interface Integration {
  id: string;
  provider: Provider;
  surface: string;
  accountKey: string;
  health: string;
  targetVerification: string;
  lastSeenAt: string | null;
}
export interface Session {
  id: string;
  integrationId: string;
  sourceSessionId: string | null;
  activity: string;
  phase: Phase;
  phaseEpoch: number;
  updatedAt: string | null;
  modelSelected: string | null;
  modelRequested: string | null;
  modelResolved: string | null;
  modelObserved: string | null;
  modelEvidenceKind: string;
  modelObservedAt: string | null;
  modelEvidenceExpiresAt: string | null;
}
export interface Event {
  eventId: string;
  eventType: string;
  source: string;
  confidence: string;
  observedAt: string;
  receivedAt: string;
  sourceInferenceId: string | null;
  phaseAtStart: Phase | null;
  phaseEpoch: number | null;
}
export interface SessionCursor {
  updatedAt: string;
  id: string;
}
export interface EventCursor {
  observedAt: string;
  eventId: string;
}
export interface Snapshot {
  sessions: Session[];
  nextCursor: SessionCursor | null;
  integrations: Integration[];
  receiver: { running: boolean; address: string | null };
  generatedAt: string;
}
export interface Usage {
  items: { unit: string; metric: string; relationToTotal: string; value: number }[];
  partial: boolean;
}
export interface Policy {
  id: string;
  scope: { provider: Provider | null; projectKey: string | null; sessionId: string | null };
  phase: Phase;
  allowedModelIds: string[];
  priority: number;
  action: 'observe' | 'notify';
  enabled: boolean;
}
export interface Evaluation {
  decision: 'compliant' | 'violation' | 'not_applicable' | 'indeterminate';
  reason: string;
  shouldNotify: boolean;
  selectionMismatchWarning: boolean;
  winningPolicyIds: string[];
  shadowedPolicyIds: string[];
  enforcement: string;
}
export interface Detail {
  session: Session;
  events: Event[];
  nextCursor: EventCursor | null;
  usage: Usage;
  policy: Evaluation;
}
export interface SetupPreview {
  mergedConfig: string;
  addedEvents: string[];
  skippedEvents: string[];
  requiresTrust: boolean;
}

export interface SetupInfo {
  helperExecutable: string;
  helperExists: boolean;
  profiles: { surface: string; configPath: string }[];
}
