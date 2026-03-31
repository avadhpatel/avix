import React, { useEffect, useState } from 'react';
import { Document, Page, pdfjs } from 'react-pdf';
import ReactMarkdown from 'react-markdown';
import { ResponsiveContainer, LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, Legend } from 'recharts';

type Props = {
  content: string;
  mime?: string;
};

const ContentRenderer: React.FC<Props> = ({ content, mime }) => {
  const [numPages, setNumPages] = useState(0);
  const [pageNumber] = useState(1);

  useEffect(() => {
    pdfjs.GlobalWorkerOptions.workerSrc = `//unpkg.com/pdfjs-dist@${pdfjs.version}/build/pdf.worker.min.js`;
  }, []);

  if (!content) return null;

  if (mime?.startsWith('image/')) {
    const src = `data:${mime};base64,${content}`;
    return <img src={src} alt="output" style={{ maxWidth: '100%', height: 'auto', cursor: 'zoom-in' }} />;
  }

  if (mime === 'application/pdf') {
    const file = { data: new Uint8Array(atob(content).split('').map((char) => char.charCodeAt(0))) };
    return (
      <>
        <Document file={file} onLoadSuccess={({ numPages }) => setNumPages(numPages)}>
          <Page pageNumber={pageNumber} />
        </Document>
        <p>
          Page {pageNumber} of {numPages}
        </p>
      </>
    );
  }

  if (mime === 'application/chart+json') {
    let data;
    try {
      data = JSON.parse(content);
    } catch {
      return <pre>Invalid JSON</pre>;
    }
    return (
      <ResponsiveContainer width="100%" height={300}>
        <LineChart data={data}>
          <CartesianGrid strokeDasharray="3 3" />
          <XAxis />
          <YAxis />
          <Tooltip />
          <Legend />
          <Line type="monotone" dataKey="uv" stroke="#8884d8" />
        </LineChart>
      </ResponsiveContainer>
    );
  }

  if (mime?.includes('text/html') || content.match(/<[!\\w][^>]*>/)) {
    return (
      <iframe
        srcDoc={content}
        sandbox="allow-same-origin allow-scripts"
        style={{ width: '100%', height: '400px', border: '1px solid #ccc' }}
      />
    );
  }

  if (mime === 'text/markdown') {
    return <ReactMarkdown>{content}</ReactMarkdown>;
  }

  return (
    <pre style={{ whiteSpace: 'pre-wrap', fontSize: '14px', margin: 0 }}>
      {content}
    </pre>
  );
};

export default ContentRenderer;
