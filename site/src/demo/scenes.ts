export interface DemoScene {
  id: string;
  label: string;
}

export const SCENES: DemoScene[] = [
  { id: "rds", label: "FM radio" },
  { id: "adsb", label: "Aircraft" },
  { id: "pocsag", label: "Pagers" },
  { id: "ais", label: "Ships" },
  { id: "ft8", label: "FT8" },
];

export const DEFAULT_SCENE = "rds";

export function sceneFrom(search: string): string {
  const asked = new URLSearchParams(search).get("scene");
  return SCENES.some((scene) => scene.id === asked) ? (asked ?? DEFAULT_SCENE) : DEFAULT_SCENE;
}

export function demoUrl(scene: string): string {
  return `/demo.html?scene=${encodeURIComponent(scene)}`;
}
