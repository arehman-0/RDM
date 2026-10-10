import type { ProgressPayload } from "../types";
import { streamLabel } from "../lib/cards";

/** Per-stream, per-thread segmentation bars (one block per engine job). */
export function SegmentBars({
  jobs,
  isYoutube,
}: {
  jobs: Record<string, ProgressPayload>;
  isYoutube: boolean;
}) {
  return (
    <>
      {Object.entries(jobs).map(([jobId, job]) => (
        <div className="stream-block" key={jobId}>
          {isYoutube && (
            <div className="stream-label">
              {streamLabel(jobId) ?? "stream"} · {job.activeThreads} active
            </div>
          )}
          <div className="segments" title="Dynamic file segments">
            {job.total > 0 &&
              job.threads.map((t) => {
                const left = (t.start / job.total) * 100;
                const widthTotal = ((t.end - t.start) / job.total) * 100;
                const widthDone = ((t.current - t.start) / job.total) * 100;
                return (
                  <div
                    key={t.id}
                    className="seg-track"
                    style={{ left: `${left}%`, width: `${widthTotal}%` }}
                  >
                    <div
                      className={`seg-fill ${t.failed ? "failed" : t.done ? "done" : ""}`}
                      style={{
                        width: `${widthTotal > 0 ? (widthDone / widthTotal) * 100 : 0}%`,
                      }}
                    />
                  </div>
                );
              })}
          </div>
        </div>
      ))}
    </>
  );
}
