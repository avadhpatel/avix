import React, { useState, useRef, KeyboardEvent } from 'react';
import { invoke } from '../../platform';
import toast from 'react-hot-toast';

interface Props {
  agentPid: number;
  disabled?: boolean;
}

const AgentInputBar: React.FC<Props> = ({ agentPid, disabled }) => {
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const send = async () => {
    const trimmed = text.trim();
    if (!trimmed || sending) return;

    setSending(true);
    try {
      await invoke('pipe_text', { pid: agentPid, text: trimmed });
      setText('');
      if (textareaRef.current) {
        textareaRef.current.style.height = 'auto';
      }
    } catch (e) {
      toast.error(`Failed to send message: ${e}`);
    } finally {
      setSending(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      send();
    }
  };

  const handleInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setText(e.target.value);
    // Auto-resize textarea
    const ta = e.target;
    ta.style.height = 'auto';
    ta.style.height = Math.min(ta.scrollHeight, 120) + 'px';
  };

  const canSend = text.trim().length > 0 && !sending && !disabled;

  return (
    <div
      style={{
        borderTop: '1px solid #1e293b',
        padding: '12px 16px',
        display: 'flex',
        alignItems: 'flex-end',
        gap: '10px',
        backgroundColor: '#0f172a',
      }}
    >
      <textarea
        ref={textareaRef}
        value={text}
        onChange={handleInput}
        onKeyDown={handleKeyDown}
        disabled={sending || disabled}
        placeholder={disabled ? 'Agent is not running' : 'Send a follow-up message… (⌘↵ to send)'}
        rows={1}
        style={{
          flex: 1,
          padding: '9px 12px',
          backgroundColor: '#1e293b',
          border: '1px solid #334155',
          borderRadius: '8px',
          color: '#f8fafc',
          fontSize: '13px',
          lineHeight: '1.5',
          resize: 'none',
          outline: 'none',
          fontFamily: 'inherit',
          minHeight: '38px',
          maxHeight: '120px',
          overflow: 'auto',
          transition: 'border-color 0.15s',
          opacity: disabled ? 0.5 : 1,
        }}
        onFocus={(e) => { e.currentTarget.style.borderColor = '#3b82f6'; }}
        onBlur={(e) => { e.currentTarget.style.borderColor = '#334155'; }}
      />
      <button
        onClick={send}
        disabled={!canSend}
        title="Send (⌘↵)"
        style={{
          flexShrink: 0,
          width: '38px',
          height: '38px',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          backgroundColor: canSend ? '#3b82f6' : '#1e293b',
          border: 'none',
          borderRadius: '8px',
          cursor: canSend ? 'pointer' : 'not-allowed',
          transition: 'background 0.15s',
          color: canSend ? 'white' : '#334155',
        }}
      >
        {sending ? (
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" style={{ animation: 'spin 1s linear infinite' }}>
            <path d="M21 12a9 9 0 1 1-6.219-8.56" />
          </svg>
        ) : (
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
            <line x1="22" y1="2" x2="11" y2="13" />
            <polygon points="22 2 15 22 11 13 2 9 22 2" />
          </svg>
        )}
      </button>
    </div>
  );
};

export default AgentInputBar;
