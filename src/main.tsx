import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ConfirmWindow } from "./components/ConfirmWindow";
import "./styles.css";

// One bundle, two windows: the main window, and the "Download request"
// popup the backend opens as `index.html#confirm`.
const isPopup = window.location.hash === "#confirm";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isPopup ? <ConfirmWindow /> : <App />}</React.StrictMode>
);
