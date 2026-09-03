import { type LucideIcon, Monitor, Moon, Sun } from "lucide-react";
import { nextTheme, setTheme, type ThemeChoice, useTheme } from "../lib/theme";
import { Button } from "./BaseControls";
import { ICON_BTN } from "./controls";
import { Icon } from "./Icon";

const THEME_ICONS: Record<ThemeChoice, LucideIcon> = {
  system: Monitor,
  dark: Moon,
  light: Sun,
};

const NAMES: Record<ThemeChoice, string> = {
  system: "Auto",
  dark: "Dark",
  light: "Light",
};

export function ThemeControl() {
  const { choice } = useTheme();
  const next = nextTheme(choice);
  return (
    <Button
      type="button"
      className={ICON_BTN}
      aria-label={`Theme: ${NAMES[choice]}. Switch to ${NAMES[next]}`}
      title={`Theme: ${NAMES[choice]} — click for ${NAMES[next]}`}
      onClick={() => setTheme(next)}
    >
      <Icon glyph={THEME_ICONS[choice]} size={16} />
    </Button>
  );
}
