export type AgentStatus = 'running' | 'paused' | 'stopped' | 'crashed';
export type Page = 'agent' | 'services' | 'tools' | 'catalog' | 'history';

export interface InstalledAgent {
  name: string;
  version: string;
  description: string;
  author: string;
  path: string;
  scope: 'system' | 'user';
}

export interface InvocationRecord {
  id: string;
  agentName: string;
  username: string;
  pid: number;
  goal: string;
  sessionId: string;
  spawnedAt: string;
  endedAt?: string;
  status: 'running' | 'completed' | 'failed' | 'killed';
  tokensConsumed: number;
  toolCallsTotal: number;
  exitReason?: string;
}

export interface Agent {
  pid: number;
  name: string;
  goal: string;
  status: AgentStatus;
}

export interface OutputItem {
  mime?: string;
  content: string;
}
