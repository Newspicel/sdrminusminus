import { createRoot } from "react-dom/client";
import { App } from "./App";
import { installGlobalHandlers } from "./lib/diagnostics";
import { initTheme } from "./lib/theme";
import { createQueryClient, Root } from "./Root";
import "@fontsource-variable/jetbrains-mono";
import "./index.css";

initTheme();
installGlobalHandlers(window);

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("missing #root element");
}

createRoot(rootEl).render(
  <Root client={createQueryClient()}>
    <App />
  </Root>,
);
