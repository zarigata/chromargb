import React from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import { installChromaMock } from './mock';
import './styles.css';

// Mock daemon is injected at dev/build time when VITE_DEV_MOCK=1 (npm run dev).
if (import.meta.env.VITE_DEV_MOCK === '1' && !window.ChromaMock) installChromaMock();

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
