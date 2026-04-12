import React, { useEffect, useState } from 'react';
import { invoke } from '../platform';
import { InstalledAgent } from '../types/agents';
import { NewSessionModal } from '../components/NewSessionModal';

const ScopeBadge: React.FC<{ scope: string }> = ({ scope }) => (
  <span
    style={{
      display: 'inline-block',
      padding: '2px 7px',
      borderRadius: '4px',
      fontSize: '10px',
      fontWeight: 700,
      letterSpacing: '0.05em',
      textTransform: 'uppercase',
      backgroundColor: scope === 'system' ? '#1e3a5f' : '#1e293b',
      color: scope === 'system' ? '#60a5fa' : '#94a3b8',
    }}
  >
    {scope === 'system' ? 'SYS' : 'USR'}
  </span>
);

const CatalogPage: React.FC = () => {
  const [agents, setAgents] = useState<InstalledAgent[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState('');
  const [spawnTarget, setSpawnTarget] = useState<InstalledAgent | null>(null);

  useEffect(() => {
    invoke<string>('list_installed', {})
      .then((json) => {
        try {
          const raw = JSON.parse(json);
          setAgents(Array.isArray(raw) ? raw : []);
        } catch {
          setAgents([]);
        }
      })
      .catch(() => setAgents([]))
      .finally(() => setLoading(false));
  }, []);

  const filtered = agents.filter(
    (a) =>
      a.name.toLowerCase().includes(search.toLowerCase()) ||
      (a.description ?? '').toLowerCase().includes(search.toLowerCase())
  );

  return (
    <div style={{ height: '100%', overflow: 'auto', padding: '24px' }}>
      <div style={{ maxWidth: '800px' }}>
        <h2 style={{ color: '#f8fafc', fontSize: '18px', fontWeight: 700, margin: '0 0 4px' }}>
          Agent Catalog
        </h2>
        <p style={{ color: '#64748b', fontSize: '13px', marginBottom: '20px' }}>
          Installed agents available to spawn
        </p>

        {/* Search */}
        <div style={{ position: 'relative', marginBottom: '20px' }}>
          <svg
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="#475569"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            style={{ position: 'absolute', left: '10px', top: '50%', transform: 'translateY(-50%)' }}
          >
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            type="text"
            placeholder="Search agents…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            style={{
              width: '100%',
              padding: '8px 12px 8px 32px',
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
          <div style={{ color: '#334155', fontSize: '13px' }}>Loading catalog…</div>
        )}

        {!loading && filtered.length === 0 && (
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
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
              <line x1="3" y1="9" x2="21" y2="9" />
              <line x1="9" y1="21" x2="9" y2="9" />
            </svg>
            <p style={{ color: '#334155', fontSize: '13px', margin: 0 }}>
              {search ? 'No agents match your search.' : 'No agents installed.'}
            </p>
          </div>
        )}

        {filtered.length > 0 && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
            {filtered.map((agent) => (
              <div
                key={agent.path}
                style={{
                  backgroundColor: '#0d1829',
                  border: '1px solid #1e293b',
                  borderRadius: '10px',
                  padding: '14px 16px',
                  display: 'flex',
                  alignItems: 'center',
                  gap: '12px',
                  transition: 'border-color 0.12s',
                }}
                onMouseEnter={(e) => {
                  (e.currentTarget as HTMLDivElement).style.borderColor = '#334155';
                }}
                onMouseLeave={(e) => {
                  (e.currentTarget as HTMLDivElement).style.borderColor = '#1e293b';
                }}
              >
                {/* Icon */}
                <div
                  style={{
                    width: '36px',
                    height: '36px',
                    borderRadius: '8px',
                    background: 'linear-gradient(135deg, #1d4ed8 0%, #7c3aed 100%)',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    flexShrink: 0,
                  }}
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="white">
                    <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" fill="none" />
                  </svg>
                </div>

                {/* Info */}
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '3px' }}>
                    <span style={{ color: '#f1f5f9', fontWeight: 600, fontSize: '14px' }}>
                      {agent.name}
                    </span>
                    <span style={{ color: '#475569', fontSize: '11px' }}>v{agent.version}</span>
                    <ScopeBadge scope={agent.scope} />
                  </div>
                  <div style={{ color: '#64748b', fontSize: '12px', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    {agent.description || 'No description'}
                  </div>
                </div>

                {/* Spawn button — opens NewSessionModal with agent pre-selected (step 2) */}
                <button
                  onClick={() => setSpawnTarget(agent)}
                  style={{
                    padding: '6px 14px',
                    backgroundColor: '#1d4ed8',
                    color: '#fff',
                    border: 'none',
                    borderRadius: '6px',
                    fontSize: '12px',
                    fontWeight: 600,
                    cursor: 'pointer',
                    flexShrink: 0,
                    transition: 'background 0.12s',
                  }}
                  onMouseEnter={(e) => {
                    (e.currentTarget as HTMLButtonElement).style.background = '#2563eb';
                  }}
                  onMouseLeave={(e) => {
                    (e.currentTarget as HTMLButtonElement).style.background = '#1d4ed8';
                  }}
                >
                  Spawn
                </button>
              </div>
            ))}
          </div>
        )}
      </div>

      <NewSessionModal
        isOpen={spawnTarget !== null}
        onClose={() => setSpawnTarget(null)}
        defaultAgent={spawnTarget ?? undefined}
      />
    </div>
  );
};

export default CatalogPage;
