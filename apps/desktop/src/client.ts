import { invoke, isTauri } from '@tauri-apps/api/core';
import type {
  Detail,
  EventCursor,
  Phase,
  Policy,
  SessionCursor,
  SetupInfo,
  SetupPreview,
  Snapshot,
} from './types';

export interface Client {
  native: boolean;
  dashboard(cursor?: SessionCursor | null): Promise<Snapshot>;
  detail(sessionId: string, cursor?: EventCursor | null): Promise<Detail>;
  changePhase(sessionId: string, expectedEpoch: number, phase: Phase): Promise<void>;
  policies(): Promise<Policy[]>;
  savePolicy(policy: Policy): Promise<void>;
  setupInfo(): Promise<SetupInfo>;
  previewSetup(currentJson: string, helperCommand: string): Promise<SetupPreview>;
  exportHistory(days: 7 | 30): Promise<{ csv: string; rows: number; truncated: boolean }>;
}
export const nativeAvailable = isTauri();
export const nativeClient: Client = {
  native: true,
  dashboard: (cursor = null) => invoke('dashboard', { cursor }),
  detail: (sessionId, cursor = null) => invoke('session_detail', { sessionId, cursor }),
  changePhase: (sessionId, expectedEpoch, phase) =>
    invoke('change_phase', { sessionId, expectedEpoch, phase }),
  policies: () => invoke('list_policies'),
  savePolicy: (policy) => invoke('save_policy', { policy }),
  setupInfo: () => invoke('setup_info'),
  previewSetup: (currentJson, helperCommand) => invoke('preview_setup', { currentJson, helperCommand }),
  exportHistory: (days) => invoke('export_history', { days }),
};

/** Explicit opt-in browser demo. This client never reads or writes native data. */
export function createDemoClient(): Client {
  const at = new Date(Date.now() - 120_000).toISOString();
  const integrations: Snapshot['integrations'] = [
    {
      id: 'demo-vscode',
      provider: 'codex',
      surface: 'codex_vscode_extension',
      accountKey: 'demo',
      health: 'unknown',
      targetVerification: 'unknown',
      lastSeenAt: at,
    },
    {
      id: 'demo-desktop',
      provider: 'codex',
      surface: 'codex_desktop_app',
      accountKey: 'demo',
      health: 'unknown',
      targetVerification: 'unknown',
      lastSeenAt: at,
    },
  ];
  const sessions: Snapshot['sessions'] = [
    {
      id: 'demo-session-1',
      integrationId: 'demo-vscode',
      sourceSessionId: 'demo-thread-vscode',
      activity: 'UNKNOWN',
      phase: 'PLANNING',
      phaseEpoch: 1,
      updatedAt: at,
      modelSelected: 'demo-codex-model',
      modelRequested: null,
      modelResolved: null,
      modelObserved: null,
      modelEvidenceKind: 'hook_common_metadata',
      modelObservedAt: null,
      modelEvidenceExpiresAt: null,
    },
    {
      id: 'demo-session-2',
      integrationId: 'demo-desktop',
      sourceSessionId: 'demo-thread-desktop',
      activity: 'UNKNOWN',
      phase: 'UNASSIGNED',
      phaseEpoch: 0,
      updatedAt: at,
      modelSelected: null,
      modelRequested: null,
      modelResolved: null,
      modelObserved: null,
      modelEvidenceKind: 'unknown',
      modelObservedAt: null,
      modelEvidenceExpiresAt: null,
    },
  ];
  const policies: Policy[] = [];
  const events = new Map<string, Detail['events']>(
    sessions.map((s) => [
      s.id,
      [
        {
          eventId: `event-${s.id}`,
          eventType: 'session.started',
          source: 'synthetic.demo',
          confidence: 'direct',
          observedAt: at,
          receivedAt: at,
          sourceInferenceId: null,
          phaseAtStart: null,
          phaseEpoch: null,
        },
      ],
    ]),
  );
  return {
    native: false,
    async dashboard() {
      return structuredClone({
        sessions,
        integrations,
        nextCursor: null,
        receiver: { running: false, address: null },
        generatedAt: new Date().toISOString(),
      });
    },
    async detail(sessionId) {
      const session = sessions.find((s) => s.id === sessionId);
      if (!session) throw new Error('Không tìm thấy phiên demo.');
      return structuredClone({
        session,
        events: events.get(sessionId) ?? [],
        nextCursor: null,
        usage: { items: [], partial: true },
        clockTrusted: true,
        quotas: ['five_hour', 'seven_day'].map((windowKey, index) => ({
          snapshot: {
            quotaKey: `demo-quota-${windowKey}`,
            provider: 'codex' as const,
            accountKey: 'demo',
            scope: 'account',
            bucket: 'synthetic',
            unit: 'percentage',
            windowKey,
            windowStart: null,
            windowEnd: null,
            resetsAt: null,
            used: null,
            limit: null,
            remaining: null,
            usedPercent: index === 0 ? 62.5 : null,
            observedAt: at,
            staleAfter: null,
          },
          sourceIntegrationId: 'demo-vscode',
          source: 'synthetic.demo',
          confidence: 'unknown',
          freshness: 'unknown' as const,
        })),
        policy: {
          decision: 'indeterminate',
          reason: session.phase === 'UNASSIGNED' ? 'unassigned_phase' : 'missing_served_evidence',
          shouldNotify: false,
          selectionMismatchWarning: false,
          winningPolicyIds: [],
          shadowedPolicyIds: [],
          enforcement: 'advisory',
        },
      });
    },
    async changePhase(id, epoch, phase) {
      const session = sessions.find((s) => s.id === id);
      if (!session || session.phaseEpoch !== epoch) throw new Error('Phase đã thay đổi. Hãy làm mới.');
      session.phase = phase;
      session.phaseEpoch++;
      session.updatedAt = new Date().toISOString();
      session.modelSelected = null;
      session.modelRequested = null;
      session.modelResolved = null;
      session.modelObserved = null;
      session.modelEvidenceKind = 'unknown';
      events.get(id)?.push({
        eventId: crypto.randomUUID(),
        eventType: 'phase.changed',
        source: 'aum.user.demo',
        confidence: 'direct',
        observedAt: session.updatedAt,
        receivedAt: session.updatedAt,
        sourceInferenceId: null,
        phaseAtStart: phase,
        phaseEpoch: session.phaseEpoch,
      });
    },
    async policies() {
      return structuredClone(policies);
    },
    async savePolicy(policy) {
      const index = policies.findIndex((p) => p.id === policy.id);
      if (index < 0) policies.push(structuredClone(policy));
      else policies[index] = structuredClone(policy);
    },
    async setupInfo() {
      return { helperExecutable: '', helperExists: false, profiles: [] };
    },
    async previewSetup() {
      throw new Error('Mở ứng dụng desktop để tạo bản đề xuất cấu hình.');
    },
    async exportHistory() {
      return { csv: 'Data\r\nSynthetic demo only\r\n', rows: 1, truncated: false };
    },
  };
}
