import { fmtSpeed } from "../lib/format";

export function Header({ speed, active }: { speed: number; active: number }) {
  return (
    <header className="topbar">
      <div className="brand">
        <span className="logo">⬇</span>
        <div>
          <h1>IDM Replica</h1>
          <p>Dynamic segmentation download engine</p>
        </div>
      </div>
      <div className="globalstats">
        <div>
          <span className="stat-num">{fmtSpeed(speed)}</span>
          <span className="stat-label">total speed</span>
        </div>
        <div>
          <span className="stat-num">{active}</span>
          <span className="stat-label">active</span>
        </div>
      </div>
    </header>
  );
}
