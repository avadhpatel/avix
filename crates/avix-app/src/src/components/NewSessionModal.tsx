import React, { useState, useEffect } from 'react';
import { invoke } from '../platform';
import { InstalledAgent } from '../types/agents';
import { useApp } from '../context/AppContext';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  defaultAgent?: InstalledAgent;
}

export const NewSessionModal: React.FC<Props> = ({ isOpen, onClose, defaultAgent }) => {
  const { sessions, setSelectedSession, refreshSessions } = useApp();
  const [step, setStep] = useState<'pick' | 'goal'>(defaultAgent ? 'goal' : 'pick');
  const [agents, setAgents] = useState<InstalledAgent[]>([]);
  const [selected, setSelected] = useState<InstalledAgent | null>(defaultAgent ?? null);
  const [goal, setGoal] = useState(defaultAgent?.description ?? '');
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState('');

  useEffect(() => {
    if (!isOpen) return;
    if (defaultAgent) {
      setSelected(defaultAgent);
      setGoal(defaultAgent.description ?? '');
      setStep('goal');
    } else {
      setStep('pick');
      setSelected(null);
      setGoal('');
    }
    setSearch('');
  }, [isOpen, defaultAgent]);

  useEffect(() => {
    if (isOpen && step === 'pick') {
      invoke<string>('list_installed', {})
        .then((json) => {
          try {
            const raw = JSON.parse(json);
            setAgents(Array.isArray(raw) ? raw : []);
          } catch {
            setAgents([]);
          }
        })
        .catch(() => setAgents([]));
    }
  }, [isOpen, step]);

  const filteredAgents = agents.filter(
    (a) =>
      a.name.toLowerCase().includes(search.toLowerCase()) ||
      (a.description ?? '').toLowerCase().includes(search.toLowerCase())
  );

  const handlePickAgent = (agent: InstalledAgent) => {
    setSelected(agent);
    setGoal(agent.description ?? '');
    setStep('goal');
  };

  const handleStart = async () => {
    if (!selected || !goal.trim() || loading) return;
    setLoading(true);
    try {
      const pidStr = await invoke<string>('spawn_agent', {
        name: selected.name,
        description: goal.trim(),
      });
      const pid = parseInt(pidStr, 10);
      await refreshSessions();
      // Find the session whose ownerPid matches the newly spawned PID
      const session = sessions.find((s) => s.ownerPid === pid);
      if (session) {
        setSelectedSession(session.id);
      }
      onClose();
    } catch {
      // ignore errors — user can retry
    } finally {
      setLoading(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div style={styles.overlay} onClick={onClose}>
      <div style={styles.modal} onClick={(e) => e.stopPropagation()}>
        {step === 'pick' ? (
          <>
            <div style={styles.header}>
              <span style={styles.title}>New Session</span>
              <button style={styles.closeBtn} onClick={onClose}>✕</button>
            </div>
            <input
              style={styles.searchInput}
              placeholder="Search agents…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              autoFocus
            />
            <div style={styles.agentList}>
              {filteredAgents.length === 0 ? (
                <div style={styles.emptyState}>No agents installed</div>
              ) : (
                filteredAgents.map((agent) => (
                  <div
                    key={agent.name}
                    style={styles.agentRow}
                    onClick={() => handlePickAgent(agent)}
                  >
                    <div style={styles.agentRowLeft}>
                      <span style={styles.agentName}>{agent.name}</span>
                      <span style={styles.scopeBadge(agent.scope)}>
                        {agent.scope === 'system' ? 'SYS' : 'USR'}
                      </span>
                      {agent.version && (
                        <span style={styles.versionBadge}>v{agent.version}</span>
                      )}
                    </div>
                    <div style={styles.agentDesc}>
                      {agent.description ?? ''}
                    </div>
                  </div>
                ))
              )}
            </div>
          </>
        ) : (
          <>
            <div style={styles.header}>
              <span style={styles.title}>
                {selected?.name ?? 'Agent'}
                {selected && (
                  <span style={{ ...styles.scopeBadge(selected.scope), marginLeft: 8 }}>
                    {selected.scope === 'system' ? 'SYS' : 'USR'}
                  </span>
                )}
              </span>
              <button style={styles.closeBtn} onClick={onClose}>✕</button>
            </div>
            <textarea
              style={styles.goalTextarea}
              placeholder="What should this agent do?"
              value={goal}
              onChange={(e) => setGoal(e.target.value)}
              autoFocus
            />
            <div style={styles.footerRow}>
              {!defaultAgent && (
                <button style={styles.backBtn} onClick={() => setStep('pick')}>
                  ← Back
                </button>
              )}
              <button
                style={styles.startBtn(!goal.trim() || loading)}
                disabled={!goal.trim() || loading}
                onClick={handleStart}
              >
                {loading ? 'Starting…' : 'Start Session'}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
};

const styles = {
  overlay: {
    position: 'fixed' as const,
    inset: 0,
    background: 'rgba(0,0,0,0.6)',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    zIndex: 1000,
  },
  modal: {
    background: '#1e1e2e',
    border: '1px solid #313244',
    borderRadius: 8,
    width: 540,
    maxHeight: '80vh',
    display: 'flex',
    flexDirection: 'column' as const,
    overflow: 'hidden',
  },
  header: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    padding: '16px 20px',
    borderBottom: '1px solid #313244',
  },
  title: {
    color: '#cdd6f4',
    fontWeight: 600,
    fontSize: 15,
    display: 'flex',
    alignItems: 'center',
  },
  closeBtn: {
    background: 'none',
    border: 'none',
    color: '#6c7086',
    cursor: 'pointer',
    fontSize: 16,
    lineHeight: 1,
    padding: 4,
  },
  searchInput: {
    margin: '12px 20px 8px',
    background: '#181825',
    border: '1px solid #313244',
    borderRadius: 6,
    color: '#cdd6f4',
    padding: '8px 12px',
    fontSize: 13,
    outline: 'none',
  },
  agentList: {
    overflowY: 'auto' as const,
    flex: 1,
    padding: '0 12px 12px',
  },
  emptyState: {
    color: '#6c7086',
    textAlign: 'center' as const,
    padding: 32,
    fontSize: 13,
  },
  agentRow: {
    padding: '10px 12px',
    borderRadius: 6,
    cursor: 'pointer',
    marginBottom: 4,
    background: '#181825',
    border: '1px solid transparent',
  },
  agentRowLeft: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    marginBottom: 4,
  },
  agentName: {
    color: '#cdd6f4',
    fontWeight: 500,
    fontSize: 13,
  },
  scopeBadge: (scope: string) => ({
    fontSize: 10,
    fontWeight: 700,
    padding: '2px 6px',
    borderRadius: 4,
    background: scope === 'system' ? '#45475a' : '#313244',
    color: scope === 'system' ? '#bac2de' : '#89b4fa',
  }),
  versionBadge: {
    fontSize: 10,
    color: '#6c7086',
  },
  agentDesc: {
    color: '#6c7086',
    fontSize: 12,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap' as const,
  },
  goalTextarea: {
    margin: '16px 20px 12px',
    background: '#181825',
    border: '1px solid #313244',
    borderRadius: 6,
    color: '#cdd6f4',
    padding: '10px 12px',
    fontSize: 13,
    resize: 'vertical' as const,
    minHeight: 120,
    outline: 'none',
  },
  footerRow: {
    display: 'flex',
    justifyContent: 'flex-end',
    gap: 8,
    padding: '0 20px 16px',
  },
  backBtn: {
    background: 'none',
    border: '1px solid #45475a',
    borderRadius: 6,
    color: '#bac2de',
    cursor: 'pointer',
    fontSize: 13,
    padding: '6px 14px',
  },
  startBtn: (disabled: boolean) => ({
    background: disabled ? '#313244' : '#89b4fa',
    border: 'none',
    borderRadius: 6,
    color: disabled ? '#6c7086' : '#1e1e2e',
    cursor: disabled ? 'not-allowed' : 'pointer',
    fontSize: 13,
    fontWeight: 600,
    padding: '6px 18px',
  }),
};
