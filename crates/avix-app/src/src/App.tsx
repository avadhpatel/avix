import React, { useEffect, useState } from "react";
import { invoke } from "./platform";
import { Toaster } from "react-hot-toast";
import { NotificationProvider } from "./context/NotificationContext";
import { AppProvider, useApp } from "./context/AppContext";
import { useNotification } from "./context/NotificationContext";
import { LoginPage } from "./components/LoginPage";
import AppShell from "./components/layout/AppShell";
import Topbar from "./components/layout/Topbar";
import Sidebar from "./components/layout/Sidebar";
import NotificationCenter from "./components/notifications/NotificationCenter";
import AgentThreadPage from "./pages/AgentThreadPage";
import CatalogPage from "./pages/CatalogPage";
import HistoryPage from "./pages/HistoryPage";
import ServicesPage from "./pages/ServicesPage";
import ToolsPage from "./pages/ToolsPage";
import SessionPage from "./pages/SessionPage";

interface AuthStatus {
  authenticated: boolean;
  identity: string;
}

// Inner app that uses contexts
const AppInner: React.FC = () => {
  const { currentPage } = useApp();
  const { unreadCount } = useNotification();
  const [notifOpen, setNotifOpen] = useState(false);

  return (
    <AppShell
      topbar={
        <Topbar
          unreadCount={unreadCount}
          onNotifClick={() => setNotifOpen((o) => !o)}
        />
      }
      sidebar={<Sidebar />}
    >
      {currentPage === 'agent' && <AgentThreadPage />}
      {currentPage === 'session' && <SessionPage />}
      {currentPage === 'catalog' && <CatalogPage />}
      {currentPage === 'history' && <HistoryPage />}
      {currentPage === 'services' && <ServicesPage />}
      {currentPage === 'tools' && <ToolsPage />}

      {notifOpen && (
        <NotificationCenter onClose={() => setNotifOpen(false)} />
      )}
    </AppShell>
  );
};

const App: React.FC = () => {
  // null = still checking, false = needs login, true = logged in
  const [authenticated, setAuthenticated] = useState<boolean | null>(null);

  useEffect(() => {
    invoke<AuthStatus>("auth_status")
      .then((r) => setAuthenticated(r.authenticated))
      .catch(() => setAuthenticated(false));
  }, []);

  if (authenticated === null) {
    return (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          width: "100vw",
          height: "100vh",
          backgroundColor: "#0f172a",
          color: "#94a3b8",
          fontSize: "1rem",
        }}
      >
        Loading…
      </div>
    );
  }

  if (!authenticated) {
    return <LoginPage onLogin={() => setAuthenticated(true)} />;
  }

  return (
    <NotificationProvider>
      <AppProvider>
        <AppInner />
        <Toaster position="bottom-right" />
      </AppProvider>
    </NotificationProvider>
  );
};

export default App;
