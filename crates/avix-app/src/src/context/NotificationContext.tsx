import React, { createContext, useContext, useState, useEffect, ReactNode } from 'react';
import { invoke, listen } from '../platform';
import { toast } from 'react-hot-toast';
import { Notification, NotificationKind } from '../types/notifications';
import NotificationToast from '../components/NotificationToast';

interface ContextType {
  notifications: Notification[];
  unreadCount: number;
  load: () => Promise<void>;
  add: (n: Notification) => void;
  markRead: (id: string) => Promise<void>;
}

const NotificationContext = createContext<ContextType | null>(null);

export const useNotification = () => {
  const context = useContext(NotificationContext);
  if (!context) {
    throw new Error('useNotification must be used within NotificationProvider');
  }
  return context;
};

export const NotificationProvider: React.FC<{children: ReactNode}> = ({ children }) => {
  const [notifications, setNotifications] = useState<Notification[]>([]);

  const unreadCount = notifications.filter(n => !n.read).length;

  const load = async () => {
    try {
      const json = await invoke<string>('get_notifications');
      const notifs: Notification[] = JSON.parse(json);
      setNotifications(notifs);
    } catch (e) {
      console.error('Failed to load notifications', e);
    }
  };

  const add = (n: Notification) => {
    setNotifications(prev => [n, ...prev.filter(p => p.id !== n.id)]);
    if (n.kind === NotificationKind.Hil) {
      toast.custom(() => <NotificationToast notif={n} />, {
        id: n.id,
        duration: Infinity,
        position: 'bottom-right',
      });
    } else {
      toast(n.message, { duration: 8000, position: 'bottom-right' });
    }
  };

  const markRead = async (id: string) => {
    try {
      await invoke('ack_notif', { id });
      setNotifications(prev => prev.map(notif =>
        notif.id === id ? { ...notif, read: true } : notif
      ));
    } catch (e) {
      console.error('Failed to ack notif', e);
    }
  };

  useEffect(() => {
    load();
    let unlisten: () => void;
    listen<Notification>('notification', (event) => {
      add(event.payload);
    }).then((f) => {
      unlisten = f;
    }).catch(console.error);
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  return (
    <NotificationContext.Provider value={{ notifications, unreadCount, load, add, markRead }}>
      {children}
    </NotificationContext.Provider>
  );
};