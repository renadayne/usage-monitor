import React from 'react';
import type { Detail, Phase, Session } from '../types';
import { currentEvaluation } from '../freshness';
import { QuotaWidgets } from './QuotaWidgets';
import { OverridePanel } from './OverridePanel';
import type { Client } from '../client';

export interface SessionTimelineProps {
  client: Client;
  onOverrideChanged: () => void;
  session: Session;
  detail: Detail | null;
  loading: boolean;
  error: string | null;
  onPhaseChange: (phase: Phase) => void;
  onLoadMore: () => void;
}

const PHASES: Phase[] = ['UNASSIGNED', 'PLANNING', 'IMPLEMENTATION', 'REVIEW', 'DONE'];

const DECISION_LABELS: Record<string, string> = {
  compliant: 'Tuân thủ',
  violation: 'Vi phạm model thực thi',
  not_applicable: 'Không áp dụng',
  indeterminate: 'Chưa xác định',
};

const REASON_LABELS: Record<string, string> = {
  override_expired_refresh_required: 'Ngoại lệ đã hết hạn; đang chờ đánh giá mới',
  unassigned_phase: 'Chưa gán phase cho phiên này',
  missing_served_evidence: 'Chưa có bằng chứng model thực thi',
  no_applicable_policy: 'Không có quy tắc áp dụng',
  stale_or_future_evidence: 'Bằng chứng đã cũ hoặc thời gian không hợp lệ',
  clock_untrusted: 'Đồng hồ hệ thống không đáng tin cậy',
  model_allowed: 'Model thực thi nằm trong danh sách cho phép',
  model_not_allowed: 'Model thực thi không được phép trong phase này',
};

const formatLocalTime = (iso: string): string => {
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? iso : new Date(ms).toLocaleString('vi-VN');
};

export function SessionTimeline({
  client,
  onOverrideChanged,
  session,
  detail,
  loading,
  error,
  onPhaseChange,
  onLoadMore,
}: SessionTimelineProps) {
  const selectId = `phase-select-${session.id}`;
  const evaluation = detail
    ? currentEvaluation(detail.policy, session, Date.now(), detail.clockTrusted)
    : null;
  return (
    <section className="session-timeline" aria-label="Chi tiết phiên">
      <header className="session-timeline__header">
        <div>
          <h2 className="session-timeline__title">Phiên: {session.sourceSessionId ?? session.id}</h2>
          <p className="session-timeline__subtitle">
            Epoch hiện tại: <strong>{session.phaseEpoch}</strong>
          </p>
        </div>
        <div className="session-timeline__phase-control">
          <label htmlFor={selectId}>Phase</label>
          <select
            id={selectId}
            value={session.phase}
            disabled={loading}
            onChange={(e) => onPhaseChange(e.target.value as Phase)}
          >
            {PHASES.map((p) => (
              <option key={p} value={p}>
                {p}
              </option>
            ))}
          </select>
          <span className="badge badge--epoch">Epoch {session.phaseEpoch}</span>
        </div>
      </header>
      {error && (
        <div className="alert alert--error" role="alert">
          Lỗi: {error}
        </div>
      )}
      {loading && !detail && (
        <p className="session-timeline__loading" role="status">
          Đang tải...
        </p>
      )}
      {detail && (
        <>
          <section className="session-timeline__policy" aria-label="Chính sách">
            <h3>Chính sách</h3>
            <div className={`policy-decision policy-decision--${evaluation!.decision}`}>
              <strong>{DECISION_LABELS[evaluation!.decision] ?? evaluation!.decision}</strong>
              <span> · {REASON_LABELS[evaluation!.reason] ?? evaluation!.reason}</span>
            </div>
            {evaluation!.selectionMismatchWarning && (
              <div className="alert alert--warning" role="status">
                Cảnh báo lệch model đã chọn so với quy tắc phase.
              </div>
            )}
            <details className="policy-details">
              <summary>Chi tiết đánh giá</summary>
              <div>
                Mã lý do: {evaluation!.reason} · Thực thi: {evaluation!.enforcement}
              </div>
            </details>
          </section>
          <section className="session-timeline__usage" aria-label="Mức sử dụng">
            <div className="usage-header">
              <h3>Mức sử dụng</h3>
              {detail.usage.partial && <span className="badge badge--partial">Một phần</span>}
            </div>
            {detail.usage.items.length === 0 ? (
              <p>Chưa có dữ liệu.</p>
            ) : (
              <table className="usage-table">
                <thead>
                  <tr>
                    <th scope="col">Chỉ số</th>
                    <th scope="col">Đơn vị</th>
                    <th scope="col">Quan hệ</th>
                    <th scope="col">Giá trị</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.usage.items.map((item, idx) => (
                    <tr key={`${item.metric}-${item.unit}-${item.relationToTotal}-${idx}`}>
                      <td>{item.metric}</td>
                      <td>{item.unit}</td>
                      <td>{item.relationToTotal}</td>
                      <td>{item.value}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
          <QuotaWidgets quotas={detail.quotas} clockTrusted={detail.clockTrusted} />
          <OverridePanel key={session.id} client={client} detail={detail} onChanged={onOverrideChanged} />
          <section className="session-timeline__events" aria-label="Sự kiện">
            <h3>Dòng thời gian sự kiện</h3>
            {detail.events.length === 0 ? (
              <p>Chưa có sự kiện.</p>
            ) : (
              <ol className="event-list">
                {detail.events.map((ev) => (
                  <li key={ev.eventId} className="event-item">
                    <details>
                      <summary>
                        {ev.eventType} · {formatLocalTime(ev.observedAt)}
                      </summary>
                      <div className="event-item__meta">
                        <span>
                          Nguồn: {ev.source} · Độ tin cậy: {ev.confidence}
                        </span>
                        <span>Nhận lúc: {formatLocalTime(ev.receivedAt)}</span>
                        <span>
                          Phase: {ev.phaseAtStart ?? 'Chưa có dữ liệu'} (E{ev.phaseEpoch ?? '—'}) · Inference:{' '}
                          {ev.sourceInferenceId ?? 'Chưa có dữ liệu'}
                        </span>
                      </div>
                    </details>
                  </li>
                ))}
              </ol>
            )}
            {detail.nextCursor && (
              <button
                type="button"
                className="btn btn--secondary session-timeline__load-more"
                disabled={loading}
                onClick={onLoadMore}
              >
                {loading ? 'Đang tải...' : 'Tải thêm'}
              </button>
            )}
          </section>
        </>
      )}
    </section>
  );
}
