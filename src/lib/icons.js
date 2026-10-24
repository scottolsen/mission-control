/**
 * Inline SVG icons, 16×16, drawn with currentColor so they take the state or
 * text color they sit in. These are static strings owned by the app, so they
 * are the only markup main.js ever assigns through innerHTML.
 */

const svg = (body, extra = "") =>
  `<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"${extra}>${body}</svg>`;

export const ICONS = {
  alert: svg('<path d="M8 1.75 14.5 13.5h-13Z"/><path d="M8 6.25v3.25"/><circle cx="8" cy="11.6" r=".35" fill="currentColor"/>'),
  reply: svg('<path d="M2.5 3.5h11v7.5H7l-3 2.5V11H2.5Z"/><path d="M5.5 7.25h5"/>'),
  spinner: svg('<circle cx="8" cy="8" r="5.75" opacity=".28"/><path d="M8 2.25a5.75 5.75 0 0 1 5.75 5.75"/>', ' class="spin"'),
  loop: svg('<path d="M12.5 5.25A5 5 0 0 0 3.4 6.1"/><path d="M12.75 2.5v2.75H10"/><path d="M3.5 10.75a5 5 0 0 0 9.1-.85"/><path d="M3.25 13.5v-2.75H6"/>'),
  remote: svg('<rect x="4.75" y="1.75" width="6.5" height="12.5" rx="1.5"/><path d="M7.25 11.75h1.5"/>'),
  pause: svg('<circle cx="8" cy="8" r="5.75"/><path d="M6.4 6v4M9.6 6v4"/>'),
  moon: svg('<path d="M13.25 9.6A5.5 5.5 0 1 1 6.4 2.75a4.4 4.4 0 0 0 6.85 6.85Z"/>'),
  question: svg('<circle cx="8" cy="8" r="5.75"/><path d="M6.3 6.4a1.75 1.75 0 1 1 2.45 1.6c-.5.22-.75.6-.75 1.1v.15"/><circle cx="8" cy="11.4" r=".35" fill="currentColor"/>'),
  search: svg('<circle cx="7" cy="7" r="4.25"/><path d="m10.25 10.25 3.25 3.25"/>'),
  gear: svg('<circle cx="8" cy="8" r="2.1"/><path d="M8 1.75v1.5M8 12.75v1.5M14.25 8h-1.5M3.25 8h-1.5M12.42 3.58l-1.06 1.06M4.64 11.36l-1.06 1.06M12.42 12.42l-1.06-1.06M4.64 4.64 3.58 3.58"/>'),
  jump: svg('<path d="M9.5 2.5h4v4"/><path d="M13.5 2.5 7.25 8.75"/><path d="M12 9.5v3a1 1 0 0 1-1 1H3.5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h3"/>'),
  branch: svg('<circle cx="4.75" cy="3.75" r="1.5"/><circle cx="4.75" cy="12.25" r="1.5"/><circle cx="11.25" cy="5.25" r="1.5"/><path d="M4.75 5.25v5.5M11.25 6.75c0 2.5-2 3-6.5 4"/>'),
  folder: svg('<path d="M1.75 4.25a1 1 0 0 1 1-1h3.4l1.6 1.75h5.5a1 1 0 0 1 1 1v6.25a1 1 0 0 1-1 1H2.75a1 1 0 0 1-1-1Z"/>'),
  terminal: svg('<rect x="1.75" y="2.75" width="12.5" height="10.5" rx="1.5"/><path d="m4.5 6.25 2 1.75-2 1.75M8.25 10h3"/>'),
  send: svg('<path d="M2.25 8 13.75 2.5 11 13.5 7.75 9.25Z"/><path d="M7.75 9.25 13.75 2.5"/>'),
  close: svg('<path d="m4 4 8 8M12 4l-8 8"/>'),
  check: svg('<path d="m3.25 8.5 3 3 6.5-7"/>'),
  refresh: svg('<path d="M13.25 4.5v3h-3"/><path d="M12.9 7.5A5 5 0 1 0 12 10.6"/>'),
  status: svg('<path d="M2.5 4h11M2.5 8h11M2.5 12h7"/>'),
  layers: svg('<rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="M8 2.5v11M8 8h6"/>'),
  user: svg('<circle cx="8" cy="5.5" r="2.75"/><path d="M2.75 13.75a5.25 5.25 0 0 1 10.5 0"/>'),
  spark: svg('<path d="M8 1.75v3.5M8 10.75v3.5M1.75 8h3.5M10.75 8h3.5M3.6 3.6l2.1 2.1M10.3 10.3l2.1 2.1M3.6 12.4l2.1-2.1M10.3 5.7l2.1-2.1"/>'),
  tool: svg('<path d="M9.75 2.25a3.25 3.25 0 0 0-3.1 4.25L2.5 10.65a1.3 1.3 0 0 0 1.85 1.85l4.15-4.15a3.25 3.25 0 0 0 4.25-4.1l-2 2-1.75-.25-.25-1.75Z"/>'),
  plug: svg('<path d="M5.5 1.75v3M10.5 1.75v3M3.75 4.75h8.5v2.5a4.25 4.25 0 0 1-8.5 0Z"/><path d="M8 11.5v2.75"/>'),
  expand: svg('<path d="M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10"/>'),
  shrink: svg('<path d="M6 2.5V6H2.5M13.5 6H10V2.5M10 13.5V10h3.5M2.5 10H6v3.5"/>'),
};
