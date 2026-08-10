// Theme picker (DESIGN.md §2). Three explicit states rather than a cycling icon: a control
// whose next state you have to guess is a mode you cannot see you are in.
import { setTheme, type ThemeChoice, useTheme } from "../lib/theme";
import type { Options } from "./controls";
import { Segmented } from "./Segmented";

const CHOICES: Options<ThemeChoice> = [
  { value: "system", label: "Auto" },
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
];

export function ThemeControl() {
  const { choice } = useTheme();
  return (
    <div className="my-0.5 flex items-center max-md:hidden">
      <Segmented label="Theme" value={choice} options={CHOICES} onChange={setTheme} />
    </div>
  );
}
