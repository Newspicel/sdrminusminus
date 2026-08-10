// Theme picker (DESIGN.md §2). Three explicit states rather than a cycling icon: a control
// whose next state you have to guess is a mode you cannot see you are in.
import { setTheme, type ThemeChoice, useTheme } from "../lib/theme";
import { segment } from "./controls";

const CHOICES: readonly { value: ThemeChoice; label: string }[] = [
  { value: "system", label: "Auto" },
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
];

export function ThemeControl() {
  const { choice } = useTheme();
  return (
    <div
      role="group"
      aria-label="Theme"
      className="my-0.5 flex items-center overflow-hidden rounded-[3px] border border-line max-md:hidden"
    >
      {CHOICES.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={choice === option.value}
          className={`${segment(choice === option.value)} rounded-none px-2`}
          onClick={() => setTheme(option.value)}
        >
          <span className="legend text-current">{option.label}</span>
        </button>
      ))}
    </div>
  );
}
