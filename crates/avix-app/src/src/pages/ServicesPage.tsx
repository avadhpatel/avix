import React, { useEffect, useState } from 'react';
import { invoke } from '../platform';

interface Service {
  name: string;
  status: string;
  pid?: number;
  description?: string;
}

const statusColor: Record<string, string> = {
  running: '#22c55e',
  stopped: '#64748b',
  failed: '#ef4444',
  starting: '#f59e0b',
};

const ServicesPage: React.FC = () => {
  const [services, setServices] = useState<Service[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    invoke<Service[]>('get_services')
      .then((data) => {
        setServices(Array.isArray(data) ? data : []);
      })
      .catch(() => {
        setError('');
        setServices([]);
      })
      .finally(() => setLoading(false));
  }, []);

  return (
    <div
      style={{
        height: '100%',
        overflow: 'auto',
        padding: '24px',
      }}
    >
      <div style={{ maxWidth: '800px' }}>
        <h2 style={{ color: '#f8fafc', fontSize: '18px', fontWeight: 700, margin: '0 0 4px' }}>
          Services
        </h2>
        <p style={{ color: '#64748b', fontSize: '13px', marginBottom: '24px' }}>
          System services running in the Avix runtime
        </p>

        {loading && (
          <div style={{ color: '#334155', fontSize: '13px' }}>Loading services…</div>
        )}

        {!loading && services.length === 0 && (
          <div
            style={{
              padding: '40px 24px',
              textAlign: 'center',
              backgroundColor: '#0d1829',
              borderRadius: '12px',
              border: '1px dashed #1e293b',
            }}
          >
            <svg
              width="36"
              height="36"
              viewBox="0 0 24 24"
              fill="none"
              stroke="#1e293b"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
              style={{ margin: '0 auto 12px', display: 'block' }}
            >
              <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
              <line x1="8" y1="21" x2="16" y2="21" />
              <line x1="12" y1="17" x2="12" y2="21" />
            </svg>
            <p style={{ color: '#334155', fontSize: '13px', margin: 0 }}>
              {error || 'No services found. The get_services command may not be implemented yet.'}
            </p>
          </div>
        )}

        {services.length > 0 && (
          <div
            style={{
              backgroundColor: '#0d1829',
              border: '1px solid #1e293b',
              borderRadius: '12px',
              overflow: 'hidden',
            }}
          >
            <table style={{ width: '100%', borderCollapse: 'collapse' }}>
              <thead>
                <tr style={{ borderBottom: '1px solid #1e293b' }}>
                  {['Name', 'Status', 'PID', 'Description'].map((h) => (
                    <th
                      key={h}
                      style={{
                        padding: '10px 16px',
                        textAlign: 'left',
                        fontSize: '11px',
                        fontWeight: 700,
                        color: '#475569',
                        letterSpacing: '0.06em',
                        textTransform: 'uppercase',
                      }}
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {services.map((svc) => (
                  <tr
                    key={svc.name}
                    style={{ borderBottom: '1px solid #0f172a' }}
                    onMouseEnter={(e) => { (e.currentTarget as HTMLTableRowElement).style.background = '#1e293b20'; }}
                    onMouseLeave={(e) => { (e.currentTarget as HTMLTableRowElement).style.background = 'none'; }}
                  >
                    <td style={{ padding: '12px 16px', color: '#e2e8f0', fontSize: '13px', fontWeight: 500 }}>
                      {svc.name}
                    </td>
                    <td style={{ padding: '12px 16px' }}>
                      <span
                        style={{
                          display: 'inline-flex',
                          alignItems: 'center',
                          gap: '5px',
                          fontSize: '11px',
                          fontWeight: 600,
                          color: statusColor[svc.status] ?? '#94a3b8',
                        }}
                      >
                        <span
                          style={{
                            width: '6px',
                            height: '6px',
                            borderRadius: '50%',
                            backgroundColor: statusColor[svc.status] ?? '#94a3b8',
                          }}
                        />
                        {svc.status}
                      </span>
                    </td>
                    <td style={{ padding: '12px 16px', color: '#475569', fontSize: '12px', fontFamily: 'monospace' }}>
                      {svc.pid ?? '—'}
                    </td>
                    <td style={{ padding: '12px 16px', color: '#475569', fontSize: '12px' }}>
                      {svc.description ?? '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
};

export default ServicesPage;
