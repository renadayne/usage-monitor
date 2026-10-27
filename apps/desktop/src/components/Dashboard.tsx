import React, { useMemo, useState } from 'react';
import type { Integration, Session, Snapshot } from '../types';

export interface DashboardProps {
  snapshot: Snapshot;
  selectedId: string | null;
  onSelect: (session: Session) => void;
  onRefresh: () => void;
  onSetup: () => void;
}

const isServedEvidence = (s: Session): boolean =>
  s.modelEvidenceKind === 'served_response' && Boolean(s.modelObserved?.trim());

const formatTime = (value: string | null): string =>
  value && Number.isFinite(Date.parse(value))
    ? new Date(value).toLocaleString('vi-VN', {
        day: '2-digit',
        month: '2-digit',
        hour: '2-digit',
        minute: '2-digit',
      })
    : 'Chưa có dữ liệu';
const surfaceName = (value: string | undefined): string =>
  value === 'codex_vscode_extension'
    ? 'Codex · VS Code'
    : value === 'codex_desktop_app'
      ? 'Codex · Desktop'
      : (value ?? 'Chưa rõ nguồn');

const getEvidenceFreshness = (s: Session, generatedAt: string) => {
  const obs = s.modelObservedAt ? Date.parse(s.modelObservedAt) : NaN;
  const exp = s.modelEvidenceExpiresAt ? Date.parse(s.modelEvidenceExpiresAt) : NaN;
  const gen = generatedAt ? Date.parse(generatedAt) : NaN;
  if (Number.isNaN(obs) || Number.isNaN(exp) || Number.isNaN(gen) || obs > gen) {
    return { badgeClass: 'badge badge--warning badge--freshness-unknown', label: 'Độ mới chưa rõ' };
  }
  return gen >= exp
    ? { badgeClass: 'badge badge--stale', label: 'Đã cũ' }
    : { badgeClass: 'badge badge--fresh', label: 'Còn hạn' };
};

const formatActivity = (activity: string): string => (activity === 'UNKNOWN' ? 'Chưa rõ' : activity);

export function Dashboard({ snapshot, selectedId, onSelect, onRefresh, onSetup }: DashboardProps) {
  const [providerFilter, setProviderFilter] = useState<string>('ALL');
  const [evidenceFilter, setEvidenceFilter] = useState<string>('ALL');
  const [modelQuery, setModelQuery] = useState<string>('');

  const integrationById = useMemo(
    () => new Map<string, Integration>(snapshot.integrations.map((i) => [i.id, i])),
    [snapshot.integrations],
  );
  const workingCount = useMemo(
    () => snapshot.sessions.filter((s) => s.activity === 'WORKING').length,
    [snapshot.sessions],
  );
  const servedCount = useMemo(() => snapshot.sessions.filter(isServedEvidence).length, [snapshot.sessions]);
  const evidenceKinds = useMemo(
    () => Array.from(new Set(snapshot.sessions.map((s) => s.modelEvidenceKind))),
    [snapshot.sessions],
  );

  const filteredSessions = useMemo(() => {
    const q = modelQuery.trim().toLowerCase();
    return snapshot.sessions.filter((s) => {
      const integ = integrationById.get(s.integrationId);
      if (providerFilter !== 'ALL' && integ?.provider !== providerFilter) return false;
      if (evidenceFilter !== 'ALL' && s.modelEvidenceKind !== evidenceFilter) return false;
      if (
        q &&
        ![s.modelSelected, s.modelRequested, s.modelResolved, s.modelObserved]
          .filter(Boolean)
          .join(' ')
          .toLowerCase()
          .includes(q)
      )
        return false;
      return true;
    });
  }, [snapshot.sessions, integrationById, providerFilter, evidenceFilter, modelQuery]);

  return (
    <section className="dashboard" aria-label="Phiên làm việc">
      <header className="dashboard__header">
        <div className="dashboard__title-group">
          <h1 className="dashboard__title">Phiên làm việc</h1>
          <span className="dashboard__snapshot-time">Đọc dữ liệu lúc {formatTime(snapshot.generatedAt)}</span>
        </div>
        <div className="dashboard__actions">
          <button type="button" className="btn btn--secondary" onClick={onRefresh}>
            Làm mới
          </button>
          <button type="button" className="btn btn--primary" onClick={onSetup}>
            Thiết lập Codex
          </button>
        </div>
      </header>
      <div className="dashboard__receiver-banner" role="status">
        <span className="badge badge--receiver">
          {snapshot.receiver.running
            ? `Bộ thu cục bộ sẵn sàng · ${snapshot.receiver.address ?? 'local'}`
            : 'Bộ thu đang dừng'}
        </span>
        <span className="badge badge--warning">Chưa xác minh</span>
      </div>
      <section className="dashboard__summary" aria-label="Tóm tắt phiên">
        <div className="summary-card">
          <span className="summary-card__label">Phiên đã ghi nhận</span>
          <strong className="summary-card__value">{snapshot.sessions.length}</strong>
        </div>
        <div className="summary-card">
          <span className="summary-card__label">Đang làm việc</span>
          <strong className="summary-card__value">{workingCount}</strong>
        </div>
        <div className="summary-card">
          <span className="summary-card__label">Có response evidence</span>
          <strong className="summary-card__value">{servedCount}</strong>
        </div>
      </section>
      <section className="dashboard__filters" aria-label="Bộ lọc phiên">
        <label className="filter-field">
          <span>Nhà cung cấp</span>
          <select value={providerFilter} onChange={(e) => setProviderFilter(e.target.value)}>
            <option value="ALL">Tất cả</option>
            {(['codex', 'antigravity', 'copilot', 'claude_code'] as const).map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
        </label>
        <label className="filter-field">
          <span>Bằng chứng</span>
          <select value={evidenceFilter} onChange={(e) => setEvidenceFilter(e.target.value)}>
            <option value="ALL">Tất cả</option>
            {evidenceKinds.map((kind) => (
              <option key={kind} value={kind}>
                {kind}
              </option>
            ))}
          </select>
        </label>
        <label className="filter-field">
          <span>Model</span>
          <input
            type="search"
            value={modelQuery}
            onChange={(e) => setModelQuery(e.target.value)}
            placeholder="Lọc theo model..."
          />
        </label>
      </section>
      {filteredSessions.length === 0 ? (
        <div className="dashboard__empty" role="region" aria-label="Trạng thái trống">
          <p>Chưa ghi nhận phiên làm việc nào.</p>
          <button type="button" className="btn btn--primary" onClick={onSetup}>
            Thiết lập Codex
          </button>
        </div>
      ) : (
        <ul className="session-list" aria-label="Danh sách phiên">
          {filteredSessions.map((s) => {
            const integ = integrationById.get(s.integrationId);
            const freshness = getEvidenceFreshness(s, snapshot.generatedAt);
            const isSelected = selectedId === s.id;
            const servedValue = isServedEvidence(s) ? s.modelObserved! : 'Chưa có dữ liệu';
            const verifiedLabel =
              !integ || integ.targetVerification === 'unknown' ? 'Chưa xác minh' : integ.targetVerification;
            return (
              <li key={s.id} className={`session-card ${isSelected ? 'session-card--selected' : ''}`}>
                <button
                  type="button"
                  className="session-card__select"
                  aria-pressed={isSelected}
                  onClick={() => onSelect(s)}
                >
                  <div className="session-card__top">
                    <span className="session-card__source-id">{surfaceName(integ?.surface)}</span>
                    <span className="badge badge--activity">{formatActivity(s.activity)}</span>
                    <span className="badge badge--phase">
                      {s.phase} · E{s.phaseEpoch}
                    </span>
                    <span className={freshness.badgeClass}>{freshness.label}</span>
                  </div>
                  <dl className="session-card__models">
                    <div>
                      <dt>Selected</dt>
                      <dd>{s.modelSelected || 'Chưa có dữ liệu'}</dd>
                    </div>
                    <div>
                      <dt>Requested</dt>
                      <dd>{s.modelRequested || 'Chưa có dữ liệu'}</dd>
                    </div>
                    <div>
                      <dt>Resolved</dt>
                      <dd>{s.modelResolved || 'Chưa có dữ liệu'}</dd>
                    </div>
                    <div>
                      <dt>Served</dt>
                      <dd>{servedValue}</dd>
                    </div>
                  </dl>
                  <div className="session-card__meta">
                    <span>Quan sát gần nhất: {formatTime(s.updatedAt)}</span>
                    <span className="badge badge--warning">{verifiedLabel}</span>
                  </div>
                </button>
                <details className="session-card__details">
                  <summary>Bằng chứng</summary>
                  <div className="session-card__meta">
                    <span>
                      Phiên nguồn: {s.sourceSessionId || 'Chưa có dữ liệu'} · ID: {s.id}
                    </span>
                    <span>
                      Loại: {s.modelEvidenceKind} · Served: {servedValue}
                    </span>
                    <span>
                      Lúc quan sát: {s.modelObservedAt || 'Chưa có dữ liệu'} · Hết hạn:{' '}
                      {s.modelEvidenceExpiresAt || 'Chưa có dữ liệu'}
                    </span>
                    <span>
                      Đích tích hợp: {verifiedLabel} ({integ?.health || 'unknown'})
                    </span>
                  </div>
                </details>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
