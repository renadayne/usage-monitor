import { describe, expect, it, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => false, invoke: vi.fn(async () => undefined) }));
import { invoke } from '@tauri-apps/api/core';
import { createDemoClient, nativeAvailable, nativeClient } from './client';
import { Dashboard } from './components/Dashboard';
import App from './App';

describe('evidence semantics in the desktop view', () => {
  it('never treats a selection or a similarly named evidence kind as served proof', async () => {
    const snapshot = await createDemoClient().dashboard();
    snapshot.sessions[0].modelEvidenceKind = 'not_served';
    snapshot.sessions[0].modelObserved = 'unproven-model';
    const html = renderToStaticMarkup(
      <Dashboard
        snapshot={snapshot}
        selectedId={null}
        onSelect={() => {}}
        onRefresh={() => {}}
        onSetup={() => {}}
      />,
    );
    expect(html).not.toContain('unproven-model');
    expect(html).not.toContain('badge--fresh"');
    expect(html).toContain('Chưa xác minh');
    expect(html).toContain('Chưa rõ');
  });
  it('requires a valid observation/expiry interval before showing fresh evidence', async () => {
    const snapshot = await createDemoClient().dashboard();
    const session = snapshot.sessions[0];
    session.modelEvidenceKind = 'served_response';
    session.modelObserved = 'response-model';
    const render = () =>
      renderToStaticMarkup(
        <Dashboard
          snapshot={snapshot}
          selectedId={null}
          onSelect={() => {}}
          onRefresh={() => {}}
          onSetup={() => {}}
        />,
      );
    expect(render()).not.toContain('Còn hạn');
    session.modelObservedAt = new Date(Date.now() + 60_000).toISOString();
    session.modelEvidenceExpiresAt = new Date(Date.now() + 120_000).toISOString();
    expect(render()).not.toContain('Còn hạn');
    session.modelObservedAt = new Date(Date.now() - 120_000).toISOString();
    session.modelEvidenceExpiresAt = new Date(Date.now() - 60_000).toISOString();
    expect(render()).toContain('Đã cũ');
    session.modelEvidenceExpiresAt = new Date(Date.now() + 120_000).toISOString();
    expect(render()).toContain('Còn hạn');
  });
});

describe('demo and native authority boundaries', () => {
  it('requires an explicit demo choice in a normal browser', () => {
    expect(nativeAvailable).toBe(false);
    const html = renderToStaticMarkup(<App />);
    expect(html).toContain('Khám phá demo');
    expect(html).not.toContain('demo-thread-vscode');
    expect(invoke).not.toHaveBeenCalled();
  });
  it('keeps demo state isolated, resets model evidence on phase changes and checks the epoch', async () => {
    const first = createDemoClient();
    const second = createDemoClient();
    const original = (await first.dashboard()).sessions[0];
    await first.changePhase(original.id, original.phaseEpoch, 'REVIEW');
    const changed = await first.detail(original.id);
    expect(changed.session.phase).toBe('REVIEW');
    expect(changed.session.modelSelected).toBeNull();
    expect(changed.policy.decision).toBe('indeterminate');
    expect(changed.events.at(-1)?.eventType).toBe('phase.changed');
    await expect(first.changePhase(original.id, original.phaseEpoch, 'DONE')).rejects.toThrow();
    expect((await second.dashboard()).sessions[0].modelSelected).toBe('demo-codex-model');
  });
  it('passes phase authority only through explicit local IPC with compare-and-set epoch', async () => {
    await nativeClient.changePhase('session', 7, 'REVIEW');
    expect(invoke).toHaveBeenLastCalledWith('change_phase', {
      sessionId: 'session',
      expectedEpoch: 7,
      phase: 'REVIEW',
    });
  });
  it('passes bounded exception intent and session binding through explicit native IPC', async () => {
    const request = {
      id: 'override',
      policyId: 'rule',
      sessionId: 'session',
      expectedPhaseEpoch: 7,
      replacementModelIds: ['model'],
      reason: 'Synthetic review exception',
      durationSeconds: 900,
    };
    await nativeClient.createOverride(request);
    expect(invoke).toHaveBeenLastCalledWith('create_override', { request });
    await nativeClient.revokeOverride('override', 'session');
    expect(invoke).toHaveBeenLastCalledWith('revoke_override', { id: 'override', sessionId: 'session' });
  });
});
