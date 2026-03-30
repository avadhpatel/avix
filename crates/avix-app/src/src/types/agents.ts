export type AgentStatus = 'running' | 'paused' | 'stopped' | 'crashed';
export type Page = 'agent' | 'services' | 'tools';

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
