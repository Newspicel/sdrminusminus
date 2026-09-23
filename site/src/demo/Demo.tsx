import "@fontsource-variable/jetbrains-mono";
import "./install";
import { App } from "../../../web/src/App";
import { initTheme, setTheme } from "../../../web/src/lib/theme";
import { createQueryClient, Root } from "../../../web/src/Root";
import "./demo.css";

initTheme();
setTheme("dark");
const client = createQueryClient();

export default function Demo() {
  return (
    <Root client={client}>
      <App />
    </Root>
  );
}
