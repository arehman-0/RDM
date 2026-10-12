import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { AddDownloadForm } from "./components/AddDownloadForm";
import { BridgeRequests } from "./components/BridgeRequests";
import { BrowserIntegrationPanel } from "./components/BrowserIntegrationPanel";
import { ComponentsSetup } from "./components/ComponentsSetup";
import { ComponentsPanel, PairingPrompt, StartupPanel } from "./components/ShellPanels";
import type { CardCommand } from "./components/DownloadCard";
import { DownloadList } from "./components/DownloadList";
import { Header } from "./components/Header";
import { LogPanel } from "./components/LogPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { useBridge } from "./hooks/useBridge";
import { useDownloadEvents } from "./hooks/useDownloadEvents";
import { useSettings } from "./hooks/useSettings";
import { useAppSettings, useComponents, usePairing } from "./hooks/useShell";
import { useToast } from "./hooks/useToast";
import { displayOrder, globalStats } from "./lib/cards";
import { needsSetup } from "./lib/components";
import type { DownloadCard } from "./types";

/** Backend command + arguments for each card action. */
function commandFor(id: string, cmd: CardCommand): [string, Record<string, unknown>] {
  switch (cmd.kind) {
    case "pause":
      return ["pause_download", { id }];
    case "resume":
      return ["resume_download", { id }];
    case "retry":
      return ["retry_download", { id }];
    case "cancel":
      return ["cancel_download", { id }];
    case "open":
      return ["open_download", { id, target: "file" }];
    case "reveal":
      return ["open_download", { id, target: "folder" }];
    case "remove":
      return ["remove_history_entry", { id }];
    case "move":
      return ["move_in_queue", { id, delta: cmd.delta }];
    case "priority":
      return ["set_queue_priority", { id, priority: cmd.priority }];
  }
}

export default function App() {
  const settings = useSettings();
  const [state, dispatch] = useDownloadEvents();
  const [toast, flash] = useToast();
  const bridge = useBridge();
  const components = useComponents();
  const [appSettings, saveAppSettings] = useAppSettings(flash);
  const pairing = usePairing();
  // The setup screen can be put aside; "Install missing components" brings it back.
  const [setupSkipped, setSetupSkipped] = useState(false);

  const cards = useMemo(() => displayOrder(state), [state]);
  const totals = useMemo(() => globalStats(cards), [cards]);

  const onCommand = (card: DownloadCard, cmd: CardCommand) => {
    const [name, args] = commandFor(card.id, cmd);
    invoke(name, args).catch((e) =>
      flash(cmd.kind === "open" || cmd.kind === "reveal" ? String(e) : `${name} failed: ${e}`)
    );
  };

  // Apply the speed cap live to every in-flight download as the slider moves.
  const applySpeedLimit = (mb: number) => {
    settings.setSpeedLimitMB(mb);
    const bytesPerSec = Math.round(mb * 1024 * 1024);
    for (const card of cards) {
      if (!["complete", "error", "canceled"].includes(card.status)) {
        invoke("set_speed_limit", { id: card.id, bytesPerSec }).catch(() => {});
      }
    }
  };

  const changeMaxConcurrent = (n: number) => {
    settings.setMaxConcurrent(n);
    invoke("set_max_concurrent", { max: n }).catch(() => {});
  };

  const clearCompleted = () => {
    invoke<string[]>("clear_completed").catch((e) => flash(`Clear failed: ${e}`));
  };

  return (
    <div className="app">
      <Header speed={totals.speed} active={totals.active} />
      <AddDownloadForm
        settings={settings}
        onStarted={(card) => dispatch({ type: "seed", card })}
        flash={flash}
      />
      <SettingsPanel
        settings={settings}
        onSpeedLimit={applySpeedLimit}
        onMaxConcurrent={changeMaxConcurrent}
      />
      <BrowserIntegrationPanel info={bridge.info} flash={flash} />
      <ComponentsPanel
        info={components.info}
        flash={flash}
        onSetup={() => {
          setSetupSkipped(false);
          invoke("install_components").catch(() => {});
        }}
      />
      <StartupPanel settings={appSettings} onChange={saveAppSettings} />
      <BridgeRequests pending={bridge.pending} flash={flash} />
      <DownloadList cards={cards} onCommand={onCommand} onClearCompleted={clearCompleted} />
      <LogPanel />
      {components.info && needsSetup(components.info, setupSkipped) && (
        <ComponentsSetup
          info={components.info}
          progress={components.progress}
          onSkip={() => setSetupSkipped(true)}
        />
      )}
      <PairingPrompt prompt={pairing} />
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
