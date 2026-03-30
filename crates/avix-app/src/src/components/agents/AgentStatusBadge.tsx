import React from 'react';
import { AgentStatus } from '../../types/agents';

const config: Record<AgentStatus, { label: string; bg: string; color: string }> = {
  running: { label: 'Running', bg: 'rgba(34,197,94,0.15)', color: '#22c55e' },
  paused: { label: 'Paused', bg: 'rgba(245,158,11,0.15)', color: '#f59e0b' },
  stopped: { label: 'Stopped', bg: 'rgba(100,116,139,0.15)', color: '#64748b' },
  crashed: { label: 'Crashed', bg: 'rgba(239,68,68,0.15)', color: '#ef4444' },
};

interface Props {
  status: AgentStatus;
}

const AgentStatusBadge: React.FC<Props> = ({ status }) => {
  const c = config[status] ?? config.stopped;
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: '5px',
        padding: '2px 8px',
        borderRadius: '9999px',
        backgroundColor: c.bg,
        color: c.color,
        fontSize: '11px',
        fontWeight: 600,
        letterSpacing: '0.02em',
      }}
    >
      <span
        style={{
          width: '6px',
          height: '6px',
          borderRadius: '50%',
          backgroundColor: c.color,
          boxShadow: status === 'running' ? `0 0 4px ${c.color}` : 'none',
        }}
      />
      {c.label}
    </span>
  );
};

export default AgentStatusBadge;
