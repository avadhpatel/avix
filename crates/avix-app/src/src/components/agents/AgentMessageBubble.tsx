import React from 'react';
import { OutputItem } from '../../types/agents';
import ContentRenderer from '../ContentRenderer';

interface Props {
  item: OutputItem;
  index: number;
}

const AgentMessageBubble: React.FC<Props> = ({ item, index }) => {
  return (
    <div
      style={{
        marginBottom: '2px',
        padding: '10px 16px',
        borderRadius: '8px',
        backgroundColor: index % 2 === 0 ? 'transparent' : 'rgba(255,255,255,0.02)',
        borderLeft: '2px solid #1e3a5f',
        color: '#e2e8f0',
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace',
        fontSize: '13px',
        lineHeight: '1.65',
      }}
    >
      <ContentRenderer content={item.content} mime={item.mime} />
    </div>
  );
};

export default AgentMessageBubble;
