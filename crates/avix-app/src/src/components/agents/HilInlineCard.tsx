import React, { useState } from 'react';
import { invoke } from '../../platform';
import { useNotification } from '../../context/NotificationContext';
import { Notification } from '../../types/notifications';
import toast from 'react-hot-toast';

interface Props {
  notif: Notification;
}

const HilInlineCard: React.FC<Props> = ({ notif }) => {
  const { markRead } = useNotification();
  const [resolving, setResolving] = useState(false);
  const hil = notif.hil!;

  const resolve = async (approve: boolean) => {
    setResolving(true);
    try {
      await invoke('resolve_hil', { id: notif.id, approve });
      await markRead(notif.id);
      toast.success(approve ? 'HIL approved' : 'HIL denied');
    } catch (e) {
      toast.error(`Failed to resolve HIL: ${e}`);
      setResolving(false);
    }
  };

  return (
    <div
      style={{
        margin: '12px 16px',
        padding: '14px 16px',
        backgroundColor: 'rgba(245,158,11,0.08)',
        border: '1px solid rgba(245,158,11,0.3)',
        borderRadius: '10px',
        display: 'flex',
        flexDirection: 'column',
        gap: '12px',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: '10px' }}>
        <div
          style={{
            flexShrink: 0,
            width: '20px',
            height: '20px',
            borderRadius: '50%',
            backgroundColor: 'rgba(245,158,11,0.2)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="#f59e0b" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
            <line x1="12" y1="9" x2="12" y2="13" />
            <line x1="12" y1="17" x2="12.01" y2="17" />
          </svg>
        </div>
        <div style={{ flex: 1 }}>
          <div style={{ fontSize: '11px', fontWeight: 700, color: '#f59e0b', marginBottom: '4px', letterSpacing: '0.06em', textTransform: 'uppercase' }}>
            Human Approval Required
          </div>
          <div style={{ fontSize: '13px', color: '#e2e8f0', lineHeight: '1.55', whiteSpace: 'pre-wrap' }}>
            {hil.prompt}
          </div>
          {hil.timeout_secs && (
            <div style={{ marginTop: '6px', fontSize: '11px', color: '#64748b' }}>
              Timeout: {hil.timeout_secs}s
            </div>
          )}
        </div>
      </div>

      <div style={{ display: 'flex', gap: '8px', justifyContent: 'flex-end' }}>
        <button
          disabled={resolving}
          onClick={() => resolve(false)}
          style={{
            padding: '6px 16px',
            backgroundColor: resolving ? '#334155' : 'rgba(239,68,68,0.15)',
            color: resolving ? '#64748b' : '#ef4444',
            border: '1px solid rgba(239,68,68,0.3)',
            borderRadius: '6px',
            fontSize: '12px',
            fontWeight: 600,
            cursor: resolving ? 'not-allowed' : 'pointer',
            transition: 'background 0.15s',
          }}
        >
          Deny
        </button>
        <button
          disabled={resolving}
          onClick={() => resolve(true)}
          style={{
            padding: '6px 16px',
            backgroundColor: resolving ? '#334155' : 'rgba(34,197,94,0.15)',
            color: resolving ? '#64748b' : '#22c55e',
            border: '1px solid rgba(34,197,94,0.3)',
            borderRadius: '6px',
            fontSize: '12px',
            fontWeight: 600,
            cursor: resolving ? 'not-allowed' : 'pointer',
            transition: 'background 0.15s',
          }}
        >
          {resolving ? 'Resolving…' : 'Approve'}
        </button>
      </div>
    </div>
  );
};

export default HilInlineCard;
