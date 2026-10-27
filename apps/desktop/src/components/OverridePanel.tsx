import { useState } from 'react';
import type { Client } from '../client';
import type { Detail } from '../types';

export function OverridePanel({
  client,
  detail,
  onChanged,
}: {
  client: Client;
  detail: Detail;
  onChanged: () => void;
}) {
  const [policy, setPolicy] = useState('');
  const [models, setModels] = useState('');
  const [reason, setReason] = useState('');
  const [duration, setDuration] = useState(900);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const winning = detail.policy.winningPolicyIds;
  const selected = winning.includes(policy) ? policy : (winning[0] ?? '');
  async function action(run: () => Promise<void>) {
    setBusy(true);
    setError('');
    try {
      await run();
      onChanged();
    } catch {
      setError('Không lưu được ngoại lệ. Hãy làm mới phase và kiểm tra policy, model ID, lý do.');
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="override-panel" aria-label="Ngoại lệ policy">
      <h3>Ngoại lệ có thời hạn</h3>
      <p className="quota-note">
        Ngoại lệ chỉ thay danh sách model cho một quy tắc ở phiên và epoch hiện tại. Cần bằng chứng model thực
        thi để kết luận tuân thủ.
      </p>
      {error && (
        <p className="alert alert--error" role="alert">
          {error}
        </p>
      )}
      {!client.native ? (
        <p>Mở bản desktop để tạo và thu hồi ngoại lệ.</p>
      ) : winning.length === 0 ? (
        <p>Chưa có quy tắc áp dụng cho phase hiện tại.</p>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const replacementModelIds = [
              ...new Set(
                models
                  .split(',')
                  .map((s) => s.trim())
                  .filter(Boolean),
              ),
            ];
            if (!replacementModelIds.length || !reason.trim()) return;
            void action(() =>
              client.createOverride({
                id: crypto.randomUUID(),
                policyId: selected,
                sessionId: detail.session.id,
                expectedPhaseEpoch: detail.session.phaseEpoch,
                replacementModelIds,
                reason,
                durationSeconds: duration,
              }),
            );
          }}
        >
          <fieldset disabled={busy || !detail.clockTrusted}>
            <label>
              Quy tắc
              <select value={selected} onChange={(e) => setPolicy(e.target.value)}>
                {winning.map((id) => (
                  <option key={id} value={id}>
                    {id}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Model ID thay thế
              <input
                required
                maxLength={8192}
                value={models}
                onChange={(e) => setModels(e.target.value)}
                placeholder="model-id-1, model-id-2"
              />
            </label>
            <label>
              Lý do
              <input required maxLength={512} value={reason} onChange={(e) => setReason(e.target.value)} />
            </label>
            <label>
              Thời hạn
              <select value={duration} onChange={(e) => setDuration(Number(e.target.value))}>
                <option value={900}>15 phút</option>
                <option value={3600}>1 giờ</option>
                <option value={86400}>24 giờ</option>
              </select>
            </label>
            <button className="btn btn--secondary" type="submit">
              {busy ? 'Đang lưu…' : 'Tạo ngoại lệ cho epoch này'}
            </button>
          </fieldset>
          {!detail.clockTrusted && <p>Chờ xác minh đồng hồ hệ thống để tạo ngoại lệ.</p>}
        </form>
      )}
      <details>
        <summary>Lịch sử ngoại lệ ({detail.overrides.length})</summary>
        {detail.overrides.length === 0 ? (
          <p>Chưa có ngoại lệ.</p>
        ) : (
          <ul className="override-list">
            {detail.overrides.map((o) => {
              const active =
                detail.clockTrusted &&
                !o.revokedAt &&
                o.phaseEpoch === detail.session.phaseEpoch &&
                winning.includes(o.policyId) &&
                Date.parse(o.createdAt) <= Date.now() &&
                Date.now() < Date.parse(o.expiresAt);
              return (
                <li key={o.id}>
                  <strong>
                    {o.policyId} · E{o.phaseEpoch}
                  </strong>
                  <span className="badge">
                    {o.revokedAt ? 'Đã thu hồi' : active ? 'Đang áp dụng' : 'Không áp dụng'}
                  </span>
                  <p>
                    {o.replacementModelIds.join(', ')} · {o.reason}
                  </p>
                  <p>
                    Tạo: {new Date(o.createdAt).toLocaleString('vi-VN')} · Hết hạn:{' '}
                    {new Date(o.expiresAt).toLocaleString('vi-VN')}
                  </p>
                  {client.native && !o.revokedAt && (
                    <button
                      className="text-button"
                      disabled={busy}
                      onClick={() => void action(() => client.revokeOverride(o.id, detail.session.id))}
                    >
                      Thu hồi
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
        {detail.overrides.length === 200 && <p>Hiển thị tối đa 200 ngoại lệ gần nhất.</p>}
      </details>
    </section>
  );
}
