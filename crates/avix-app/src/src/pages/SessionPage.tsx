import React, { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '../platform';
import { useApp } from '../context/AppContext';
import { useNotification } from '../context/NotificationContext';
import { NotificationKind } from '../types/notifications';
import { Session, InvocationMessages, ConversationEntry } from '../types/agents';
import HilInlineCard from '../components/agents/HilInlineCard';
import { NewSessionModal } from '../components/NewSessionModal';

const RoleBadge: React.FC<{ role: string }> = ({ role }) => {
  const color =
    role === 'user' ? '#89b4fa' :
    role === 'assistant' ? '#a6e3a1' :
    '#f9e2af';
  return (
    <span
      style={{
        fontSize: 10,
        fontWeight: 700,
        color,
        textTransform: 'uppercase',
        letterSpacing: '0.06em',
        marginRight: 6,
        flexShrink: 0,
      }}
    >
      {role}
    </span>
  );
};

const MessageBubble: React.FC<{ entry: ConversationEntry; agentName: string }> = ({ entry }) => (
  <div
    style={{
      display: 'flex',
      flexDirection: 'column',
      padding: '8px 12px',
      borderRadius: 8,
      background: entry.role === 'user' ? 'rgba(137,180,250,0.06)' : 'rgba(166,227,161,0.04)',
      border: '1px solid rgba(255,255,255,0.04)',
      marginBottom: 6,
    }}
  >
    <div style={{ display: 'flex', alignItems: 'baseline', gap: 6, marginBottom: 4 }}>
      <RoleBadge role={entry.role} />
      {entry.thought && (
        <span style={{ fontSize: 11, color: '#585b70', fontStyle: 'italic' }}>
          thinking: {entry.thought}
        </span>
      )}
    </div>
    {entry.content && (
      <div style={{ fontSize: 13, color: '#cdd6f4', lineHeight: 1.6, whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
        {entry.content}
      </div>
    )}
    {entry.toolCalls && entry.toolCalls.length > 0 && (
      <div style={{ marginTop: 6 }}>
        {entry.toolCalls.map((tc, i) => (
          <div key={i} style={{ fontSize: 11, color: '#f9e2af', background: 'rgba(249,226,175,0.06)', borderRadius: 4, padding: '2px 8px', marginBottom: 2 }}>
            tool: {tc.name}
          </div>
        ))}
      </div>
    )}
    {entry.filesChanged && entry.filesChanged.length > 0 && (
      <div style={{ marginTop: 6 }}>
        {entry.filesChanged.map((f, i) => (
          <div key={i} style={{ fontSize: 11, color: '#cba6f7', background: 'rgba(203,166,247,0.06)', borderRadius: 4, padding: '2px 8px', marginBottom: 2 }}>
            changed: {f.path}
          </div>
        ))}
      </div>
    )}
  </div>
);

const InvocationBlock: React.FC<{ block: InvocationMessages }> = ({ block }) => (
  <div style={{ marginBottom: 16 }}>
    <div
      style={{
        fontSize: 10,
        fontWeight: 700,
        color: '#585b70',
        letterSpacing: '0.08em',
        textTransform: 'uppercase',
        marginBottom: 6,
        paddingLeft: 4,
      }}
    >
      {block.agentName} — {block.status}
    </div>
    {block.entries.map((entry, i) => (
      <MessageBubble key={i} entry={entry} agentName={block.agentName} />
    ))}
  </div>
);

const SessionPage: React.FC = () => {
  const { selectedSessionId, sessions, streamingOutputs, refreshSessions, conversationVersion, liveToolCalls } = useApp();
  const { notifications } = useNotification();

  const [session, setSession] = useState<Session | null>(null);
  const [invocationMessages, setInvocationMessages] = useState<InvocationMessages[]>([]);
  const [loadingMessages, setLoadingMessages] = useState(false);
  const [inputText, setInputText] = useState('');
  const [sending, setSending] = useState(false);
  const [newSessionOpen, setNewSessionOpen] = useState(false);
  const [railOpen, setRailOpen] = useState(false);
  const threadRef = useRef<HTMLDivElement>(null);

  const loadSession = useCallback(async () => {
    if (!selectedSessionId) return;
    try {
      const jsonStr = await invoke<string>('get_session', { session_id: selectedSessionId });
      if (jsonStr) {
        setSession(JSON.parse(jsonStr) as Session);
      }
    } catch {
      // session not found
    }
  }, [selectedSessionId]);

  const loadMessages = useCallback(async () => {
    if (!selectedSessionId) return;
    setLoadingMessages(true);
    try {
      const jsonStr = await invoke<string>('get_session_messages', { session_id: selectedSessionId });
      const parsed = JSON.parse(jsonStr) as InvocationMessages[];
      setInvocationMessages(parsed);
    } catch {
      setInvocationMessages([]);
    } finally {
      setLoadingMessages(false);
    }
  }, [selectedSessionId]);

  useEffect(() => {
    setSession(null);
    setInvocationMessages([]);
    setInputText('');
    loadSession();
    loadMessages();
  }, [selectedSessionId, loadSession, loadMessages]);

  // Reload conversation whenever an agent in this session exits
  useEffect(() => {
    if (conversationVersion > 0) {
      loadMessages();
    }
  }, [conversationVersion, loadMessages]);

  // Also sync session from context sessions list for live status updates
  useEffect(() => {
    if (!selectedSessionId) return;
    const found = sessions.find((s) => s.id === selectedSessionId);
    if (found) setSession(found);
  }, [sessions, selectedSessionId]);

  // Scroll to bottom when new content arrives
  useEffect(() => {
    if (threadRef.current) {
      threadRef.current.scrollTop = threadRef.current.scrollHeight;
    }
  }, [invocationMessages, streamingOutputs]);

  // Pending HIL for this session
  const sessionPids = session?.pids ?? [];
  const pendingHil = notifications.find(
    (n) =>
      n.kind === NotificationKind.Hil &&
      sessionPids.includes(n.agent_pid ?? 0) &&
      !n.hil?.outcome &&
      !n.resolved_at
  );

  const activePid = session?.pids[0] ?? 0;
  const liveText = activePid ? streamingOutputs[activePid] : undefined;

  const handleSend = async () => {
    if (!session || !inputText.trim() || sending) return;
    setSending(true);
    try {
      if (session.status === 'idle') {
        await invoke('resume_session', { session_id: session.id, input: inputText.trim() });
        await refreshSessions();
      } else if (session.status === 'running') {
        await invoke('pipe_text', { pid: activePid, text: inputText.trim() });
      }
      setInputText('');
    } catch {
      // ignore
    } finally {
      setSending(false);
    }
  };

  if (!selectedSessionId) {
    return (
      <div style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', color: '#585b70' }}>
        Select a session from the sidebar
      </div>
    );
  }

  const isTerminal = session && (session.status === 'completed' || session.status === 'failed' || session.status === 'archived');
  const showRail = (session?.participants?.length ?? 0) > 1;

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', height: '100%', overflow: 'hidden', background: '#1e1e2e' }}>
      {/* Header */}
      <div
        style={{
          padding: '12px 20px',
          borderBottom: '1px solid #313244',
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          flexShrink: 0,
        }}
      >
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              color: '#cdd6f4',
              fontWeight: 600,
              fontSize: 14,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {session?.title || session?.goal || 'Session'}
          </div>
          {session?.goal && session?.title && (
            <div style={{ color: '#6c7086', fontSize: 11, marginTop: 2, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {session.goal}
            </div>
          )}
        </div>
        {session && (
          <span
            style={{
              fontSize: 11,
              fontWeight: 700,
              padding: '2px 8px',
              borderRadius: 10,
              background:
                session.status === 'running' ? 'rgba(166,227,161,0.15)' :
                session.status === 'idle' ? 'rgba(137,180,250,0.15)' :
                session.status === 'paused' ? 'rgba(249,226,175,0.15)' :
                'rgba(108,112,134,0.15)',
              color:
                session.status === 'running' ? '#a6e3a1' :
                session.status === 'idle' ? '#89b4fa' :
                session.status === 'paused' ? '#f9e2af' :
                '#6c7086',
            }}
          >
            {session.status}
          </span>
        )}
        {session && session.tokensTotal > 0 && (
          <span style={{ fontSize: 11, color: '#585b70' }}>
            {session.tokensTotal.toLocaleString()} tokens
          </span>
        )}
        {showRail && (
          <button
            onClick={() => setRailOpen((v) => !v)}
            style={{
              background: 'none',
              border: '1px solid #313244',
              borderRadius: 6,
              color: '#6c7086',
              cursor: 'pointer',
              fontSize: 11,
              padding: '3px 8px',
            }}
          >
            {session?.participants.length} agents {railOpen ? '▶' : '◀'}
          </button>
        )}
      </div>

      {/* Body: thread + optional rail */}
      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        {/* Message thread */}
        <div ref={threadRef} style={{ flex: 1, overflowY: 'auto', padding: '16px 20px' }}>
          {loadingMessages && (
            <div style={{ color: '#585b70', fontSize: 13, textAlign: 'center', padding: 24 }}>
              Loading…
            </div>
          )}
          {invocationMessages.map((block) => (
            <InvocationBlock key={block.invocationId} block={block} />
          ))}
          {/* Live tool activity feed */}
          {activePid > 0 && (liveToolCalls[activePid]?.length ?? 0) > 0 && (
            <div style={{ marginBottom: 10 }}>
              {liveToolCalls[activePid].slice(-10).map((tc, i) => (
                <div
                  key={i}
                  style={{
                    fontSize: 11,
                    color: tc.isResult ? '#a6e3a1' : '#f9e2af',
                    background: tc.isResult ? 'rgba(166,227,161,0.04)' : 'rgba(249,226,175,0.04)',
                    borderRadius: 4,
                    padding: '2px 8px',
                    marginBottom: 2,
                    fontFamily: 'monospace',
                  }}
                >
                  {tc.isResult ? '← ' : '→ '}{tc.tool}
                  {tc.isResult && tc.result && (
                    <span style={{ color: '#6c7086', marginLeft: 6 }}>
                      {tc.result.slice(0, 80)}{tc.result.length > 80 ? '…' : ''}
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}

          {/* Live streaming block */}
          {liveText && (
            <div style={{ marginBottom: 16 }}>
              <div style={{ fontSize: 10, fontWeight: 700, color: '#585b70', letterSpacing: '0.08em', textTransform: 'uppercase', marginBottom: 6, paddingLeft: 4 }}>
                {session?.primaryAgent ?? 'agent'} — streaming
              </div>
              <div
                style={{
                  padding: '8px 12px',
                  borderRadius: 8,
                  background: 'rgba(166,227,161,0.04)',
                  border: '1px solid rgba(166,227,161,0.1)',
                  fontSize: 13,
                  color: '#cdd6f4',
                  lineHeight: 1.6,
                  whiteSpace: 'pre-wrap',
                  wordBreak: 'break-word',
                }}
              >
                {liveText}
                <span style={{ animation: 'blink 1s step-end infinite', color: '#a6e3a1' }}>▌</span>
              </div>
            </div>
          )}
        </div>

        {/* Multi-agent rail */}
        {showRail && railOpen && session && (
          <div
            style={{
              width: 220,
              borderLeft: '1px solid #313244',
              padding: '16px 14px',
              overflowY: 'auto',
              flexShrink: 0,
            }}
          >
            <div style={{ fontSize: 10, fontWeight: 700, color: '#585b70', letterSpacing: '0.08em', textTransform: 'uppercase', marginBottom: 10 }}>
              Participants
            </div>
            {session.participants.map((p) => {
              const invBlock = invocationMessages.find((b) => b.agentName === p);
              return (
                <div key={p} style={{ marginBottom: 10 }}>
                  <div style={{ color: '#cdd6f4', fontSize: 12, fontWeight: 500 }}>{p}</div>
                  {invBlock && (
                    <div style={{ color: '#6c7086', fontSize: 11 }}>{invBlock.status}</div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* Input bar */}
      <div style={{ borderTop: '1px solid #313244', padding: '12px 16px', flexShrink: 0 }}>
        {pendingHil ? (
          <HilInlineCard notif={pendingHil} />
        ) : isTerminal ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
            <span style={{ color: '#585b70', fontSize: 13 }}>
              Session {session?.status} — read-only
            </span>
            <button
              onClick={() => setNewSessionOpen(true)}
              style={{
                background: '#313244',
                border: 'none',
                borderRadius: 6,
                color: '#cdd6f4',
                cursor: 'pointer',
                fontSize: 12,
                padding: '5px 14px',
              }}
            >
              Spawn new session
            </button>
          </div>
        ) : (
          <div style={{ display: 'flex', gap: 8 }}>
            <input
              style={{
                flex: 1,
                background: '#181825',
                border: '1px solid #313244',
                borderRadius: 8,
                color: '#cdd6f4',
                fontSize: 13,
                padding: '8px 12px',
                outline: 'none',
              }}
              placeholder={
                session?.status === 'running'
                  ? 'Send input to running agent…'
                  : 'Resume session with a new goal…'
              }
              value={inputText}
              onChange={(e) => setInputText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault();
                  handleSend();
                }
              }}
              disabled={sending}
            />
            <button
              onClick={handleSend}
              disabled={!inputText.trim() || sending}
              style={{
                background: !inputText.trim() || sending ? '#313244' : '#89b4fa',
                border: 'none',
                borderRadius: 8,
                color: !inputText.trim() || sending ? '#6c7086' : '#1e1e2e',
                cursor: !inputText.trim() || sending ? 'not-allowed' : 'pointer',
                fontSize: 13,
                fontWeight: 600,
                padding: '8px 18px',
              }}
            >
              {sending ? '…' : 'Send'}
            </button>
          </div>
        )}
      </div>

      <NewSessionModal isOpen={newSessionOpen} onClose={() => setNewSessionOpen(false)} />
    </div>
  );
};

export default SessionPage;
