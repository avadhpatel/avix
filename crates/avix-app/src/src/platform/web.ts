// Browser platform adapter: invoke → POST /api/invoke, listen → WebSocket /api/events

export async function invoke<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  const res = await fetch('/api/invoke', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ command, args }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}

// Shared WebSocket connection with auto-reconnect.
let _ws: WebSocket | null = null;
const _listeners = new Map<string, Set<(e: { payload: unknown }) => void>>();

function getWs(): WebSocket {
  if (
    _ws &&
    (_ws.readyState === WebSocket.OPEN ||
      _ws.readyState === WebSocket.CONNECTING)
  ) {
    return _ws;
  }
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  _ws = new WebSocket(`${protocol}//${location.host}/api/events`);
  _ws.addEventListener('message', (msg) => {
    try {
      const { event, data } = JSON.parse(msg.data as string);
      const handlers = _listeners.get(event as string);
      if (handlers) {
        handlers.forEach((h) => h({ payload: data }));
      }
    } catch {
      // malformed frame — ignore
    }
  });
  _ws.addEventListener('close', () => {
    _ws = null;
    setTimeout(getWs, 2000);
  });
  return _ws;
}

export function listen<T>(
  event: string,
  callback: (event: { payload: T }) => void,
): Promise<() => void> {
  getWs();
  if (!_listeners.has(event)) {
    _listeners.set(event, new Set());
  }
  const handlers = _listeners.get(event)!;
  const cb = callback as (e: { payload: unknown }) => void;
  handlers.add(cb);
  return Promise.resolve(() => {
    handlers.delete(cb);
  });
}
