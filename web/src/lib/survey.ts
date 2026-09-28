import { useMutation, useQuery } from "@tanstack/react-query";
import { create } from "zustand";
import { controlSurvey, surveyQuery } from "./api";
import { omitNodes } from "./byNode";
import { clearAction, failAction } from "./refusals";
import type {
  ServerEvent,
  SurveyAction,
  SurveyCell,
  SurveyGrid,
  SurveyStop,
  SurveyUpdate,
} from "./types";
import { useSeed } from "./useSeed";

export interface SurveyState {
  cells: readonly SurveyCell[];
  recording: boolean;
  levelDbfs: number | null;
  targetHz: number | null;
  dropped: number;
  stopped: SurveyStop | null;
}

export interface SurveyStore {
  byNode: Readonly<Record<string, SurveyState>>;
  observe: (event: ServerEvent) => void;
  seed: (grid: SurveyGrid) => void;
  forget: (nodes: readonly string[]) => void;
  reset: () => void;
}

export const EMPTY_SURVEY: SurveyState = {
  cells: [],
  recording: false,
  levelDbfs: null,
  targetHz: null,
  dropped: 0,
  stopped: null,
};

const METRES_PER_DEG_LON = 111_320;
const METRES_PER_DEG_LAT = 110_540;
const CELL_M = 10;

export function cellKey(cell: SurveyCell): string {
  const lat = (cell.latitude * Math.PI) / 180;
  const x = cell.longitude * METRES_PER_DEG_LON * Math.max(0.01, Math.cos(lat));
  const y = cell.latitude * METRES_PER_DEG_LAT;
  return `${Math.round(cell.frequency_hz)}:${Math.round(x / CELL_M)}:${Math.round(y / CELL_M)}`;
}

export function withCell(
  cells: readonly SurveyCell[],
  cell: SurveyCell,
  held: number,
): readonly SurveyCell[] {
  const key = cellKey(cell);
  const at = cells.findIndex((existing) => cellKey(existing) === key);
  const merged =
    at === -1 ? [...cells, cell] : cells.map((existing, index) => (index === at ? cell : existing));
  return merged.length > held ? merged.slice(merged.length - held) : merged;
}

function followed(previous: SurveyState, update: SurveyUpdate): SurveyState {
  const cells =
    update.cell == null
      ? previous.cells.slice(Math.max(0, previous.cells.length - update.cells))
      : withCell(previous.cells, update.cell, update.cells);
  return {
    cells,
    recording: update.recording,
    levelDbfs: update.level_dbfs ?? null,
    targetHz: update.target_hz ?? previous.targetHz,
    dropped: update.dropped,
    stopped: update.stopped ?? (update.recording ? null : previous.stopped),
  };
}

export const useSurveyStore = create<SurveyStore>((set) => ({
  byNode: {},
  observe: (event) => {
    if (event.type !== "SurveyUpdate") {
      return;
    }
    const { node, update } = event.data;
    set((state) => ({
      byNode: { ...state.byNode, [node]: followed(state.byNode[node] ?? EMPTY_SURVEY, update) },
    }));
  },
  seed: (grid) =>
    set((state) => ({
      byNode: {
        ...state.byNode,
        [grid.node]: {
          ...(state.byNode[grid.node] ?? EMPTY_SURVEY),
          cells: grid.cells,
          recording: grid.recording,
          targetHz: grid.frequency_hz ?? null,
          dropped: grid.dropped ?? 0,
        },
      },
    })),
  forget: (nodes) => set((state) => ({ byNode: omitNodes(state.byNode, nodes) })),
  reset: () => set({ byNode: {} }),
}));

export const SURVEY_ACTION = "Survey";

function seedSurvey(grid: SurveyGrid): void {
  useSurveyStore.getState().seed(grid);
}

export function useSurveySeed(node: string): void {
  const query = useQuery(surveyQuery(node));
  const held = useSurveyStore((store) => store.byNode[node] !== undefined);
  useSeed(node, SURVEY_ACTION, query, held, seedSurvey);
}

export function useSurveyControl(node: string): {
  control: (action: SurveyAction) => void;
  pending: boolean;
} {
  const send = useMutation({
    mutationFn: (action: SurveyAction) => controlSurvey(node, action),
    onSuccess: (grid) => {
      seedSurvey(grid);
      clearAction(node, SURVEY_ACTION);
    },
    onError: (error) => failAction(node, SURVEY_ACTION, error),
  });
  return { control: (action) => send.mutate(action), pending: send.isPending };
}
