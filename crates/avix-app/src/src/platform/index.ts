// Platform detection: Tauri sets __TAURI_INTERNALS__ on window.
// In a plain browser served by avix-web, we fall back to the HTTP/WS adapter.
import * as tauriPlatform from './tauri';
import * as webPlatform from './web';

const isTauri =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

const platform = isTauri ? tauriPlatform : webPlatform;

export const invoke = platform.invoke as <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export const listen = platform.listen as <T>(
  event: string,
  callback: (event: { payload: T }) => void,
) => Promise<() => void>;
