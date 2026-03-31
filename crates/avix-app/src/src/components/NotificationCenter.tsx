import React from 'react';
import { useNotification } from '../context/NotificationContext';

interface Props {
  onClose: () => void;
}

const NotificationCenter: React.FC<Props> = ({ onClose }) => {
  const { notifications, markRead, unreadCount } = useNotification();
  const unread = notifications.filter(n => !n.read).reverse();

  return (
    <div style={{
      background: 'white',
      width: '500px',
      height: '600px',
      borderRadius: '12px',
      boxShadow: '0 25px 50px rgba(0,0,0,0.25)',
      overflow: 'hidden',
      display: 'flex',
      flexDirection: 'column',
    }}>
      <div style={{
        padding: '1rem 1.5rem',
        backgroundColor: '#f3f4f6',
        borderBottom: '1px solid #e5e7eb',
        display: 'flex',
        justifyContent: 'space-between',
        alignItems: 'center',
        fontWeight: 'bold',
        fontSize: '1.1rem',
      }}>
        Notifications ({unreadCount})
        <button
          onClick={onClose}
          style={{
            background: 'none',
            border: 'none',
            fontSize: '1.5rem',
            cursor: 'pointer',
            color: '#6b7280',
          }}
        >
          ×
        </button>
      </div>
      <div style={{
        flex: 1,
        overflow: 'auto',
        padding: '1rem',
      }}>
        {unread.map((n) => (
          <div
            key={n.id}
            style={{
              padding: '1rem',
              marginBottom: '0.5rem',
              backgroundColor: '#f9fafb',
              borderRadius: '8px',
              border: '1px solid #e5e7eb',
            }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '0.5rem' }}>
              <strong style={{ color: '#374151' }}>{n.kind}</strong>
              <button
                onClick={() => markRead(n.id)}
                style={{
                  background: '#3b82f6',
                  color: 'white',
                  border: 'none',
                  padding: '0.25rem 0.5rem',
                  borderRadius: '4px',
                  cursor: 'pointer',
                  fontSize: '0.8rem',
                }}
              >
                Mark read
              </button>
            </div>
            <p style={{ margin: 0, marginBottom: '0.5rem', color: '#4b5563', whiteSpace: 'pre-wrap' }}>{n.message}</p>
            <div style={{ fontSize: '0.8rem', color: '#9ca3af' }}>
              {new Date(n.created_at).toLocaleString()}
              {n.agent_pid && ` | Agent ${n.agent_pid}`}
            </div>
          </div>
        ))}
        {unread.length === 0 && (
          <p style={{ textAlign: 'center', color: '#9ca3af', marginTop: '2rem' }}>
            No unread notifications
          </p>
        )}
      </div>
    </div>
  );
};

export default NotificationCenter;