/** First-run / nothing-here screen: what to do and what's possible. */
export function EmptyState() {
  return (
    <div className="empty">
      <p>No downloads yet.</p>
      <small>Paste a URL above. YouTube links are auto-extracted and muxed.</small>
      <ul className="empty-tips">
        <li>
          <b>Files</b> download over many parallel connections and survive restarts — resume
          picks up where it stopped.
        </li>
        <li>
          <b>Start at</b> schedules a download for later; the queue starts it on time.
        </li>
        <li>
          <b>Advanced</b> adds auth headers or mirror URLs for extra bandwidth.
        </li>
      </ul>
    </div>
  );
}
