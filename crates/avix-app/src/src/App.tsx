import React, { useEffect, useRef } from "react";
import { GoldenLayout } from "golden-layout";

const App: React.FC = () => {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (containerRef.current) {
      const layout = new GoldenLayout(containerRef.current);

      layout.registerComponent("welcome", () => <div>Welcome to Avix!</div>);

      layout.addItem({
        type: "component",
        componentType: "welcome",
        componentState: {},
      });

      layout.init();
    }
  }, []);

  return (
    <div ref={containerRef} style={{ width: "100vw", height: "100vh" }} />
  );
};

export default App;