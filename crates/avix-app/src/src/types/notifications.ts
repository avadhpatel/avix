export enum NotificationKind {
  Hil = "Hil",
  AgentExit = "AgentExit",
  SysAlert = "SysAlert",
}

export enum HilOutcome {
  Approved = "Approved",
  Denied = "Denied",
  Timeout = "Timeout",
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