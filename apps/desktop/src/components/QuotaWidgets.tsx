import type { AccountQuota } from '../types';
import { quotaFreshness, quotaNumber } from '../quota';

const labels = { fresh: 'Còn mới', stale: 'Dữ liệu cũ', expired: 'Đã hết cửa sổ', unknown: 'Chưa xác minh' };
const date = (value: string | null) => (value ? new Date(value).toLocaleString('vi-VN') : 'Chưa có dữ liệu');

export function QuotaWidgets({ quotas, clockTrusted }: { quotas: AccountQuota[]; clockTrusted?: boolean }) {
  return (
    <section className="quota-widgets" aria-label="Quota tài khoản">
      <h3>Quota tài khoản</h3>
      <p className="quota-note">
        Các phiên cùng tài khoản tham chiếu chung hạn mức. Mỗi cửa sổ và đơn vị được giữ riêng.
      </p>
      {quotas.length === 0 ? (
        <p>Chưa có nguồn quota khả dụng cho tài khoản này.</p>
      ) : (
        <div className="quota-grid">
          {quotas.map((q) => {
            const s = q.snapshot;
            const freshness = quotaFreshness(q, Date.now(), clockTrusted);
            return (
              <article className="quota-card" key={s.quotaKey}>
                <header>
                  <strong>
                    {s.provider} · {s.bucket}
                  </strong>
                  <span className={`badge badge--${freshness}`}>{labels[freshness]}</span>
                </header>
                <p>
                  {s.scope} · {s.windowKey} · {s.unit}
                </p>
                <dl>
                  <dt>Đã dùng</dt>
                  <dd>{quotaNumber(s.used)}</dd>
                  <dt>Hạn mức</dt>
                  <dd>{quotaNumber(s.limit)}</dd>
                  <dt>Còn lại</dt>
                  <dd>{quotaNumber(s.remaining)}</dd>
                  <dt>Tỷ lệ đã dùng</dt>
                  <dd>{s.usedPercent === null ? 'Chưa có dữ liệu' : `${quotaNumber(s.usedPercent)}%`}</dd>
                  <dt>Quan sát lúc</dt>
                  <dd>{date(s.observedAt)}</dd>
                  <dt>Đặt lại lúc</dt>
                  <dd>{date(s.resetsAt)}</dd>
                </dl>
                <details>
                  <summary>Nguồn và thời gian</summary>
                  <p>
                    {q.sourceIntegrationId} · {q.source} · {q.confidence}
                  </p>
                  <p>
                    Bắt đầu: {date(s.windowStart)} · Kết thúc: {date(s.windowEnd)}
                  </p>
                  <p>Dữ liệu cũ từ: {date(s.staleAfter)}</p>
                </details>
              </article>
            );
          })}
        </div>
      )}
      {quotas.length === 200 && <p>Đang hiển thị tối đa 200 cửa sổ quan sát gần nhất.</p>}
    </section>
  );
}
