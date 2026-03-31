import React from 'react';
import { Notification, NotificationKind } from '../types/notifications';
import { invoke } from '../platform';
import toast from 'react-hot-toast';

interface Props {
  notif: Notification;
}

const NotificationToast: React.FC<Props> = ({ notif }) => {
  if (notif.kind !== NotificationKind.Hil) {
    return (
      <div style={{ padding: '1rem' }}>
        <p>{notif.message}</p>
      </div>
    );
  }

  return (
    <div style={{
      background: 'white',
      padding: '1.5rem',
      borderRadius: '8px',
      boxShadow: '0 10px 25px rgba(0,0,0,0.2)',
      maxWidth: '400px',
      minHeight: '150px',
    }}>
      <h3 style={{ margin: 0, marginBottom: '0.5rem', color: '#333' }}>HIL Request</h3>
      <p style={{ margin: 0, marginBottom: '1rem', color: '#666', whiteSpace: 'pre-wrap' }}>{notif.hil!.prompt}</p>
      <div style={{ display: 'flex', gap: '1rem', justifyContent: 'flex-end' }}>
        <button
          onClick={async () => {
            await invoke('resolve_hil', { id: notif.id, approve: false });
            toast.dismiss();
          }}
          style={{
            background: '#ef4444',
            color: 'white',
            border: 'none',
            padding: '0.75rem 1.5rem',
            borderRadius: '6px',
            cursor: 'pointer',
            fontWeight: 'bold',
          }}
        >
          Deny
        </button>
        <button
          onClick={async () => {
            await invoke('resolve_hil', { id: notif.id, approve: true });
            toast.dismiss();
          }}
          style={{
            background: '#10b981',
            color: 'white',
            border: 'none',
            padding: '0.75rem 1.5rem',
            borderRadius: '6px',
            cursor: 'pointer',
            fontWeight: 'bold',
          }}
        >
          Approve
        </button>
      </div>
    </div>
  );
};

export default NotificationToast;