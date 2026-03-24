import React from 'react';
import { useNotification } from '../context/NotificationContext';
import { NotificationKind } from '../types/notifications';

interface Props {
  agentId: string;
}

const HilBanner: React.FC&lt;Props&gt; = ({ agentId }) =&gt; {
  const { notifications } = useNotification();
  const pendingHils = notifications.filter(n =&gt; 
    n.kind === NotificationKind.Hil &amp;&amp; 
    n.agent_pid === parseInt(agentId) &amp;&amp; 
    !n.hil?.outcome &amp;&amp; 
    !n.resolved_at
  );

  if (!pendingHils.length) return null;

  return (
    &lt;div style={{
      position: 'sticky',
      top: 0,
      zIndex: 10,
      backgroundColor: '#f59e0b',
      color: 'white',
      padding: '1rem',
      borderBottom: '2px solid #d97706',
      display: 'flex',
      flexDirection: 'column',
      gap: '0.5rem',
    }}&gt;
      {pendingHils.map((hil) =&gt; (
        &lt;div key={hil.id} style={{ display: 'flex', gap: '1rem', alignItems: 'center' }}&gt;
          &lt;strong&gt;HIL Request:&lt;/strong&gt;
          &lt;span style={{ flex: 1 }}&gt;{hil.hil!.prompt}&lt;/span&gt;
          &lt;button
            onClick={async () =&gt; {
              await invoke('resolve_hil', { id: hil.id, approve: true });
            }}
            style={{ background: '#10b981', color: 'white', border: 'none', padding: '0.5rem 1rem', borderRadius: '4px', cursor: 'pointer' }}
          &gt;
            Approve
          &lt;/button&gt;
          &lt;button
            onClick={async () =&gt; {
              await invoke('resolve_hil', { id: hil.id, approve: false });
            }}
            style={{ background: '#ef4444', color: 'white', border: 'none', padding: '0.5rem 1rem', borderRadius: '4px', cursor: 'pointer' }}
          &gt;
            Deny
          &lt;/button&gt;
        &lt;/div&gt;
      ))}
    &lt;/div&gt;
  );
};

export default HilBanner;