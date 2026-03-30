import React from 'react';
import { useApp } from '../../context/AppContext';
import { useNotification } from '../../context/NotificationContext';
import { NotificationKind } from '../../types/notifications';
import { Page } from '../../types/agents';
import SidebarAgentItem from './SidebarAgentItem';

const NavItem: React.FC<{
  label: string;
  icon: React.ReactNode;
  active: boolean;
  onClick: () => void;
}> = ({ label, icon, active, onClick }) => (
  <button
    onClick={onClick}
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: '10px',
      width: '100%',
      padding: '8px 12px',
      background: active ? '#1e293b' : 'none',
      border: 'none',
      borderRadius: '6px',
      cursor: 'pointer',
      textAlign: 'left',
      color: active ? '#f8fafc' : '#94a3b8',
      fontSize: '13px',
      fontWeight: active ? 600 : 400,
      transition: 'background 0.12s, color 0.12s',
    }}
    onMouseEnter={(e) => {
      if (!active) {
        (e.currentTarget as HTMLButtonElement).style.background = '#1e293b';
        (e.currentTarget as HTMLButtonElement).style.color = '#cbd5e1';
      }
    }}
    onMouseLeave={(e) => {
      if (!active) {
        (e.currentTarget as HTMLButtonElement).style.background = 'none';
        (e.currentTarget as HTMLButtonElement).style.color = '#94a3b8';
      }
    }}
  >
    {icon}
    {label}
  </button>
);

const Sidebar: React.FC = () => {
  const { agents, selectedAgentPid, currentPage, setSelectedAgent, setPage } = useApp();
  const { notifications } = useNotification();

  const getHilCount = (pid: number) =>
    notifications.filter(
      (n) =>
        n.kind === NotificationKind.Hil &&
        n.agent_pid === pid &&
        !n.hil?.outcome &&
        !n.resolved_at
    ).length;

  const runningAgents = agents.filter((a) => a.status !== 'stopped');
  const stoppedAgents = agents.filter((a) => a.status === 'stopped');

  return (
    <div
      style={{
        height: '100%',
        backgroundColor: '#0d1829',
        display: 'flex',
        flexDirection: 'column',
        padding: '8px',
        gap: '2px',
      }}
    >
      {/* Logo / app name */}
      <div
        style={{
          padding: '10px 12px 16px',
          borderBottom: '1px solid #1e293b',
          marginBottom: '8px',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <div
            style={{
              width: '24px',
              height: '24px',
              borderRadius: '6px',
              background: 'linear-gradient(135deg, #3b82f6 0%, #8b5cf6 100%)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              flexShrink: 0,
            }}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="white">
              <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" fill="none"/>
            </svg>
          </div>
          <span
            style={{
              color: '#f8fafc',
              fontWeight: 700,
              fontSize: '15px',
              letterSpacing: '-0.01em',
            }}
          >
            Avix
          </span>
        </div>
      </div>

      {/* Agents section */}
      <div style={{ flex: 1, overflow: 'auto', display: 'flex', flexDirection: 'column', gap: '1px' }}>
        <div
          style={{
            padding: '4px 12px 6px',
            fontSize: '10px',
            fontWeight: 700,
            color: '#475569',
            letterSpacing: '0.08em',
            textTransform: 'uppercase',
          }}
        >
          Agents
        </div>

        {agents.length === 0 && (
          <p
            style={{
              padding: '8px 12px',
              fontSize: '12px',
              color: '#475569',
              fontStyle: 'italic',
            }}
          >
            No agents running
          </p>
        )}

        {runningAgents.map((agent) => (
          <SidebarAgentItem
            key={agent.pid}
            agent={agent}
            isSelected={selectedAgentPid === agent.pid && currentPage === 'agent'}
            hilCount={getHilCount(agent.pid)}
            onClick={() => setSelectedAgent(agent.pid)}
          />
        ))}

        {stoppedAgents.length > 0 && (
          <>
            <div
              style={{
                padding: '8px 12px 4px',
                fontSize: '10px',
                fontWeight: 700,
                color: '#334155',
                letterSpacing: '0.08em',
                textTransform: 'uppercase',
                marginTop: '4px',
              }}
            >
              Stopped
            </div>
            {stoppedAgents.map((agent) => (
              <SidebarAgentItem
                key={agent.pid}
                agent={agent}
                isSelected={selectedAgentPid === agent.pid && currentPage === 'agent'}
                hilCount={0}
                onClick={() => setSelectedAgent(agent.pid)}
              />
            ))}
          </>
        )}
      </div>

      {/* Bottom nav */}
      <div
        style={{
          borderTop: '1px solid #1e293b',
          paddingTop: '8px',
          display: 'flex',
          flexDirection: 'column',
          gap: '1px',
        }}
      >
        <NavItem
          label="Services"
          active={currentPage === 'services'}
          onClick={() => setPage('services' as Page)}
          icon={
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
              <line x1="8" y1="21" x2="16" y2="21" />
              <line x1="12" y1="17" x2="12" y2="21" />
            </svg>
          }
        />
        <NavItem
          label="Tools"
          active={currentPage === 'tools'}
          onClick={() => setPage('tools' as Page)}
          icon={
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
            </svg>
          }
        />
      </div>
    </div>
  );
};

export default Sidebar;
