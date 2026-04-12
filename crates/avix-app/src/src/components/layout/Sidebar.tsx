import React, { useState } from 'react';
import { useApp } from '../../context/AppContext';
import { useNotification } from '../../context/NotificationContext';
import { NotificationKind } from '../../types/notifications';
import { Page, Session } from '../../types/agents';
import { NewSessionModal } from '../NewSessionModal';

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

const statusDot = (status: Session['status']) => {
  const color =
    status === 'running' ? '#a6e3a1' :
    status === 'paused' ? '#f9e2af' :
    status === 'idle' ? '#89b4fa' :
    '#6c7086';
  return (
    <span
      style={{
        width: 7,
        height: 7,
        borderRadius: '50%',
        background: color,
        flexShrink: 0,
        display: 'inline-block',
      }}
    />
  );
};

const Sidebar: React.FC = () => {
  const {
    currentPage,
    setPage,
    sessions,
    selectedSessionId,
    setSelectedSession,
  } = useApp();
  const { notifications } = useNotification();
  const [newSessionOpen, setNewSessionOpen] = useState(false);

  const activeStatuses = new Set(['running', 'idle', 'paused']);
  const activeSessions = sessions.filter((s) => activeStatuses.has(s.status));

  const getSessionHilCount = (session: Session) =>
    notifications.filter(
      (n) =>
        n.kind === NotificationKind.Hil &&
        session.pids.includes(n.agent_pid ?? 0) &&
        !n.hil?.outcome &&
        !n.resolved_at
    ).length;

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

      {/* Sessions section */}
      <div style={{ flex: 1, overflow: 'auto', display: 'flex', flexDirection: 'column', gap: '1px' }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            padding: '4px 12px 6px',
          }}
        >
          <span
            style={{
              fontSize: '10px',
              fontWeight: 700,
              color: '#475569',
              letterSpacing: '0.08em',
              textTransform: 'uppercase',
            }}
          >
            Sessions
          </span>
          <button
            onClick={() => setNewSessionOpen(true)}
            title="New Session"
            style={{
              background: 'none',
              border: 'none',
              color: '#475569',
              cursor: 'pointer',
              fontSize: 16,
              lineHeight: 1,
              padding: '0 2px',
            }}
          >
            +
          </button>
        </div>

        {activeSessions.length === 0 ? (
          <p
            style={{
              padding: '8px 12px',
              fontSize: '12px',
              color: '#475569',
              fontStyle: 'italic',
            }}
          >
            No active sessions — click + to start one
          </p>
        ) : (
          activeSessions.map((session) => {
            const hilCount = getSessionHilCount(session);
            const isSelected = selectedSessionId === session.id && currentPage === 'session';
            const title = (session.title || session.goal || '').slice(0, 40);
            return (
              <button
                key={session.id}
                onClick={() => setSelectedSession(session.id)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  width: '100%',
                  padding: '7px 12px',
                  background: isSelected ? '#1e293b' : 'none',
                  border: 'none',
                  borderRadius: 6,
                  cursor: 'pointer',
                  textAlign: 'left',
                  color: isSelected ? '#f8fafc' : '#94a3b8',
                  fontSize: 12,
                  transition: 'background 0.12s',
                }}
              >
                {statusDot(session.status)}
                <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {title || 'Session'}
                </span>
                {hilCount > 0 && (
                  <span
                    style={{
                      background: '#f38ba8',
                      color: '#1e1e2e',
                      borderRadius: 10,
                      fontSize: 10,
                      fontWeight: 700,
                      padding: '1px 6px',
                      flexShrink: 0,
                    }}
                  >
                    {hilCount}
                  </span>
                )}
              </button>
            );
          })
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
          label="Catalog"
          active={currentPage === 'catalog'}
          onClick={() => setPage('catalog' as Page)}
          icon={
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
              <line x1="3" y1="9" x2="21" y2="9" />
              <line x1="9" y1="21" x2="9" y2="9" />
            </svg>
          }
        />
        <NavItem
          label="History"
          active={currentPage === 'history'}
          onClick={() => setPage('history' as Page)}
          icon={
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="12" cy="12" r="10" />
              <polyline points="12 6 12 12 16 14" />
            </svg>
          }
        />
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

      <NewSessionModal isOpen={newSessionOpen} onClose={() => setNewSessionOpen(false)} />
    </div>
  );
};

export default Sidebar;
