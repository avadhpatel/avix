import React from 'react';
import { useApp } from '../context/AppContext';
import { useNotification } from '../context/NotificationContext';
import { NotificationKind } from '../types/notifications';
import AgentThread from '../components/agents/AgentThread';
import AgentInputBar from '../components/agents/AgentInputBar';
import HilInlineCard from '../components/agents/HilInlineCard';
import AgentStatusBadge from '../components/agents/AgentStatusBadge';

const AgentThreadPage: React.FC = () => {
  const { agents, selectedAgentPid, agentOutputs, streamingOutputs } = useApp();
  const { notifications } = useNotification();

  const agent = agents.find((a) => a.pid === selectedAgentPid);
  const outputs = selectedAgentPid != null ? (agentOutputs[selectedAgentPid] ?? []) : [];

  const pendingHils = notifications.filter(
    (n) =>
      n.kind === NotificationKind.Hil &&
      n.agent_pid === selectedAgentPid &&
      n.hil &&
      !n.hil.outcome &&
      !n.resolved_at
  );

  if (selectedAgentPid == null || !agent) {
    return (
      <div
        style={{
          flex: 1,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
          color: '#334155',
          gap: '12px',
        }}
      >
        <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="round" strokeLinejoin="round">
          <rect x="2" y="3" width="20" height="14" rx="2" ry="2"/>
          <line x1="8" y1="21" x2="16" y2="21"/>
          <line x1="12" y1="17" x2="12" y2="21"/>
        </svg>
        <p style={{ fontSize: '14px', margin: 0 }}>Select an agent from the sidebar</p>
        <p style={{ fontSize: '12px', color: '#1e293b', margin: 0 }}>or click + Add Agent to spawn a new one</p>
      </div>
    );
  }

  const isActive = agent.status === 'running' || agent.status === 'paused';

  return (
    <div
      style={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
      }}
    >
      {/* Agent header */}
      <div
        style={{
          padding: '12px 16px',
          borderBottom: '1px solid #1e293b',
          display: 'flex',
          alignItems: 'center',
          gap: '12px',
          flexShrink: 0,
        }}
      >
        <div style={{ flex: 1 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
            <span style={{ color: '#f8fafc', fontWeight: 600, fontSize: '15px' }}>
              {agent.name}
            </span>
            <AgentStatusBadge status={agent.status} />
            <span style={{ color: '#334155', fontSize: '11px' }}>
              PID {agent.pid}
            </span>
          </div>
          {agent.goal && (
            <p
              style={{
                margin: '3px 0 0',
                fontSize: '12px',
                color: '#475569',
                whiteSpace: 'nowrap',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                maxWidth: '600px',
              }}
            >
              {agent.goal}
            </p>
          )}
        </div>
      </div>

      {/* HIL cards */}
      {pendingHils.length > 0 && (
        <div
          style={{
            borderBottom: '1px solid rgba(245,158,11,0.2)',
            backgroundColor: 'rgba(245,158,11,0.03)',
            flexShrink: 0,
          }}
        >
          {pendingHils.map((n) => (
            <HilInlineCard key={n.id} notif={n} />
          ))}
        </div>
      )}

      {/* Thread */}
      <AgentThread
        outputs={outputs}
        agentPid={agent.pid}
        agentName={agent.name}
        streamingText={selectedAgentPid != null ? streamingOutputs[selectedAgentPid] : undefined}
      />

      {/* Input bar */}
      <AgentInputBar agentPid={agent.pid} disabled={!isActive} />
    </div>
  );
};

export default AgentThreadPage;
