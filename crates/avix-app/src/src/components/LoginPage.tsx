import React, { useState } from 'react';
import { invoke } from '../platform';

interface Props {
  onLogin: () => void;
}

export const LoginPage: React.FC<Props> = ({ onLogin }) => {
  const [identity, setIdentity] = useState('admin');
  const [credential, setCredential] = useState('');
  const [save, setSave] = useState(true);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!credential.trim()) return;
    setLoading(true);
    setError('');
    try {
      await invoke('login', {
        identity: identity.trim(),
        credential: credential.trim(),
        save,
      });
      onLogin();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        width: '100vw',
        height: '100vh',
        backgroundColor: '#0f172a',
      }}
    >
      <div
        style={{
          backgroundColor: '#1e293b',
          border: '1px solid #334155',
          borderRadius: '12px',
          padding: '2.5rem',
          width: '100%',
          maxWidth: '420px',
        }}
      >
        <h1
          style={{
            color: '#f8fafc',
            fontSize: '1.5rem',
            fontWeight: 700,
            marginBottom: '0.25rem',
          }}
        >
          Avix
        </h1>
        <p style={{ color: '#94a3b8', marginBottom: '2rem', fontSize: '0.9rem' }}>
          Sign in to continue
        </p>

        <form onSubmit={handleSubmit}>
          <div style={{ marginBottom: '1rem' }}>
            <label
              style={{
                display: 'block',
                color: '#cbd5e1',
                fontSize: '0.85rem',
                marginBottom: '0.4rem',
              }}
            >
              Username
            </label>
            <input
              type="text"
              value={identity}
              onChange={(e) => setIdentity(e.target.value)}
              required
              style={{
                width: '100%',
                padding: '0.6rem 0.75rem',
                backgroundColor: '#0f172a',
                border: '1px solid #334155',
                borderRadius: '6px',
                color: '#f8fafc',
                fontSize: '0.95rem',
                boxSizing: 'border-box',
              }}
            />
          </div>

          <div style={{ marginBottom: '1rem' }}>
            <label
              style={{
                display: 'block',
                color: '#cbd5e1',
                fontSize: '0.85rem',
                marginBottom: '0.4rem',
              }}
            >
              API Key or Password
            </label>
            <input
              type="password"
              value={credential}
              onChange={(e) => setCredential(e.target.value)}
              required
              placeholder="Paste your API key or enter password"
              style={{
                width: '100%',
                padding: '0.6rem 0.75rem',
                backgroundColor: '#0f172a',
                border: '1px solid #334155',
                borderRadius: '6px',
                color: '#f8fafc',
                fontSize: '0.95rem',
                boxSizing: 'border-box',
              }}
            />
          </div>

          <label
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: '0.5rem',
              color: '#94a3b8',
              fontSize: '0.85rem',
              marginBottom: '1.5rem',
              cursor: 'pointer',
            }}
          >
            <input
              type="checkbox"
              checked={save}
              onChange={(e) => setSave(e.target.checked)}
            />
            Remember credentials (~/.config/avix/client.yaml)
          </label>

          {error && (
            <p
              style={{
                color: '#f87171',
                fontSize: '0.85rem',
                marginBottom: '1rem',
                padding: '0.6rem',
                backgroundColor: 'rgba(248,113,113,0.1)',
                borderRadius: '6px',
              }}
            >
              {error}
            </p>
          )}

          <button
            type="submit"
            disabled={loading || !credential.trim()}
            style={{
              width: '100%',
              padding: '0.7rem',
              backgroundColor:
                loading || !credential.trim() ? '#334155' : '#3b82f6',
              color: loading || !credential.trim() ? '#64748b' : 'white',
              border: 'none',
              borderRadius: '6px',
              fontSize: '0.95rem',
              fontWeight: 600,
              cursor: loading || !credential.trim() ? 'not-allowed' : 'pointer',
            }}
          >
            {loading ? 'Signing in…' : 'Sign in'}
          </button>
        </form>
      </div>
    </div>
  );
};
