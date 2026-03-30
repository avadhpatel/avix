import React, { useEffect, useRef } from 'react';
import { useNotification } from '../../context/NotificationContext';
import { useApp } from '../../context/AppContext';
import { NotificationKind } from '../../types/notifications';

interface Props {
  onClose: () => void;
}

const kindLabel: Record<string, { label: string; color: string }> = {
  [NotificationKind.Hil]: { label: 'HIL', color: '#f59e0b' },
  [NotificationKind.AgentExit]: { label: 'Exit', color: '#64748b' },
  [NotificationKind.SysAlert]: { label: 'Alert', color: '#ef4444' },
};

const NotificationCenter: React.FC<Props> = ({ onClose }) => {
  const { notifications, markRead, unreadCount } = useNotification();
  const { setSelectedAgent } = useApp();
  const ref = useRef<HTMLDivElement>(null);

  const sorted = [...notifications].sort(
    (a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
  );
  const unread = sorted.filter((n) => !n.read);
  const read = sorted.filter((n) => n.read);

  // Close on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose();
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [onClose]);

  const handleNotifClick = (notif: typeof notifications[0]) => {
    if (notif.agent_pid) {
      setSelectedAgent(notif.agent_pid);
      onClose();
    }
    if (!notif.read) {
      markRead(notif.id);
    }
  };

  const renderItem = (notif: typeof notifications[0]) => {
    const k = kindLabel[notif.kind] ?? { label: notif.kind, color: '#94a3b8' };
    return (
      <div
        key={notif.id}
        onClick={() => handleNotifClick(notif)}
        style={{
          padding: '12px 16px',
          borderRadius: '8px',
          backgroundColor: notif.read ? 'transparent' : 'rgba(59,130,246,0.05)',
          border: `1px solid ${notif.read ? '#1e293b' : '#1e3a5f'}`,
          cursor: notif.agent_pid ? 'pointer' : 'default',
          transition: 'background 0.12s',
          marginBottom: '6px',
        }}
        onMouseEnter={(e) => {
          if (notif.agent_pid) (e.currentTarget as HTMLDivElement).style.background = '#1e293b';
        }}
        onMouseLeave={(e) => {
          (e.currentTarget as HTMLDivElement).style.background = notif.read ? 'transparent' : 'rgba(59,130,246,0.05)';
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '5px' }}>
          <span
            style={{
              padding: '1px 7px',
              borderRadius: '9999px',
              fontSize: '10px',
              fontWeight: 700,
              backgroundColor: `${k.color}22`,
              color: k.color,
              letterSpacing: '0.04em',
            }}
          >
            {k.label}
          </span>
          {notif.agent_pid && (
            <span style={{ fontSize: '11px', color: '#475569' }}>Agent #{notif.agent_pid}</span>
          )}
          <span style={{ marginLeft: 'auto', fontSize: '10px', color: '#334155' }}>
            {new Date(notif.created_at).toLocaleTimeString()}
          </span>
          {!notif.read && (
            <button
              onClick={(e) => { e.stopPropagation(); markRead(notif.id); }}
              style={{
                background: 'none',
                border: 'none',
                color: '#475569',
                cursor: 'pointer',
                fontSize: '11px',
                padding: '0',
              }}
            >
              ✓
            </button>
          )}
        </div>
        <p style={{ margin: 0, fontSize: '12px', color: '#94a3b8', whiteSpace: 'pre-wrap', lineHeight: '1.45' }}>
          {notif.message}
        </p>
        {notif.hil && notif.hil.prompt && (
          <p style={{ margin: '4px 0 0', fontSize: '11px', color: '#64748b', fontStyle: 'italic' }}>
            {notif.hil.prompt.slice(0, 80)}{notif.hil.prompt.length > 80 ? '…' : ''}
          </p>
        )}
      </div>
    );
  };

  return (
    <div
      ref={ref}
      style={{
        position: 'fixed',
        top: '56px',
        right: '12px',
        width: '380px',
        maxHeight: '560px',
        backgroundColor: '#0d1829',
        border: '1px solid #1e293b',
        borderRadius: '12px',
        boxShadow: '0 20px 60px rgba(0,0,0,0.5)',
        overflow: 'hidden',
        display: 'flex',
        flexDirection: 'column',
        zIndex: 2000,
      }}
    >
      {/* Header */}
      <div
        style={{
          padding: '14px 16px',
          borderBottom: '1px solid #1e293b',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          <span style={{ color: '#f8fafc', fontWeight: 600, fontSize: '14px' }}>Notifications</span>
          {unreadCount > 0 && (
            <span
              style={{
                backgroundColor: '#f59e0b',
                color: '#0f172a',
                borderRadius: '9999px',
                fontSize: '10px',
                fontWeight: 700,
                padding: '1px 6px',
              }}
            >
              {unreadCount}
            </span>
          )}
        </div>
        <button
          onClick={onClose}
          style={{
            background: 'none',
            border: 'none',
            color: '#475569',
            cursor: 'pointer',
            fontSize: '18px',
            lineHeight: 1,
            padding: '2px',
            borderRadius: '4px',
          }}
        >
          ×
        </button>
      </div>

      {/* Content */}
      <div style={{ flex: 1, overflow: 'auto', padding: '12px' }}>
        {unread.length === 0 && read.length === 0 && (
          <p style={{ textAlign: 'center', color: '#334155', fontSize: '13px', marginTop: '24px' }}>
            No notifications
          </p>
        )}

        {unread.length > 0 && (
          <>
            <div style={{ fontSize: '10px', fontWeight: 700, color: '#334155', letterSpacing: '0.08em', textTransform: 'uppercase', marginBottom: '8px' }}>
              Unread
            </div>
            {unread.map(renderItem)}
          </>
        )}

        {read.length > 0 && (
          <>
            <div style={{ fontSize: '10px', fontWeight: 700, color: '#1e293b', letterSpacing: '0.08em', textTransform: 'uppercase', margin: '12px 0 8px' }}>
              Earlier
            </div>
            {read.map(renderItem)}
          </>
        )}
      </div>
    </div>
  );
};

export default NotificationCenter;
