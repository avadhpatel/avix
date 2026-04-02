import React, { useEffect, useState } from 'react';
import { invoke, listen } from '../platform';

interface Tool {
  name: string;
  description?: string;
  namespace?: string;
}

interface ToolListResponse {
  total: number;
  available: number;
  unavailable: number;
  tools: Tool[];
}

const loadTools = () =>
  invoke<ToolListResponse>('get_tools')
    .then((data) => data)
    .catch(() => ({ total: 0, available: 0, unavailable: 0, tools: [] } as ToolListResponse));

const ToolsPage: React.FC = () => {
  const [response, setResponse] = useState<ToolListResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState('');

  useEffect(() => {
    loadTools()
      .then((data) => setResponse(data))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    let active = true;
    const unlisteners: Array<() => void> = [];

    listen('tool.changed', () => {
      if (!active) return;
      loadTools().then((data) => setResponse(data));
    }).then((f) => unlisteners.push(f));

    return () => {
      active = false;
      unlisteners.forEach((f) => f());
    };
  }, []);

  const tools = response?.tools ?? [];
  const total = response?.total ?? 0;
  const available = response?.available ?? 0;
  const unavailable = response?.unavailable ?? 0;

  const filtered = tools.filter(
    (t) =>
      t.name.toLowerCase().includes(search.toLowerCase()) ||
      (t.description ?? '').toLowerCase().includes(search.toLowerCase())
  );

  // Group by namespace
  const grouped = filtered.reduce<Record<string, Tool[]>>((acc, tool) => {
    const ns = tool.namespace ?? tool.name.split('/')[0] ?? 'other';
    if (!acc[ns]) acc[ns] = [];
    acc[ns].push(tool);
    return acc;
  }, {});

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
          Tools
        </h2>
        <p style={{ color: '#64748b', fontSize: '13px', marginBottom: '20px' }}>
          Tools accessible to agents in the Avix tool registry
        </p>

        {/* Search */}
        <div style={{ marginBottom: '20px' }}>
          <input
            type="text"
            placeholder="Search tools…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            style={{
              width: '100%',
              padding: '8px 12px',
              backgroundColor: '#0d1829',
              border: '1px solid #1e293b',
              borderRadius: '8px',
              color: '#f8fafc',
              fontSize: '13px',
              outline: 'none',
              boxSizing: 'border-box',
            }}
            onFocus={(e) => { e.currentTarget.style.borderColor = '#3b82f6'; }}
            onBlur={(e) => { e.currentTarget.style.borderColor = '#1e293b'; }}
          />
        </div>

        {loading && (
          <div style={{ color: '#334155', fontSize: '13px' }}>Loading tools…</div>
        )}

        {!loading && total === 0 && (
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
              <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
            </svg>
            <p style={{ color: '#334155', fontSize: '13px', margin: 0 }}>
              No tools registered. Tools are registered by services via ipc.register.
            </p>
          </div>
        )}

        {!loading && total > 0 && (
          <div style={{ marginBottom: '16px', display: 'flex', gap: '16px' }}>
            <div style={{ backgroundColor: '#0d1829', padding: '8px 16px', borderRadius: '8px', border: '1px solid #1e293b' }}>
              <span style={{ color: '#64748b', fontSize: '12px' }}>Total: </span>
              <span style={{ color: '#e2e8f0', fontSize: '14px', fontWeight: 600 }}>{total}</span>
            </div>
            <div style={{ backgroundColor: '#0d1829', padding: '8px 16px', borderRadius: '8px', border: '1px solid #1e293b' }}>
              <span style={{ color: '#64748b', fontSize: '12px' }}>Available: </span>
              <span style={{ color: '#22c55e', fontSize: '14px', fontWeight: 600 }}>{available}</span>
            </div>
            <div style={{ backgroundColor: '#0d1829', padding: '8px 16px', borderRadius: '8px', border: '1px solid #1e293b' }}>
              <span style={{ color: '#64748b', fontSize: '12px' }}>Unavailable: </span>
              <span style={{ color: '#f59e0b', fontSize: '14px', fontWeight: 600 }}>{unavailable}</span>
            </div>
          </div>
        )}

        {!loading && total > 0 && filtered.length === 0 && (
          <p style={{ color: '#334155', fontSize: '13px' }}>No tools match your search.</p>
        )}

        {Object.entries(grouped).map(([ns, nsTools]) => (
          <div key={ns} style={{ marginBottom: '20px' }}>
            <div
              style={{
                fontSize: '10px',
                fontWeight: 700,
                color: '#475569',
                letterSpacing: '0.08em',
                textTransform: 'uppercase',
                marginBottom: '8px',
                padding: '0 4px',
              }}
            >
              {ns}
            </div>
            <div
              style={{
                backgroundColor: '#0d1829',
                border: '1px solid #1e293b',
                borderRadius: '10px',
                overflow: 'hidden',
              }}
            >
              {nsTools.map((tool, i) => (
                <div
                  key={tool.name}
                  style={{
                    padding: '10px 16px',
                    borderBottom: i < nsTools.length - 1 ? '1px solid #0f172a' : 'none',
                    display: 'flex',
                    alignItems: 'center',
                    gap: '12px',
                  }}
                >
                  <code
                    style={{
                      fontSize: '12px',
                      color: '#7dd3fc',
                      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
                      minWidth: '160px',
                      flexShrink: 0,
                    }}
                  >
                    {tool.name}
                  </code>
                  <span style={{ fontSize: '12px', color: '#475569' }}>
                    {tool.description ?? '—'}
                  </span>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
};

export default ToolsPage;
