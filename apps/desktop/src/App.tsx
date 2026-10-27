import { useEffect, useRef, useState } from 'react';
import { nativeAvailable, nativeClient, createDemoClient } from './client';
import type { Client } from './client';
import type { Detail, Phase, Session, Snapshot } from './types';
import { Dashboard } from './components/Dashboard';
import { SessionTimeline } from './components/SessionTimeline';
import { PolicyEditor } from './components/PolicyEditor';
import { SetupWizard } from './components/SetupWizard';

type Page = 'sessions' | 'history' | 'policies' | 'setup';
const nav: [Page, string, string][] = [
  ['sessions', '◫', 'Tổng quan'],
  ['history', '◷', 'Lịch sử'],
  ['policies', '◇', 'Chính sách'],
  ['setup', '⌘', 'Thiết lập'],
];

export default function App() {
  const [, setClockTick] = useState(0);
  const [client, setClient] = useState<Client | null>(nativeAvailable ? nativeClient : null);
  const clientRef = useRef(client);
  const selectionRef = useRef<string | null>(null);
  const [page, setPage] = useState<Page>('sessions');
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [selected, setSelected] = useState<Session | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [error, setError] = useState('');
  const [detailError, setDetailError] = useState('');
  const [loading, setLoading] = useState(false);
  const [moreBusy, setMoreBusy] = useState(false);
  const [exportBusy, setExportBusy] = useState(false);
  const [days, setDays] = useState<7 | 30>(7);
  const [notice, setNotice] = useState('');
  const demo = client !== null && !client.native;
  const needsFreshnessTick =
    (page === 'sessions' || page === 'history') &&
    Boolean(
      snapshot?.sessions.some(
        (s) => s.modelEvidenceExpiresAt && Date.parse(s.modelEvidenceExpiresAt) > Date.now(),
      ) ||
      (detail?.session.modelEvidenceExpiresAt &&
        Date.parse(detail.session.modelEvidenceExpiresAt) > Date.now()) ||
      detail?.quotas.some((q) =>
        [q.snapshot.staleAfter, q.snapshot.windowEnd, q.snapshot.resetsAt].some(
          (t) => t && Date.parse(t) > Date.now(),
        ),
      ) ||
      detail?.overrides.some((o) => !o.revokedAt && Date.parse(o.expiresAt) > Date.now()) ||
      (detail?.policy.overrideExpiresAtMs && detail.policy.overrideExpiresAtMs > Date.now()),
    );
  useEffect(() => {
    if (!needsFreshnessTick) return;
    const timer = window.setInterval(() => setClockTick((t) => t + 1), 1000);
    return () => window.clearInterval(timer);
  }, [needsFreshnessTick]);

  useEffect(() => {
    if (!client) return;
    let active = true;
    let pending = false;
    const load = async () => {
      if (pending) return;
      pending = true;
      try {
        const next = await client.dashboard();
        if (active) {
          setError('');
          setSnapshot((previous) => {
            if (!previous || previous.sessions.length <= 100) return next;
            const seen = new Map(previous.sessions.map((s) => [s.id, s]));
            next.sessions.forEach((s) => seen.set(s.id, s));
            return { ...next, sessions: [...seen.values()], nextCursor: previous.nextCursor };
          });
        }
      } catch {
        if (active) setError('Không đọc được dữ liệu desktop. Bộ thu có thể đang dừng.');
      } finally {
        pending = false;
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [client, refresh]);

  useEffect(() => {
    if (!client || !selected) return;
    let active = true;
    let pending = false;
    const load = async () => {
      if (pending) return;
      pending = true;
      setLoading(true);
      try {
        const next = await client.detail(selected.id);
        if (active) {
          setDetail((previous) => {
            if (
              !previous ||
              previous.session.id !== selected.id ||
              previous.events.length <= next.events.length
            )
              return next;
            const events = new Map(previous.events.map((e) => [e.eventId, e]));
            next.events.forEach((e) => events.set(e.eventId, e));
            return { ...next, events: [...events.values()], nextCursor: previous.nextCursor };
          });
          setDetailError('');
        }
      } catch {
        if (active) setDetailError('Không đọc được chi tiết phiên.');
      } finally {
        pending = false;
        if (active) setLoading(false);
      }
    };
    setDetail(null);
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [client, selected?.id, refresh]);

  function switchClient(next: Client | null) {
    clientRef.current = next;
    selectionRef.current = null;
    setClient(next);
    setSnapshot(null);
    setSelected(null);
    setDetail(null);
    setError('');
    setNotice('');
    setPage('sessions');
  }
  function selectSession(session: Session | null) {
    selectionRef.current = session?.id ?? null;
    setSelected(session);
  }
  async function changePhase(phase: Phase) {
    const session = detail?.session ?? selected;
    if (!client || !session) return;
    setLoading(true);
    setDetailError('');
    try {
      await client.changePhase(session.id, session.phaseEpoch, phase);
      if (clientRef.current === client) setRefresh((n) => n + 1);
    } catch {
      if (clientRef.current === client)
        setDetailError('Không đổi được phase. Có thể epoch đã thay đổi; hãy làm mới.');
    } finally {
      if (clientRef.current === client) setLoading(false);
    }
  }
  async function moreSessions() {
    if (!client || !snapshot?.nextCursor) return;
    setMoreBusy(true);
    try {
      const next = await client.dashboard(snapshot.nextCursor);
      if (clientRef.current !== client) return;
      setSnapshot((previous) =>
        previous
          ? {
              ...previous,
              nextCursor: next.nextCursor,
              sessions: [...new Map([...previous.sessions, ...next.sessions].map((s) => [s.id, s])).values()],
            }
          : next,
      );
    } catch {
      if (clientRef.current === client) setError('Không tải được trang tiếp theo.');
    } finally {
      setMoreBusy(false);
    }
  }
  async function moreEvents() {
    if (!client || !selected || !detail?.nextCursor) return;
    setLoading(true);
    try {
      const next = await client.detail(selected.id, detail.nextCursor);
      if (clientRef.current !== client || selectionRef.current !== selected.id) return;
      setDetail((previous) =>
        previous
          ? {
              ...next,
              events: [...new Map([...previous.events, ...next.events].map((e) => [e.eventId, e])).values()],
            }
          : next,
      );
    } catch {
      if (clientRef.current === client && selectionRef.current === selected.id)
        setDetailError('Không tải được trang sự kiện tiếp theo.');
    } finally {
      if (clientRef.current === client && selectionRef.current === selected.id) setLoading(false);
    }
  }
  async function exportCsv() {
    if (!client) return;
    setExportBusy(true);
    setNotice('');
    try {
      const result = await client.exportHistory(days);
      if (clientRef.current !== client) return;
      const url = URL.createObjectURL(new Blob([result.csv], { type: 'text/csv;charset=utf-8' }));
      const link = document.createElement('a');
      link.href = url;
      link.download = `${demo ? 'demo-' : ''}usage-monitor-${days}days.csv`;
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setNotice(
        `Đã chuẩn bị ${result.rows} dòng${result.truncated ? ' · Đã chạm giới hạn 10.000 dòng' : ''}.`,
      );
    } catch {
      if (clientRef.current === client) setError('Không xuất được lịch sử.');
    } finally {
      setExportBusy(false);
    }
  }

  const visibleSnapshot =
    snapshot && page === 'history'
      ? {
          ...snapshot,
          sessions: snapshot.sessions.filter(
            (s) => s.updatedAt && Date.parse(s.updatedAt) >= Date.now() - days * 86_400_000,
          ),
        }
      : snapshot;
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <a
          className="brand"
          href="#"
          onClick={(e) => {
            e.preventDefault();
            setPage('sessions');
          }}
        >
          <span className="brand-mark">
            <i />
            <i />
            <i />
          </span>
          <span>
            usage<span className="brand-light">monitor</span>
          </span>
        </a>
        <div className="sidebar-caption">AGENT WORKSPACE</div>
        <nav aria-label="Điều hướng">
          {nav.map(([id, icon, label]) => (
            <button
              key={id}
              className={page === id ? 'nav-item active' : 'nav-item'}
              aria-current={page === id ? 'page' : undefined}
              onClick={() => setPage(id)}
            >
              <span aria-hidden>{icon}</span>
              {label}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <span className="local-badge">
            <span /> Local first
          </span>
          <p>
            Metadata ở máy bạn.
            <br />
            Codex VS Code & Desktop.
          </p>
          {demo ? (
            <button
              className="text-button"
              onClick={() => switchClient(nativeAvailable ? nativeClient : null)}
            >
              Thoát demo
            </button>
          ) : (
            <button className="text-button" onClick={() => switchClient(createDemoClient())}>
              Xem dữ liệu demo
            </button>
          )}
          <small>v0.1 · Observer</small>
        </div>
      </aside>
      <main className="workspace">
        <div className="workspace-top">
          <span>
            Không gian cá nhân <span className="separator">/</span> {nav.find(([id]) => id === page)?.[2]}
          </span>
          <span className="privacy-note">◈ Metadata only</span>
        </div>
        {demo && (
          <div className="demo-banner" role="status">
            <strong>Dữ liệu giả lập</strong>
            <span>Các thao tác trong demo không thay đổi dữ liệu desktop.</span>
            <button onClick={() => switchClient(nativeAvailable ? nativeClient : null)}>Thoát demo</button>
          </div>
        )}
        {error && (
          <div className="alert alert--error global-alert" role="alert">
            {error}
            <button className="text-button" onClick={() => setRefresh((n) => n + 1)}>
              Thử lại
            </button>
          </div>
        )}
        {!client ? (
          <section className="welcome">
            <div className="welcome-symbol">◫</div>
            <p className="eyebrow">YOUR AGENTS, ONE VIEW</p>
            <h1>
              Một nơi để xem
              <br />
              agent của bạn.
            </h1>
            <p>
              Phiên làm việc, bằng chứng model và usage —<br />
              được theo dõi tại máy bạn.
            </p>
            <div className="welcome-actions">
              <button className="btn btn--primary" onClick={() => switchClient(createDemoClient())}>
                Khám phá demo
              </button>
              <span>Mở bản desktop để dùng dữ liệu thật.</span>
            </div>
            <div className="welcome-surfaces">
              <span>Codex · VS Code</span>
              <span>Codex · Desktop</span>
              <span>Advisory policy</span>
            </div>
          </section>
        ) : page === 'policies' ? (
          <PolicyEditor client={client} />
        ) : page === 'setup' ? (
          <SetupWizard client={client} />
        ) : (
          <>
            {page === 'history' && (
              <div className="history-toolbar">
                <div>
                  <p className="eyebrow">LOCAL HISTORY</p>
                  <h2>Lịch sử phiên</h2>
                </div>
                <div>
                  <label>
                    Khoảng thời gian
                    <select value={days} onChange={(e) => setDays(Number(e.target.value) as 7 | 30)}>
                      <option value={7}>7 ngày</option>
                      <option value={30}>30 ngày</option>
                    </select>
                  </label>
                  <button
                    className="btn btn--secondary"
                    disabled={exportBusy}
                    onClick={() => void exportCsv()}
                  >
                    Xuất CSV
                  </button>
                </div>
              </div>
            )}
            {notice && (
              <p className="success-note" role="status">
                {notice}
              </p>
            )}
            {visibleSnapshot ? (
              <div className={`sessions-layout ${selected ? 'has-detail' : ''}`}>
                <div>
                  <Dashboard
                    snapshot={visibleSnapshot}
                    selectedId={selected?.id ?? null}
                    onSelect={selectSession}
                    onRefresh={() => setRefresh((n) => n + 1)}
                    onSetup={() => setPage('setup')}
                  />
                  {snapshot?.nextCursor && (
                    <button
                      className="btn btn--secondary more-sessions"
                      disabled={moreBusy}
                      onClick={() => void moreSessions()}
                    >
                      {moreBusy ? 'Đang tải…' : 'Tải thêm phiên'}
                    </button>
                  )}
                </div>
                {selected && (
                  <aside className="detail-panel">
                    <div className="detail-topline">
                      <span>CHI TIẾT PHIÊN</span>
                      <button aria-label="Đóng chi tiết phiên" onClick={() => selectSession(null)}>
                        ×
                      </button>
                    </div>
                    <SessionTimeline
                      client={client}
                      onOverrideChanged={() => setRefresh((value) => value + 1)}
                      session={detail?.session ?? selected}
                      detail={detail}
                      loading={loading}
                      error={detailError || null}
                      onPhaseChange={(phase) => void changePhase(phase)}
                      onLoadMore={() => void moreEvents()}
                    />
                  </aside>
                )}
              </div>
            ) : (
              <div className="loading-state" role="status">
                Đang đọc dữ liệu local…
              </div>
            )}
          </>
        )}
      </main>
    </div>
  );
}
