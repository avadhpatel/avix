import React, { createContext, useContext, useState, useEffect, ReactNode } from 'react';
import { invoke, listen } from '@tauri-apps/api/tauri';
import { toast } from 'react-hot-toast';
import { Notification, NotificationKind } from '../types/notifications';
import NotificationToast from '../components/NotificationToast';

interface ContextType {
  notifications: Notification[];
  unreadCount: number;
  load: () => Promise&lt;void&gt;;
  add: (n: Notification) =&gt; void;
  markRead: (id: string) =&gt; Promise&lt;void&gt;;
}

const NotificationContext = createContext&lt;ContextType | null&gt;(null);

export const useNotification = () =&gt; {
  const context = useContext(NotificationContext);
  if (!context) {
    throw new Error('useNotification must be used within NotificationProvider');
  }
  return context;
};

export const NotificationProvider: React.FC&lt;{children: ReactNode}&gt; = ({ children }) =&gt; {
  const [notifications, setNotifications] = useState&lt;Notification[]&gt;([]);

  const unreadCount = notifications.filter(n =&gt; !n.read).length;

  const load = async () =&gt; {
    try {
      const json = await invoke&lt;string&gt;('get_notifications');
      const notifs: Notification[] = JSON.parse(json);
      setNotifications(notifs);
    } catch (e) {
      console.error('Failed to load notifications', e);
    }
  };

  const add = (n: Notification) =&gt; {
    setNotifications(prev =&gt; [n, ...prev.filter(p =&gt; p.id !== n.id)]);
    if (n.kind === NotificationKind.Hil) {
      toast.custom((t) =&gt; &lt;NotificationToast notif={n} /&gt;, {
        id: n.id,
        duration: Infinity,
        position: 'bottom-right',
      });
    } else {
      toast(n.message, { duration: 8000, position: 'bottom-right' });
    }
  };

  const markRead = async (id: string) =&gt; {
    try {
      await invoke('ack_notif', { id });
      setNotifications(prev =&gt; prev.map(notif =&gt;
        notif.id === id ? { ...notif, read: true } : notif
      ));
    } catch (e) {
      console.error('Failed to ack notif', e);
    }
  };

  useEffect(() =&gt; {
    load();
    let unlisten: () =&gt; void;
    listen&lt;Notification&gt;('notification', (event) =&gt; {
      add(event.payload);
    }).then((f) =&gt; {
      unlisten = f;
    }).catch(console.error);
    return () =&gt; {
      if (unlisten) unlisten();
    };
  }, []);

  return (
    &lt;NotificationContext.Provider value={{ notifications, unreadCount, load, add, markRead }}&gt;
      {children}
    &lt;/NotificationContext.Provider&gt;
  );
};