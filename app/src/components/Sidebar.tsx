export type Page = 'devices' | 'studio' | 'profiles';

const NAV: Array<{ id: Page; icon: string; label: string }> = [
  { id: 'devices', icon: '⌨', label: 'Devices' },
  { id: 'studio', icon: '✦', label: 'Studio' },
  { id: 'profiles', icon: '★', label: 'Profiles' },
];

/** Vertical nav rail: icons + labels; collapses to icons under 900px. */
export function Sidebar({ page, onNav }: { page: Page; onNav(p: Page): void }) {
  return (
    <nav className="sidebar" aria-label="Sections">
      {NAV.map((n) => (
        <button
          key={n.id}
          className={page === n.id ? 'nav-item on' : 'nav-item'}
          onClick={() => onNav(n.id)}
          aria-current={page === n.id ? 'page' : undefined}
          title={n.label}
        >
          <span className="nav-icon" aria-hidden>
            {n.icon}
          </span>
          <span className="nav-label">{n.label}</span>
        </button>
      ))}
    </nav>
  );
}
