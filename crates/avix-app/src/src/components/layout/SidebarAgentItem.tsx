import React from 'react';
import { Agent } from '../../types/agents';

const statusColor: Record<string, string> = {
  running: '#22c55e',
  paused: '#f59e0b',
  stopped: '#64748b',
  crashed: '#ef4444',
};

interface Props {
  agent: Agent;
  isSelected: boolean;
  hilCount: number;
  onClick: () => void;
}

const SidebarAgentItem: React.FC<Props> = ({ agent, isSelected, hilCount, onClick }) => {
  const dot = statusColor[agent.status] ?? '#64748b';
  const shortName = agent.name.length > 22 ? agent.name.slice(0, 20) + '…' : agent.name;

  return (
    <button
      onClick={onClick}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: '10px',
        width: '100%',
        padding: '8px 12px',
        background: isSelected ? '#1e3a5f' : 'none',
        border: 'none',
        borderRadius: '6px',
        cursor: 'pointer',
        textAlign: 'left',
        transition: 'background 0.12s',
        borderLeft: isSelected ? '2px solid #3b82f6' : '2px solid transparent',
      }}
      onMouseEnter={(e) => {
        if (!isSelected) (e.currentTarget as HTMLButtonElement).style.background = '#1e293b';
      }}
      onMouseLeave={(e) => {
        if (!isSelected) (e.currentTarget as HTMLButtonElement).style.background = 'none';
      }}
    >
      {/* Status dot */}
      <span
        style={{
          width: '7px',
          height: '7px',
          borderRadius: '50%',
          backgroundColor: dot,
          flexShrink: 0,
          boxShadow: agent.status === 'running' ? `0 0 6px ${dot}` : 'none',
        }}
      />

      {/* Name */}
      <span
        style={{
          flex: 1,
          fontSize: '13px',
          color: isSelected ? '#f8fafc' : '#cbd5e1',
          fontWeight: isSelected ? 500 : 400,
          overflow: 'hidden',
          whiteSpace: 'nowrap',
          textOverflow: 'ellipsis',
        }}
      >
        {shortName}
      </span>

      {/* HIL badge */}
      {hilCount > 0 && (
        <span
          style={{
            backgroundColor: '#f59e0b',
            color: '#0f172a',
            borderRadius: '9999px',
            fontSize: '10px',
            fontWeight: 700,
            minWidth: '18px',
            height: '18px',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            padding: '0 4px',
            flexShrink: 0,
          }}
        >
          {hilCount}
        </span>
      )}
    </button>
  );
};

export default SidebarAgentItem;
