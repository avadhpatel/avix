# GUI App Gap 4: Golden Layout Frontend Dashboard

## Spec Reference
docs/spec/gui-cli-via-atp.md sections:
* s4 Frontend: golden-layout (floating+docking), top-right +Add Agent → modal(Name+Description+Advanced collapsed), Panel titles: agent-name – task summary + subtitle(status+info), Multi panels/agent (agentId+sessionId), Layout persistence appDataDir/ui-layout.json (levels C/D/E: geometry, agent bind, task, focus, prefs), Initial layout rules (first-time/returning/reboot).
* s7 Persistence: ui-layout.json loaded on startup.

## Goals
* Build GoldenLayout-based multi-panel dashboard.
* Implement +Add Agent modal and panel headers.
* Support multiple panels per agent bound by agentId/sessionId.
* Persist/restore layout state via backend.

## Dependencies
* golden-layout ^1.2 (npm), React, TS.
* Backend commands: spawn_agent, save_layout, load_layout.

## Files to Create/Edit
* src/App.tsx (GoldenLayout root, +Add button, layout manager)
* src/components/AddAgentModal.tsx
* src/components/PanelHeader.tsx
* src/components/PanelContent.tsx (placeholder)
* src/hooks/useLayoutPersistence.ts
* package.json: golden-layout dep

## Detailed Tasks
1. src/App.tsx: GoldenLayout init, top-right fixed +Add button → modal.
```
tsx
import GoldenLayout from 'golden-layout';

const layout = new GoldenLayout({
  root: { type: 'row', content: [] },  // initial empty
});
layout.registerComponentFactoryFunction('panel', Panel);
layout.init();
```
* Fixed header/UI: Bell icon (notifs), +Add Agent button.

2. AddAgentModal: Name input, Description textarea, Advanced collapsed (model, tools?).
* On submit: invoke('spawn_agent', {name, desc}) → add panel for new agentId.

3. PanelHeader: `{agentName} – {taskSummary}` subtitle `{status} {info}`.
* Drag handle, close button, session selector (for multi-panel).

4. Multi-panel: configId = `${agentId}-${sessionId || 'default'}`, bind events by id.

5. Persistence hook:
```
tsx
useEffect(() => {
  invoke('load_layout').then(layoutConfig => {
    layout.loadLayout(JSON.parse(layoutConfig));
  });
  const onStateChange = () => invoke('save_layout', { layout: JSON.stringify(layout.saveLayout()) });
  layout.on('stateChanged', onStateChange);
}, []);
```
* Initial layout rules: first-time root row empty, returning load ui-layout.json, reboot restore + pull HILs.

6. Tests: React Testing Lib snapshots for modal/header, layout load/save.

## Verify
* Drag/drop/add/remove panels persist across reloads.
* +Add → spawn → new panel with header.
* Multi-panel per agent selectable.

Est: 3h