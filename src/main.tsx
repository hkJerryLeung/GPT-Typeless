/**
 * Main Entry Point
 */
import React from "react";
import ReactDOM from "react-dom/client";

import "./App.css";
const overlay = new URLSearchParams(window.location.search).get("overlay") === "1";
document.documentElement.classList.toggle("overlay-window", overlay);
// Dynamic imports keep the overlay independent of main initialization and credentials.
const entry = overlay ? import("./components/VoiceOverlay") : import("./App");
void entry.then(({ default: Entry }) => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode><Entry /></React.StrictMode>,
  );
});
