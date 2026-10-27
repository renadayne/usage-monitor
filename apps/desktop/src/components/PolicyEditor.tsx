import { useEffect, useState } from 'react';
import type { Client } from '../client';
import type { Phase, Policy, Provider } from '../types';

export function PolicyEditor({ client }: { client: Client }) {
  const [policies, setPolicies] = useState<Policy[]>([]);
  const [phase, setPhase] = useState<Phase>('REVIEW');
  const [models, setModels] = useState('');
  const [provider, setProvider] = useState<Provider | ''>('codex');
  const [priority, setPriority] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  useEffect(() => {
    let active = true;
    client
      .policies()
      .then((p) => {
        if (active) setPolicies(p);
      })
      .catch(() => {
        if (active) setError('Không đọc được policy đã lưu.');
      });
    return () => {
      active = false;
    };
  }, [client]);

  async function save(policy: Policy) {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      await client.savePolicy(policy);
      setPolicies(await client.policies());
      setNotice('Đã lưu policy.');
    } catch {
      setError('Không lưu được. Kiểm tra model ID và phạm vi policy.');
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="page policy-page">
      <header className="page-heading">
        <div>
          <p className="eyebrow">MODEL POLICY</p>
          <h1>Chính sách theo phase</h1>
          <p>So sánh model có bằng chứng với quy tắc bạn chọn.</p>
        </div>
        <span className="badge">Advisory</span>
      </header>
      <div className="notice">
        Codex hook chỉ cung cấp model được chọn. Bằng chứng này chưa xác nhận model đã trả lời.
      </div>
      <form
        className="panel policy-form"
        onSubmit={(e) => {
          e.preventDefault();
          const allowedModelIds = [
            ...new Set(
              models
                .split(',')
                .map((s) => s.trim())
                .filter(Boolean),
            ),
          ];
          if (!allowedModelIds.length) return;
          void save({
            id: crypto.randomUUID(),
            scope: { provider: provider || null, projectKey: null, sessionId: null },
            phase,
            allowedModelIds,
            priority,
            action: 'observe',
            enabled: true,
          });
        }}
      >
        <h2>Thêm quy tắc</h2>
        <div className="form-grid">
          <label>
            Phase
            <select value={phase} onChange={(e) => setPhase(e.target.value as Phase)}>
              <option value="PLANNING">Planning</option>
              <option value="IMPLEMENTATION">Implementation</option>
              <option value="REVIEW">Review</option>
              <option value="DONE">Done</option>
            </select>
          </label>
          <label>
            Phạm vi
            <select value={provider} onChange={(e) => setProvider(e.target.value as Provider | '')}>
              <option value="">Tất cả provider</option>
              <option value="codex">Codex</option>
              <option value="copilot">Copilot</option>
              <option value="claude_code">Claude Code</option>
              <option value="antigravity">Antigravity</option>
            </select>
          </label>
          <label>
            Ưu tiên
            <input
              type="number"
              min={-2147483648}
              max={2147483647}
              step={1}
              value={priority}
              onChange={(e) => setPriority(Number(e.target.value))}
            />
          </label>
        </div>
        <label>
          Model ID được phép
          <input
            required
            maxLength={4096}
            value={models}
            onChange={(e) => setModels(e.target.value)}
            placeholder="model-id-1, model-id-2"
          />
          <small>Dùng ID chính xác, phân cách bằng dấu phẩy.</small>
        </label>
        <div className="form-footer">
          <span>Quy tắc cùng hạng chỉ cho phép các model chung.</span>
          <button className="btn btn--primary" disabled={busy} type="submit">
            Lưu quy tắc
          </button>
        </div>
      </form>
      {error && (
        <p role="alert" className="alert alert--error">
          {error}
        </p>
      )}
      {notice && (
        <p role="status" className="success-note">
          {notice}
        </p>
      )}
      <div className="section-heading">
        <h2>Quy tắc đã lưu</h2>
        <span>{policies.length}</span>
      </div>
      {!policies.length ? (
        <div className="empty-small">Chưa có quy tắc. Các phiên vẫn được quan sát bình thường.</div>
      ) : (
        <div className="policy-list">
          {policies.map((p) => (
            <article className="panel rule" key={p.id}>
              <div>
                <span className="eyebrow">
                  {p.scope.provider || 'GLOBAL'} · {p.phase}
                </span>
                <h3>{p.allowedModelIds.join(' / ')}</h3>
                <p>
                  Ưu tiên {p.priority} · {p.action === 'notify' ? 'Có cảnh báo' : 'Quan sát'}
                </p>
              </div>
              <button
                className="btn btn--secondary"
                disabled={busy}
                onClick={() => void save({ ...p, enabled: !p.enabled })}
              >
                {p.enabled ? 'Tắt quy tắc' : 'Bật quy tắc'}
              </button>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
