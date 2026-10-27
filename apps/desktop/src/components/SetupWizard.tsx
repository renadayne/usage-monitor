import { useState } from 'react';
import type { Client } from '../client';
import type { SetupPreview } from '../types';

export function SetupWizard({ client }: { client: Client }) {
  const [config, setConfig] = useState('{}');
  const [command, setCommand] = useState('');
  const [preview, setPreview] = useState<SetupPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return (
    <section className="page setup-page">
      <header className="page-heading">
        <div>
          <p className="eyebrow">CODEX OBSERVER</p>
          <h1>Thiết lập kết nối</h1>
          <p>Xem trước cấu hình hook trước khi tự áp dụng trong Codex.</p>
        </div>
        <span className="badge">Chưa xác minh</span>
      </header>
      <div className="surface-grid">
        {['VS Code Extension', 'Desktop App'].map((surface) => (
          <article className="panel surface-card" key={surface}>
            <div className="provider-symbol">C</div>
            <div>
              <h3>Codex · {surface}</h3>
              <p>Xác minh riêng trên từng ứng dụng.</p>
            </div>
            <span className="status-dot" title="Chưa xác minh" />
          </article>
        ))}
      </div>
      <div className="notice">
        Bản đề xuất chỉ nằm trong bộ nhớ. Bạn cần review và trust hook trong Codex sau khi tự cập nhật file.
      </div>
      <form
        className="panel setup-form"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          setError('');
          setPreview(null);
          try {
            setPreview(await client.previewSetup(config, command));
          } catch {
            setError(
              client.native
                ? 'Không tạo được đề xuất. Kiểm tra JSON và lệnh helper.'
                : 'Chức năng này cần ứng dụng desktop.',
            );
          } finally {
            setBusy(false);
          }
        }}
      >
        <h2>Preview hooks.json</h2>
        <p>
          Dán nội dung hooks.json đã loại bí mật. Token của bộ thu nằm trong file cấu hình riêng của helper.
        </p>
        <label>
          Lệnh helper đã review
          <input
            required
            value={command}
            maxLength={4096}
            onChange={(e) => {
              setCommand(e.target.value);
              setPreview(null);
            }}
            placeholder="aum-observer --config đường-dẫn-file-của-ứng-dụng.json"
            spellCheck={false}
          />
        </label>
        <label>
          Nội dung hooks.json hiện tại
          <textarea
            value={config}
            onChange={(e) => {
              setConfig(e.target.value);
              setPreview(null);
            }}
            maxLength={1048576}
            rows={9}
            spellCheck={false}
          />
        </label>
        <div className="form-footer">
          <span>Không có thao tác ghi cấu hình provider.</span>
          <button type="submit" disabled={busy || !client.native} className="btn btn--primary">
            {busy ? 'Đang tạo…' : 'Tạo bản đề xuất'}
          </button>
        </div>
        {!client.native && <small>Mở ứng dụng desktop để dùng planner.</small>}
      </form>
      {error && (
        <p role="alert" className="alert alert--error">
          {error}
        </p>
      )}
      {preview && (
        <section className="panel preview">
          <div className="section-heading">
            <h2>Bản đề xuất</h2>
            <span className="badge">Cần review & trust</span>
          </div>
          <p>
            Thêm: {preview.addedEvents.join(', ') || 'Không có'} · Giữ lệnh có sẵn:{' '}
            {preview.skippedEvents.join(', ') || 'Không có'}
          </p>
          <pre>{preview.mergedConfig}</pre>
          <p>
            Review lệnh và toàn bộ thay đổi trước khi cập nhật hooks.json. Hai surface vẫn giữ trạng thái chưa
            xác minh cho tới khi kiểm tra thực tế.
          </p>
        </section>
      )}
      <section className="panel availability">
        <h2>Nguồn dữ liệu hiện có</h2>
        <dl>
          <div>
            <dt>Model được chọn</dt>
            <dd>Codex hook metadata</dd>
          </div>
          <div>
            <dt>Model đã trả lời</dt>
            <dd>Chưa có bằng chứng trên máy đích</dd>
          </div>
          <div>
            <dt>Usage / quota</dt>
            <dd>Chưa có nguồn Codex được xác minh</dd>
          </div>
          <div>
            <dt>Thực thi policy</dt>
            <dd>Advisory</dd>
          </div>
        </dl>
      </section>
    </section>
  );
}
