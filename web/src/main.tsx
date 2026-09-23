import { createRoot } from "react-dom/client";
import { App } from "./App";
import { FieldApp } from "./field/FieldApp";
import { isFieldPath } from "./field/missions";
import { adoptTokenFromUrl } from "./lib/auth";
import { installGlobalHandlers } from "./lib/diagnostics";
import { initTheme } from "./lib/theme";
import { createQueryClient, Root } from "./Root";
import "@fontsource-variable/jetbrains-mono";
import "./index.css";

initTheme();
installGlobalHandlers(window);
adoptTokenFromUrl(window.location, window.history);

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("missing #root element");
}
const Face = isFieldPath(window.location.pathname) ? FieldApp : App;

createRoot(rootEl).render(
  <Root client={createQueryClient()}>
    <Face />
  </Root>,
);
