export enum NotificationKind {
  Hil = &quot;Hil&quot;,
  AgentExit = &quot;AgentExit&quot;,
  SysAlert = &quot;SysAlert&quot;,
}

export enum HilOutcome {
  Approved = &quot;Approved&quot;,
  Denied = &quot;Denied&quot;,
  Timeout = &quot;Timeout&quot;,
}

export interface HilState {
  hil_id: string;
  pid: number;
  approval_token: string;
  prompt: string;
  timeout_secs: number;
  outcome?: HilOutcome;
}

export interface Notification {
  id: string;
  kind: NotificationKind;
  agent_pid?: number;
  session_id?: string;
  message: string;
  hil?: HilState;
  created_at: string;
  resolved_at?: string;
  read: boolean;
}