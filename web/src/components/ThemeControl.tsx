import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { setTheme, type ThemeChoice, useTheme } from "../lib/theme";

const NAMES: Record<ThemeChoice, string> = {
  system: "Auto",
  dark: "Dark",
  light: "Light",
};

export function ThemeControl() {
  const { choice } = useTheme();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`Theme: ${NAMES[choice]}`}
            title={`Theme: ${NAMES[choice]}`}
          />
        }
      >
        <ThemeIcon choice={choice} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>Theme</DropdownMenuLabel>
        <DropdownMenuRadioGroup
          value={choice}
          onValueChange={(value) => {
            if (value === "system" || value === "dark" || value === "light") setTheme(value);
          }}
        >
          {Object.entries(NAMES).map(([value, name]) => (
            <DropdownMenuRadioItem key={value} value={value}>
              {name}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Sun, moon, and the two of them split down the middle for auto — one 16px grid, so the three
 * states are the same weight of ink and the button never shifts. */
function ThemeIcon({ choice }: { choice: ThemeChoice }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className="size-4"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      aria-hidden
    >
      {choice === "light" && (
        <>
          <circle cx="8" cy="8" r="3.2" />
          <path d="M8 1v1.8M8 13.2V15M1 8h1.8M13.2 8H15M3.1 3.1l1.3 1.3M11.6 11.6l1.3 1.3M12.9 3.1l-1.3 1.3M4.4 11.6l-1.3 1.3" />
        </>
      )}
      {choice === "dark" && <path d="M13.4 9.6A5.8 5.8 0 0 1 6.4 2.6a5.8 5.8 0 1 0 7 7Z" />}
      {choice === "system" && (
        <>
          <circle cx="8" cy="8" r="5.4" />
          <path d="M8 2.6a5.4 5.4 0 0 1 0 10.8Z" fill="currentColor" stroke="none" />
        </>
      )}
    </svg>
  );
}
