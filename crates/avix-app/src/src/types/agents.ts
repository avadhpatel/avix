export type AgentStatus = 'running' | 'paused' | 'stopped' | 'crashed';
export type Page = 'agent' | 'services' | 'tools' | 'catalog' | 'history' | 'session';

export type SessionStatus = 'running' | 'idle' | 'paused' | 'completed' | 'failed' | 'archived';

export interface Session {
  id: string;
  title: string;
  goal: string;
  status: SessionStatus;
  summary?: string;
  originAgent: string;
  primaryAgent: string;
  participants: string[];
  ownerPid: number;
  pids: number[];
  lastUpdated: string;
  spawnedAt: string;
  tokensTotal: number;
}

export interface ConversationEntry {
  role: 'user' | 'assistant' | 'tool' | 'system';
  content: string;
  toolCalls?: Array<{ id: string; name: string; args: unknown; result?: unknown }>;
  filesChanged?: Array<{ path: string; diff?: string; content?: string }>;
  thought?: string;
}

export interface InvocationMessages {
  invocationId: string;
  agentName: string;
  status: string;
  entries: ConversationEntry[];
}

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

export interface LiveToolEntry {
  callId: string;
  tool: string;
  args?: unknown;
  result?: string;
  isResult: boolean;
  timestamp: number;
}
