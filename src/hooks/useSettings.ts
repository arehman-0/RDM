import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";

/** Options applied to the next download (and some live to running ones). */
export interface Settings {
  connections: number;
  setConnections: (n: number) => void;
  /** Speed cap in MB/s; 0 = unlimited. */
  speedLimitMB: number;
  setSpeedLimitMB: (n: number) => void;
  /** Max simultaneous downloads (queue concurrency cap). */
  maxConcurrent: number;
  setMaxConcurrent: (n: number) => void;
  saveDir: string;
  setSaveDir: (s: string) => void;
  verify: boolean;
  setVerify: (b: boolean) => void;
  expectedChecksum: string;
  setExpectedChecksum: (s: string) => void;
  showAdvanced: boolean;
  setShowAdvanced: (b: boolean) => void;
  /** Custom headers, one "Key: Value" per line. */
  headersText: string;
  setHeadersText: (s: string) => void;
  /** Mirror URLs, one per line. */
  mirrorsText: string;
  setMirrorsText: (s: string) => void;
}

export function useSettings(): Settings {
  const [connections, setConnections] = useState(8);
  const [speedLimitMB, setSpeedLimitMB] = useState(0);
  const [maxConcurrent, setMaxConcurrent] = useState(3);
  const [saveDir, setSaveDir] = useState("");
  const [verify, setVerify] = useState(false);
  const [expectedChecksum, setExpectedChecksum] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [headersText, setHeadersText] = useState("");
  const [mirrorsText, setMirrorsText] = useState("");

  // Initial values from the backend.
  useEffect(() => {
    invoke<number>("suggested_connections")
      .then(setConnections)
      .catch(() => {});
    invoke<string>("default_save_dir")
      .then(setSaveDir)
      .catch(() => {});
  }, []);

  return {
    connections,
    setConnections,
    speedLimitMB,
    setSpeedLimitMB,
    maxConcurrent,
    setMaxConcurrent,
    saveDir,
    setSaveDir,
    verify,
    setVerify,
    expectedChecksum,
    setExpectedChecksum,
    showAdvanced,
    setShowAdvanced,
    headersText,
    setHeadersText,
    mirrorsText,
    setMirrorsText,
  };
}
