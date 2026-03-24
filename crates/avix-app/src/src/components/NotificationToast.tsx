import React from 'react';
import { Notification, NotificationKind, HilOutcome } from '../types/notifications';
import { invoke } from '@tauri-apps/api/tauri';
import toast from 'react-hot-toast';

interface Props {
  notif: Notification;
  toast: any; // toast props
}

const NotificationToast: React.FC&lt;Props&gt; = ({ notif }) =&gt; {
  if (notif.kind !== NotificationKind.Hil) {
    return (
      &lt;div style={{ padding: '1rem' }}&gt;
        &lt;p&gt;{notif.message}&lt;/p&gt;
      &lt;/div&gt;
    );
  }

  return (
    &lt;div style={{
      background: 'white',
      padding: '1.5rem',
      borderRadius: '8px',
      boxShadow: '0 10px 25px rgba(0,0,0,0.2)',
      maxWidth: '400px',
      minHeight: '150px',
    }}&gt;
      &lt;h3 style={{ margin: 0, marginBottom: '0.5rem', color: '#333' }}&gt;HIL Request&lt;/h3&gt;
      &lt;p style={{ margin: 0, marginBottom: '1rem', color: '#666', whiteSpace: 'pre-wrap' }}&gt;{notif.hil!.prompt}&lt;/p&gt;
      &lt;div style={{ display: 'flex', gap: '1rem', justifyContent: 'flex-end' }}&gt;
        &lt;button
          onClick={async () =&gt; {
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
        &gt;
          Deny
        &lt;/button&gt;
        &lt;button
          onClick={async () =&gt; {
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
        &gt;
          Approve
        &lt;/button&gt;
      &lt;/div&gt;
    &lt;/div&gt;
  );
};

export default NotificationToast;