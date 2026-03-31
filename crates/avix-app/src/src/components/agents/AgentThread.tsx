import React, { useEffect, useRef } from 'react';
import { OutputItem } from '../../types/agents';
import AgentMessageBubble from './AgentMessageBubble';

interface Props {
  outputs: OutputItem[];
  agentPid: number;
  agentName: string;
  streamingText?: string;
}

const AgentThread: React.FC<Props> = ({ outputs, agentPid, agentName, streamingText }) => {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [outputs.length, streamingText]);

  return (
    <div
      style={{
        flex: 1,
        overflowY: 'auto',
        padding: '16px',
        display: 'flex',
        flexDirection: 'column',
        gap: '2px',
      }}
    >
      {outputs.length === 0 && !streamingText ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            flex: 1,
            gap: '8px',
            color: '#334155',
            paddingTop: '60px',
          }}
        >
          <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="10" />
            <path d="M12 6v6l4 2" />
          </svg>
          <p style={{ fontSize: '13px', margin: 0 }}>
            Agent #{agentPid} ({agentName}) — waiting for output…
          </p>
        </div>
      ) : (
        <>
          {outputs.map((item, i) => (
            <AgentMessageBubble key={i} item={item} index={i} />
          ))}
          {streamingText && (
            <div
              style={{
                padding: '10px 14px',
                borderRadius: '8px',
                background: 'rgba(99,102,241,0.06)',
                border: '1px solid rgba(99,102,241,0.15)',
              }}
            >
              <pre style={{ whiteSpace: 'pre-wrap', fontSize: '14px', margin: 0, color: '#e2e8f0' }}>
                {streamingText}
                <span
                  style={{
                    display: 'inline-block',
                    width: '2px',
                    height: '1em',
                    background: '#818cf8',
                    marginLeft: '2px',
                    verticalAlign: 'text-bottom',
                    animation: 'blink 1s step-end infinite',
                  }}
                />
              </pre>
            </div>
          )}
        </>
      )}
      <div ref={bottomRef} />
    </div>
  );
};

export default AgentThread;
