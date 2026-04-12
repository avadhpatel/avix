import React, { useEffect, useState } from 'react';
import { invoke } from '../platform';
import { InvocationRecord } from '../types/agents';

const statusColor: Record<string, string> = {
  running: '#22c55e',
  completed: '#3b82f6',
  failed: '#ef4444',
  killed: '#f59e0b',
};

const statusBg: Record<string, string> = {
  running: '#052e16',
  completed: '#1e3a5f',
  failed: '#2d0a0a',
  killed: '#2d1d00',
};

function shortId(id: string) {
  return id.length > 8 ? id.slice(0, 8) : id;
}

function formatDate(iso?: string) {
  if (!iso) return '—';
  try {
    return new Date(iso).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

// ── Detail drawer ─────────────────────────────────────────────────────────────

interface InvocationDetail {
  id: string;
  agentName: string;
  status: string;
  goal: string;
  spawnedAt: string;
  endedAt?: string;
  tokensConsumed: number;
  toolCallsTotal: number;
  exitReason?: string;
  conversation?: Array<{ role: string; content: string }>;
}

const DetailDrawer: React.FC<{ id: string; onClose: () => void }> = ({ id, onClose }) => {
  const [detail, setDetail] = useState<InvocationDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');

  useEffect(() => {
    invoke<string | null>('get_invocation', { invocationId: id })
      .then((json) => {
        if (!json) {
          setError('Invocation not found.');
          return;
        }
        try {
          setDetail(JSON.parse(json));
        } catch {
          setError('Failed to parse invocation data.');
        }
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [id]);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        backgroundColor: 'rgba(0,0,0,0.6)',
        display: 'flex',
        justifyContent: 'flex-end',
        zIndex: 100,
      }}
      onClick={onClose}
    >
      <div
        style={{
          width: '480px',
          height: '100%',
          backgroundColor: '#0f172a',
          borderLeft: '1px solid #1e293b',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div
          style={{
            padding: '16px 20px',
            borderBottom: '1px solid #1e293b',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
          }}
        >
          <div>
            <div style={{ color: '#f1f5f9', fontWeight: 700, fontSize: '14px' }}>
              Invocation Detail
            </div>
            <div style={{ color: '#475569', fontSize: '11px', fontFamily: 'monospace', marginTop: '2px' }}>
              {id}
            </div>
          </div>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              color: '#475569',
              cursor: 'pointer',
              padding: '4px',
              display: 'flex',
            }}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        </div>

        {/* Body */}
        <div style={{ flex: 1, overflow: 'auto', padding: '20px' }}>
          {loading && <div style={{ color: '#334155', fontSize: '13px' }}>Loading…</div>}
          {error && <div style={{ color: '#ef4444', fontSize: '13px' }}>{error}</div>}

          {detail && (
            <>
              {/* Meta grid */}
              <div
                style={{
                  display: 'grid',
                  gridTemplateColumns: '1fr 1fr',
                  gap: '12px',
                  marginBottom: '20px',
                }}
              >
                {[
                  { label: 'Agent', value: detail.agentName },
                  { label: 'Status', value: detail.status },
                  { label: 'Spawned', value: formatDate(detail.spawnedAt) },
                  { label: 'Ended', value: formatDate(detail.endedAt) },
                  { label: 'Tokens', value: detail.tokensConsumed.toLocaleString() },
                  { label: 'Tool calls', value: detail.toolCallsTotal.toString() },
                ].map(({ label, value }) => (
                  <div key={label}>
                    <div style={{ color: '#475569', fontSize: '10px', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: '3px' }}>
                      {label}
                    </div>
                    <div style={{ color: '#e2e8f0', fontSize: '13px' }}>{value}</div>
                  </div>
                ))}
              </div>

              {detail.goal && (
                <div style={{ marginBottom: '20px' }}>
                  <div style={{ color: '#475569', fontSize: '10px', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: '6px' }}>
                    Goal
                  </div>
                  <div style={{ color: '#cbd5e1', fontSize: '13px', lineHeight: 1.5, backgroundColor: '#0d1829', borderRadius: '6px', padding: '10px 12px', border: '1px solid #1e293b' }}>
                    {detail.goal}
                  </div>
                </div>
              )}

              {detail.exitReason && (
                <div style={{ marginBottom: '20px' }}>
                  <div style={{ color: '#475569', fontSize: '10px', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: '6px' }}>
                    Exit reason
                  </div>
                  <div style={{ color: '#f87171', fontSize: '12px', fontFamily: 'monospace' }}>
                    {detail.exitReason}
                  </div>
                </div>
              )}

              {/* Conversation */}
              {detail.conversation && detail.conversation.length > 0 && (
                <div>
                  <div style={{ color: '#475569', fontSize: '10px', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: '10px' }}>
                    Conversation ({detail.conversation.length} messages)
                  </div>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
                    {detail.conversation.map((msg, i) => (
                      <div
                        key={i}
                        style={{
                          backgroundColor: '#0d1829',
                          border: '1px solid #1e293b',
                          borderRadius: '8px',
                          padding: '10px 12px',
                        }}
                      >
                        <div
                          style={{
                            fontSize: '10px',
                            fontWeight: 700,
                            textTransform: 'uppercase',
                            letterSpacing: '0.05em',
                            marginBottom: '5px',
                            color: msg.role === 'user' ? '#60a5fa' : '#a78bfa',
                          }}
                        >
                          {msg.role}
                        </div>
                        <div style={{ color: '#cbd5e1', fontSize: '12px', lineHeight: 1.6, whiteSpace: 'pre-wrap' }}>
                          {msg.content}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {(!detail.conversation || detail.conversation.length === 0) && (
                <div style={{ color: '#334155', fontSize: '12px', fontStyle: 'italic' }}>
                  No conversation recorded for this invocation.
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
};

// ── Main page ─────────────────────────────────────────────────────────────────

const HistoryPage: React.FC = () => {
  const [records, setRecords] = useState<InvocationRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [agentFilter, setAgentFilter] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const load = (filter?: string) => {
    setLoading(true);
    invoke<string>('list_invocations', {
      agentName: filter || null,
    })
      .then((json) => {
        try {
          const raw = JSON.parse(json);
          setRecords(Array.isArray(raw) ? raw : []);
        } catch {
          setRecords([]);
        }
      })
      .catch(() => setRecords([]))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    load();
  }, []);

  const handleFilterChange = (v: string) => {
    setAgentFilter(v);
    load(v || undefined);
  };

  return (
    <div style={{ height: '100%', overflow: 'auto', padding: '24px' }}>
      <div style={{ maxWidth: '900px' }}>
        <h2 style={{ color: '#f8fafc', fontSize: '18px', fontWeight: 700, margin: '0 0 4px' }}>
          Invocation History
        </h2>
        <p style={{ color: '#64748b', fontSize: '13px', marginBottom: '20px' }}>
          Past agent runs — click a row to view details and conversation
        </p>

        {/* Filter */}
        <div style={{ position: 'relative', marginBottom: '20px', maxWidth: '280px' }}>
          <input
            type="text"
            placeholder="Filter by agent name…"
            value={agentFilter}
            onChange={(e) => handleFilterChange(e.target.value)}
            style={{
              width: '100%',
              padding: '7px 12px',
              backgroundColor: '#0d1829',
              border: '1px solid #1e293b',
              borderRadius: '8px',
              color: '#e2e8f0',
              fontSize: '13px',
              outline: 'none',
              boxSizing: 'border-box',
            }}
          />
        </div>

        {loading && (
          <div style={{ color: '#334155', fontSize: '13px' }}>Loading history…</div>
        )}

        {!loading && records.length === 0 && (
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
              <circle cx="12" cy="12" r="10" />
              <polyline points="12 6 12 12 16 14" />
            </svg>
            <p style={{ color: '#334155', fontSize: '13px', margin: 0 }}>
              {agentFilter ? `No invocations found for "${agentFilter}".` : 'No invocation history yet.'}
            </p>
          </div>
        )}

        {records.length > 0 && (
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
                  {['ID', 'Agent', 'Status', 'Spawned', 'Tokens', 'Goal'].map((h) => (
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
                {records.map((rec) => (
                  <tr
                    key={rec.id}
                    onClick={() => setSelectedId(rec.id)}
                    style={{ borderBottom: '1px solid #0f172a', cursor: 'pointer' }}
                    onMouseEnter={(e) => {
                      (e.currentTarget as HTMLTableRowElement).style.background = '#1e293b30';
                    }}
                    onMouseLeave={(e) => {
                      (e.currentTarget as HTMLTableRowElement).style.background = 'none';
                    }}
                  >
                    <td style={{ padding: '11px 16px', color: '#475569', fontSize: '12px', fontFamily: 'monospace' }}>
                      {shortId(rec.id)}…
                    </td>
                    <td style={{ padding: '11px 16px', color: '#e2e8f0', fontSize: '13px', fontWeight: 500 }}>
                      {rec.agentName}
                    </td>
                    <td style={{ padding: '11px 16px' }}>
                      <span
                        style={{
                          display: 'inline-block',
                          padding: '2px 8px',
                          borderRadius: '4px',
                          fontSize: '11px',
                          fontWeight: 600,
                          color: statusColor[rec.status] ?? '#94a3b8',
                          backgroundColor: statusBg[rec.status] ?? '#0d1829',
                        }}
                      >
                        {rec.status}
                      </span>
                    </td>
                    <td style={{ padding: '11px 16px', color: '#64748b', fontSize: '12px' }}>
                      {formatDate(rec.spawnedAt)}
                    </td>
                    <td style={{ padding: '11px 16px', color: '#64748b', fontSize: '12px', fontFamily: 'monospace' }}>
                      {rec.tokensConsumed.toLocaleString()}
                    </td>
                    <td
                      style={{
                        padding: '11px 16px',
                        color: '#64748b',
                        fontSize: '12px',
                        maxWidth: '200px',
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      }}
                    >
                      {rec.goal || '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {selectedId && (
        <DetailDrawer id={selectedId} onClose={() => setSelectedId(null)} />
      )}
    </div>
  );
};

export default HistoryPage;
