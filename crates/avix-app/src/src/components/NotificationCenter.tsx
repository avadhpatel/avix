import React from 'react';
import { useNotification } from '../context/NotificationContext';
import { NotificationKind } from '../types/notifications';

interface Props {
  onClose: () =&gt; void;
}

const NotificationCenter: React.FC&lt;Props&gt; = ({ onClose }) =&gt; {
  const { notifications, markRead, unreadCount } = useNotification();
  const unread = notifications.filter(n =&gt; !n.read).reverse();

  return (
    &lt;div style={{
      background: 'white',
      width: '500px',
      height: '600px',
      borderRadius: '12px',
      boxShadow: '0 25px 50px rgba(0,0,0,0.25)',
      overflow: 'hidden',
      display: 'flex',
      flexDirection: 'column',
    }}&gt;
      &lt;div style={{
        padding: '1rem 1.5rem',
        backgroundColor: '#f3f4f6',
        borderBottom: '1px solid #e5e7eb',
        display: 'flex',
        justifyContent: 'space-between',
        alignItems: 'center',
        fontWeight: 'bold',
        fontSize: '1.1rem',
      }}&gt;
        Notifications ({unreadCount})
        &lt;button
          onClick={onClose}
          style={{
            background: 'none',
            border: 'none',
            fontSize: '1.5rem',
            cursor: 'pointer',
            color: '#6b7280',
          }}
        &gt;
          ×
        &lt;/button&gt;
      &lt;/div&gt;
      &lt;div style={{
        flex: 1,
        overflow: 'auto',
        padding: '1rem',
      }}&gt;
        {unread.map((n) =&gt; (
          &lt;div
            key={n.id}
            style={{
              padding: '1rem',
              marginBottom: '0.5rem',
              backgroundColor: '#f9fafb',
              borderRadius: '8px',
              border: '1px solid #e5e7eb',
            }}
          &gt;
            &lt;div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '0.5rem' }}&gt;
              &lt;strong style={{ color: '#374151' }}&gt;{n.kind}&lt;/strong&gt;
              &lt;button
                onClick={() =&gt; markRead(n.id)}
                style={{
                  background: '#3b82f6',
                  color: 'white',
                  border: 'none',
                  padding: '0.25rem 0.5rem',
                  borderRadius: '4px',
                  cursor: 'pointer',
                  fontSize: '0.8rem',
                }}
              &gt;
                Mark read
              &lt;/button&gt;
            &lt;/div&gt;
            &lt;p style={{ margin: 0, marginBottom: '0.5rem', color: '#4b5563', whiteSpace: 'pre-wrap' }}&gt;{n.message}&lt;/p&gt;
            &lt;div style={{ fontSize: '0.8rem', color: '#9ca3af' }}&gt;
              {new Date(n.created_at).toLocaleString()}
              {n.agent_pid &amp;&amp; ` | Agent ${n.agent_pid}`}
            &lt;/div&gt;
          &lt;/div&gt;
        ))}
        {unread.length === 0 &amp;&amp; (
          &lt;p style={{ textAlign: 'center', color: '#9ca3af', marginTop: '2rem' }}&gt;
            No unread notifications
          &lt;/p&gt;
        )}
      &lt;/div&gt;
    &lt;/div&gt;
  );
};

export default NotificationCenter;