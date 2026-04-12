import React, {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useRef,
  ReactNode,
} from 'react';
import { invoke, listen } from '../platform';
import { Agent, AgentStatus, LiveToolEntry, OutputItem, Page, Session } from '../types/agents';

interface AppContextValue {
  agents: Agent[];
  selectedAgentPid: number | null;
  currentPage: Page;
  agentOutputs: Record<number, OutputItem[]>;
  streamingOutputs: Record<number, string>;
  setSelectedAgent: (pid: number) => void;
  setPage: (p: Page) => void;
  addAgent: (agent: Agent) => void;
  updateAgentStatus: (pid: number, status: AgentStatus) => void;
  removeAgent: (pid: number) => void;
  appendOutput: (pid: number, item: OutputItem) => void;
  // Session-centric additions
  identity: string;
  sessions: Session[];
  selectedSessionId: string | null;
  setSelectedSession: (id: string) => void;
  refreshSessions: () => Promise<void>;
  // Incremented whenever any agent exits — lets SessionPage reload conversation
  conversationVersion: number;
  // Live tool call activity: pid → last N entries
  liveToolCalls: Record<number, LiveToolEntry[]>;
}

const AppContext = createContext<AppContextValue | null>(null);

export const useApp = () => {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error('useApp must be used within AppProvider');
  return ctx;
};

interface RawAgent {
  pid?: number | string;
  name?: string;
  goal?: string;
  status?: string;
}

function parseAgent(raw: RawAgent): Agent {
  return {
    pid: typeof raw.pid === 'string' ? parseInt(raw.pid, 10) : (raw.pid ?? 0),
    name: raw.name ?? 'Unknown',
    goal: raw.goal ?? '',
    status: (raw.status as AgentStatus) ?? 'running',
  };
}

export const AppProvider: React.FC<{ children: ReactNode }> = ({ children }) => {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [selectedAgentPid, setSelectedAgentPid] = useState<number | null>(null);
  const [currentPage, setCurrentPage] = useState<Page>('agent');
  const [agentOutputs, setAgentOutputs] = useState<Record<number, OutputItem[]>>({});
  const [streamingOutputs, setStreamingOutputs] = useState<Record<number, string>>({});
  // Pending chunk buffers: turn_id -> accumulated text
  const pendingChunks = useRef<Map<string, { pid: number; text: string }>>(new Map());

  // Session-centric state
  const [identity, setIdentity] = useState<string>('');
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selectedSessionId, setSelectedSessionIdState] = useState<string | null>(null);
  const [conversationVersion, setConversationVersion] = useState(0);
  const [liveToolCalls, setLiveToolCalls] = useState<Record<number, LiveToolEntry[]>>({});

  const addAgent = useCallback((agent: Agent) => {
    setAgents((prev) => {
      if (prev.some((a) => a.pid === agent.pid)) return prev;
      return [...prev, agent];
    });
  }, []);

  const updateAgentStatus = useCallback((pid: number, status: AgentStatus) => {
    setAgents((prev) =>
      prev.map((a) => (a.pid === pid ? { ...a, status } : a))
    );
  }, []);

  const removeAgent = useCallback((pid: number) => {
    setAgents((prev) => prev.map((a) => (a.pid === pid ? { ...a, status: 'stopped' as AgentStatus } : a)));
  }, []);

  const appendOutput = useCallback((pid: number, item: OutputItem) => {
    setAgentOutputs((prev) => ({
      ...prev,
      [pid]: [...(prev[pid] ?? []), item],
    }));
  }, []);

  const setSelectedAgent = useCallback((pid: number) => {
    setSelectedAgentPid(pid);
    setCurrentPage('agent');
  }, []);

  const setPage = useCallback((p: Page) => {
    setCurrentPage(p);
    if (p !== 'agent') setSelectedAgentPid(null);
    if (p !== 'session') setSelectedSessionIdState(null);
  }, []);

  const setSelectedSession = useCallback((id: string) => {
    setSelectedSessionIdState(id);
    setCurrentPage('session');
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      const json = await invoke<string>('list_sessions', {});
      const raw = JSON.parse(json);
      setSessions(Array.isArray(raw) ? raw : []);
    } catch {
      // ignore errors
    }
  }, []);

  // Load identity + agents + sessions on mount
  useEffect(() => {
    // Load identity
    invoke<{ identity?: string; username?: string }>('auth_status')
      .then((status) => {
        const id = status?.identity ?? status?.username ?? '';
        setIdentity(id);
      })
      .catch(() => {});

    // Load agents
    invoke<string>('list_agents')
      .then((json) => {
        try {
          const raw: RawAgent[] = JSON.parse(json);
          setAgents(raw.map(parseAgent));
        } catch {
          // ignore parse errors
        }
      })
      .catch(() => {});

    // Load sessions
    refreshSessions();
  }, [refreshSessions]);

  // Listen for agent lifecycle events
  useEffect(() => {
    // Use an active flag to handle React StrictMode's double-invoke: cleanup may run
    // before async listen() promises resolve. If that happens, we cancel immediately
    // inside the .then() callback rather than leaving orphaned listeners.
    let active = true;
    const unlisteners: Array<() => void> = [];

    Promise.all([
      // agent.spawned → refresh agent list + sessions
      listen<unknown>('agent.spawned', () => {
        invoke<string>('list_agents')
          .then((json) => {
            try {
              const raw: RawAgent[] = JSON.parse(json);
              setAgents(raw.map(parseAgent));
            } catch {}
          })
          .catch(() => {});
        refreshSessions();
      }),

      // agent.exit → mark stopped + refresh sessions + trigger conversation reload
      listen<{ pid: number; exitCode?: number }>('agent.exit', (e) => {
        const { pid } = e.payload;
        removeAgent(pid);
        refreshSessions();
        setConversationVersion((v) => v + 1);
        // Clear live tool activity for this pid
        setLiveToolCalls((prev) => {
          const next = { ...prev };
          delete next[pid];
          return next;
        });
      }),

      // agent.status → update status
      listen<{ pid: number; status: string }>('agent.status', (e) => {
        updateAgentStatus(e.payload.pid, e.payload.status as AgentStatus);
      }),

      // agent.output → plain text output (non-streaming)
      listen<{ pid: number; text: string }>('agent.output', (e) => {
        const { pid, text } = e.payload;
        if (text) {
          appendOutput(pid, { content: text });
        }
      }),

      // agent.output.chunk → streaming output, accumulate until is_final
      listen<{ pid: number; turn_id: string; text_delta: string; seq: number; is_final: boolean }>(
        'agent.output.chunk',
        (e) => {
          const { pid, turn_id, text_delta, is_final } = e.payload;
          const existing = pendingChunks.current.get(turn_id);
          const accumulated = (existing?.text ?? '') + text_delta;

          if (is_final) {
            pendingChunks.current.delete(turn_id);
            // Clear streaming display for this pid
            setStreamingOutputs((prev) => {
              const next = { ...prev };
              delete next[pid];
              return next;
            });
            // Only commit non-empty output
            if (accumulated.length > 0) {
              appendOutput(pid, { content: accumulated });
            }
          } else {
            pendingChunks.current.set(turn_id, { pid, text: accumulated });
            // Update live streaming display
            setStreamingOutputs((prev) => ({ ...prev, [pid]: accumulated }));
          }
        }
      ),
      // agent.tool_call → accumulate live tool activity (last 20 per pid)
      listen<{ pid: number; callId: string; tool: string; args: unknown }>(
        'agent.tool_call',
        (e) => {
          const { pid, callId, tool, args } = e.payload;
          const entry: LiveToolEntry = { callId, tool, args, isResult: false, timestamp: Date.now() };
          setLiveToolCalls((prev) => {
            const existing = prev[pid] ?? [];
            const updated = [...existing, entry].slice(-20);
            return { ...prev, [pid]: updated };
          });
        }
      ),

      // agent.tool_result → annotate matching call with result
      listen<{ pid: number; callId: string; tool: string; result: string }>(
        'agent.tool_result',
        (e) => {
          const { pid, callId, tool, result } = e.payload;
          const entry: LiveToolEntry = { callId, tool, result, isResult: true, timestamp: Date.now() };
          setLiveToolCalls((prev) => {
            const existing = prev[pid] ?? [];
            const updated = [...existing, entry].slice(-20);
            return { ...prev, [pid]: updated };
          });
        }
      ),
    ]).then((fns) => {
      if (!active) {
        // Cleanup already ran before promises resolved — unlisten immediately
        fns.forEach((f) => f());
        return;
      }
      unlisteners.push(...fns);
    }).catch(() => {});

    return () => {
      active = false;
      unlisteners.forEach((f) => f());
    };
  }, [appendOutput, removeAgent, updateAgentStatus, refreshSessions]);

  return (
    <AppContext.Provider
      value={{
        agents,
        selectedAgentPid,
        currentPage,
        agentOutputs,
        streamingOutputs,
        setSelectedAgent,
        setPage,
        addAgent,
        updateAgentStatus,
        removeAgent,
        appendOutput,
        identity,
        sessions,
        selectedSessionId,
        setSelectedSession,
        refreshSessions,
        conversationVersion,
        liveToolCalls,
      }}
    >
      {children}
    </AppContext.Provider>
  );
};
