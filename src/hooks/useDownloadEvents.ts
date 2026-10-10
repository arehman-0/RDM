import { useEffect, useReducer, type Dispatch } from "react";
import { invoke } from "@tauri-apps/api/tauri";
import { cardsReducer, emptyCards, type CardsAction, type CardsState } from "../lib/cards";
import type {
  HistoryRemoved,
  JobRecord,
  ProgressPayload,
  QueueStatus,
  YoutubeStatus,
} from "../types";
import { useTauriEvent } from "./useTauriEvent";

/**
 * The dashboard's card state, as a passive mirror of the backend: seeded once
 * from `list_history`, then kept current purely by events.
 */
export function useDownloadEvents(): [CardsState, Dispatch<CardsAction>] {
  const [state, dispatch] = useReducer(cardsReducer, emptyCards);

  useTauriEvent<ProgressPayload>("download-progress", (payload) =>
    dispatch({ type: "progress", payload })
  );
  useTauriEvent<YoutubeStatus>("youtube-status", (payload) =>
    dispatch({ type: "youtube", payload })
  );
  useTauriEvent<QueueStatus>("queue-status", (payload) => dispatch({ type: "queue", payload }));
  useTauriEvent<HistoryRemoved>("history-removed", (payload) =>
    dispatch({ type: "removed", ids: payload.ids })
  );

  useEffect(() => {
    invoke<JobRecord[]>("list_history")
      .then((records) => dispatch({ type: "history", records }))
      .catch(() => {});
  }, []);

  return [state, dispatch];
}
