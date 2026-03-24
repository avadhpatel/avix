# GUI App Gap 5: ContentRenderer Component

## Spec Reference
docs/spec/gui-cli-via-atp.md section s4 Frontend:
* ContentRenderer: images (<img>), PDF.js, Recharts charts, iframe (safe src).
* Markdown-ish output rendering.
* Fallback for rich content: ASCII tables/charts → scrollable text.

## Goals
* Render agent.output events as rich content in panels.
* Support image/PDF/chart/iframe/markdown/text.
* Graceful fallbacks for unsupported formats.
* Scrollable, responsive in GoldenLayout panels.

## Dependencies
* npm: react-pdf (PDF.js), recharts, marked (markdown), react-markdown.
* Backend events: agent.output {mime: 'image/png', data: base64 | url, content: string}.

## Files to Create/Edit
* src/components/ContentRenderer.tsx
* src/components/MarkdownRenderer.tsx (optional helper)
* package.json: deps above

## Detailed Tasks
1. ContentRenderer props: {content: any, mime?: string}
```
tsx
const ContentRenderer: React.FC<{content: any, mime?: string}> = ({content, mime}) => {
  if (mime?.startsWith('image/')) {
    return <img src={`data:${mime};base64,${content}`} alt=\"output\" />;
  } else if (mime === 'application/pdf') {
    return <PDFViewer file={{data: atob(content)}} />;
  } else if (mime === 'application/chart+json') {
    return <RechartsChart data={JSON.parse(content)} />;
  } else if (mime?.includes('html') || content.includes('<')) {
    return <iframe srcDoc={content} sandbox=\"allow-same-origin\" />;
  } else {
    return <Markdown>{content}</Markdown>;  // or plain text
  }
};
```
* Handle streaming: append to content buffer.

2. Fallbacks: ASCII art/tables → <pre>{content}</pre>, scrollable.

3. PanelContent.tsx integrate: useEventListener('agent-event', filter by panelId) → update content.

4. UX: Auto-scroll to bottom for streaming output, zoom/pan for images/PDF.

5. Tests: snapshots for each render type (mock data).

## Verify
* Spawn agent → output renders: text/markdown ok, image/PDF/chart display, fallback text.
* Responsive in resized panels.

Est: 2h