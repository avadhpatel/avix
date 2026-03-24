# GUI App Gap 6: HIL + Notifications UI

## Spec Reference
docs/spec/gui-cli-via-atp.md sections:
* s4 GUI: HIL - per-panel orange banner + sticky react-hot-toast (Approve/Deny buttons) + notification center queue (multiples/escalations); Notifications - react-hot-toast bottom-right + Bell icon → popover/floating panel center (unified store).
* s6 Flows: HIL → notification::add_hil() → emit → GUI toast/banner; Notifications → store → toast + center.
* s7 Persistence: notifications.json loaded/saved.

## Goals
* Render HIL requests as panel banners + approve/deny toasts.
* Show notifications via toasts + central Bell popover list.
* Queue/handle multiples/escalations in notification center.
* Persist unread notifications across sessions.

## Dependencies
* npm: react-hot-toast.
* Backend: notification events, resolve_hil command, get_notifications.

## Files to Create/Edit
* src/components/HilBanner.tsx (per-panel)
* src/components/NotificationToast.tsx
* src/components/NotificationCenter.tsx (Bell popover)
* src/hooks/useNotifications.ts (listen events, store)
* src/App.tsx: integrate Bell icon, Toaster

## Detailed Tasks
1. useNotifications hook:
```
tsx
useEffect(() => {
  const unlisten = listen('notification', (event: Notification) => {
    toast.custom(<NotificationToast notif={event.payload} />);
    notifications.add(event.payload);
  });
  invoke('load_notifications').then(setNotifications);
  return unlisten;
}, []);
```
* Persist on change: debounce save.

2. HilBanner (panel prop agentId): filter HILs for agent, orange bar top: \"HIL: {prompt}\" [Approve] [Deny] → invoke('resolve_hil', id, true/false).

3. NotificationToast: sticky for HIL, transient for others; buttons for HIL.

4. NotificationCenter: Bell icon → popover list (title/body/timestamp), mark read, queue multiples.

5. App.tsx: <Toaster position=\"bottom-right\" />, fixed Bell top-right.

6. UX: Escalations → multiple toasts/queue; Reboot → load pending HILs/notifs.

7. Tests: render toast/banner, button clicks emit resolve.

## Verify
* HIL event → orange banner in panel + sticky toast w/ buttons → resolves.
* SysAlert → transient toast + adds to center.
* Bell popover shows list, persists.

Est: 2h