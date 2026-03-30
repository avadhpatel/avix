import React, { ReactNode } from 'react';

interface Props {
  sidebar: ReactNode;
  topbar: ReactNode;
  children: ReactNode;
}

const AppShell: React.FC<Props> = ({ sidebar, topbar, children }) => {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateAreas: '"sidebar topbar" "sidebar main"',
        gridTemplateColumns: '240px 1fr',
        gridTemplateRows: '48px 1fr',
        height: '100vh',
        width: '100vw',
        overflow: 'hidden',
        backgroundColor: '#0f172a',
      }}
    >
      <div style={{ gridArea: 'topbar' }}>{topbar}</div>
      <div
        style={{
          gridArea: 'sidebar',
          borderRight: '1px solid #1e293b',
          overflow: 'hidden',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        {sidebar}
      </div>
      <div
        style={{
          gridArea: 'main',
          overflow: 'hidden',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        {children}
      </div>
    </div>
  );
};

export default AppShell;
