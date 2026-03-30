import React, { useState } from 'react';
import { invoke } from '../platform';
import toast from 'react-hot-toast';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  // pid as string (backend returns numeric pid as a string)
  onAgentAdded: (pidStr: string) => void;
}

export const AddAgentModal: React.FC<Props> = ({ isOpen, onClose, onAgentAdded }) => {
  const [name, setName] = useState('');
  const [desc, setDesc] = useState('');
  const [submitting, setSubmitting] = useState(false);

  if (!isOpen) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;

    setSubmitting(true);
    try {
      // spawn_agent returns the numeric PID as a plain string
      const pidStr: string = await invoke('spawn_agent', {
        name: name.trim(),
        description: desc.trim(),
      });
      toast.success(`Agent '${name}' spawned (PID: ${pidStr})`);
      onAgentAdded(pidStr);
      setName('');
      setDesc('');
      onClose();
    } catch (error: any) {
      toast.error(`Spawn failed: ${error}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div style={{
      position: 'fixed',
      inset: 0,
      backgroundColor: 'rgba(0, 0, 0, 0.5)',
      zIndex: 2000,
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      padding: '2rem',
    }}>
      <div style={{
        backgroundColor: '#0d1829',
        border: '1px solid #1e293b',
        padding: '1.75rem',
        borderRadius: '12px',
        maxWidth: '480px',
        width: '100%',
        maxHeight: '90vh',
        overflow: 'auto',
        boxShadow: '0 25px 60px rgba(0,0,0,0.5)',
      }}>
        <h2 style={{ marginBottom: '1.25rem', color: '#f8fafc', fontSize: '16px', fontWeight: 700 }}>
          Add New Agent
        </h2>
        <form onSubmit={handleSubmit}>
          <div style={{ marginBottom: '1rem' }}>
            <label style={{ display: 'block', color: '#94a3b8', fontSize: '12px', fontWeight: 600, marginBottom: '6px', letterSpacing: '0.04em' }}>
              NAME *
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
              placeholder="e.g. data-analyst"
              style={{
                width: '100%',
                padding: '8px 12px',
                backgroundColor: '#1e293b',
                border: '1px solid #334155',
                borderRadius: '6px',
                color: '#f8fafc',
                fontSize: '13px',
                outline: 'none',
                boxSizing: 'border-box',
              }}
              onFocus={(e) => { e.currentTarget.style.borderColor = '#3b82f6'; }}
              onBlur={(e) => { e.currentTarget.style.borderColor = '#334155'; }}
            />
          </div>
          <div style={{ marginBottom: '1.25rem' }}>
            <label style={{ display: 'block', color: '#94a3b8', fontSize: '12px', fontWeight: 600, marginBottom: '6px', letterSpacing: '0.04em' }}>
              GOAL / DESCRIPTION
            </label>
            <textarea
              value={desc}
              onChange={(e) => setDesc(e.target.value)}
              rows={3}
              placeholder="Describe what this agent should do…"
              style={{
                width: '100%',
                padding: '8px 12px',
                backgroundColor: '#1e293b',
                border: '1px solid #334155',
                borderRadius: '6px',
                color: '#f8fafc',
                fontSize: '13px',
                outline: 'none',
                resize: 'vertical',
                fontFamily: 'inherit',
                boxSizing: 'border-box',
              }}
              onFocus={(e) => { e.currentTarget.style.borderColor = '#3b82f6'; }}
              onBlur={(e) => { e.currentTarget.style.borderColor = '#334155'; }}
            />
          </div>
          <div style={{ display: 'flex', gap: '8px', justifyContent: 'flex-end' }}>
            <button
              type="button"
              onClick={onClose}
              disabled={submitting}
              style={{
                padding: '7px 16px',
                border: '1px solid #334155',
                background: 'none',
                borderRadius: '6px',
                color: '#94a3b8',
                fontSize: '13px',
                cursor: 'pointer',
              }}
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={submitting || !name.trim()}
              style={{
                padding: '7px 16px',
                backgroundColor: submitting || !name.trim() ? '#1e293b' : '#3b82f6',
                color: submitting || !name.trim() ? '#475569' : 'white',
                border: 'none',
                borderRadius: '6px',
                fontSize: '13px',
                fontWeight: 600,
                cursor: submitting || !name.trim() ? 'not-allowed' : 'pointer',
              }}
            >
              {submitting ? 'Creating…' : 'Create Agent'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
