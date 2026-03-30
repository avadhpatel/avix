import React, { useEffect, useRef } from 'react';
import { OutputItem } from '../../types/agents';
import AgentMessageBubble from './AgentMessageBubble';

interface Props {
  outputs: OutputItem[];
  agentPid: number;
  agentName: string;
}

const AgentThread: React.FC<Props> = ({ outputs, agentPid, agentName }) => {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [outputs.length]);

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
      {outputs.length === 0 ? (
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
        outputs.map((item, i) => (
          <AgentMessageBubble key={i} item={item} index={i} />
        ))
      )}
      <div ref={bottomRef} />
    </div>
  );
};

export default AgentThread;
