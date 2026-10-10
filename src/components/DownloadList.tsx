import type { DownloadCard as Card } from "../types";
import { DownloadCard, type CardCommand } from "./DownloadCard";
import { EmptyState } from "./EmptyState";

export function DownloadList({
  cards,
  onCommand,
  onClearCompleted,
}: {
  cards: Card[];
  onCommand: (card: Card, cmd: CardCommand) => void;
  onClearCompleted: () => void;
}) {
  return (
    <main className="dashboard">
      {cards.some((c) => c.status === "complete" || c.status === "canceled") && (
        <div className="dashboard-tools">
          <button className="linklike" onClick={onClearCompleted}>
            Clear completed
          </button>
        </div>
      )}
      {cards.length === 0 && <EmptyState />}
      {cards.map((card) => (
        <DownloadCard key={card.id} card={card} onCommand={onCommand} />
      ))}
    </main>
  );
}
