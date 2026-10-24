import { create } from "zustand";

export interface LauncherState {
  nickname: string | null;
  installDir: string | null;
  installed: boolean;
  driveError: string | null;
}

export interface MemoryStatus {
  totalMib: number;
  availableMib: number;
  heapMib: number | null;
}

interface LauncherStore {
  launcherState: LauncherState | null;
  nickname: string;
  busy: boolean;
  gameRunning: boolean;
  memoryStatus: MemoryStatus | null;
  progress: number;
  stage: string;
  error: string;
  logs: string[];
  setLauncherState: (state: LauncherState) => void;
  setNickname: (nickname: string) => void;
  setBusy: (busy: boolean) => void;
  setGameRunning: (running: boolean) => void;
  setMemoryStatus: (status: MemoryStatus) => void;
  setProgress: (message: string) => void;
  setError: (error: string) => void;
  addLog: (message: string) => void;
}

function progressForMessage(message: string) {
  const normalized = message.toLocaleLowerCase("ru-RU");
  if (normalized.includes("проверка установки")) return 8;
  if (normalized.includes("java")) return 22;
  if (normalized.includes("метадан")) return 32;
  if (normalized.includes("библиотек")) return 50;
  if (normalized.includes("ресурс")) return 70;
  if (normalized.includes("neoforge") || normalized.includes("загрузчик")) return 82;
  if (normalized.includes("mods") || normalized.includes("мод")) return 92;
  if (normalized.includes("запуск")) return 98;
  return 45;
}

function timestamp() {
  return new Date().toLocaleTimeString("ru-RU", { hour12: false });
}

export const useLauncherStore = create<LauncherStore>((set) => ({
  launcherState: null,
  nickname: "",
  busy: false,
  gameRunning: false,
  memoryStatus: null,
  progress: 0,
  stage: "Ожидание команды",
  error: "",
  logs: ["Ожидание подключения к игровому контуру."],
  setLauncherState: (launcherState) =>
    set((state) => ({
      launcherState,
      nickname:
        launcherState.nickname ?? (state.launcherState?.nickname ? "" : state.nickname),
    })),
  setNickname: (nickname) => set({ nickname }),
  setBusy: (busy) => set({ busy }),
  setGameRunning: (gameRunning) => set({ gameRunning }),
  setMemoryStatus: (memoryStatus) => set({ memoryStatus }),
  setProgress: (message) =>
    set((state) => ({
      stage: message,
      progress: progressForMessage(message),
      logs: [...state.logs, `[${timestamp()}] ${message}`].slice(-7),
    })),
  setError: (error) =>
    set((state) =>
      state.error === error
        ? state
        : {
            error,
            logs: error
              ? [...state.logs, `[${timestamp()}] ERROR // ${error}`].slice(-7)
              : state.logs,
          },
    ),
  addLog: (message) =>
    set((state) => ({
      stage: message,
      logs: [...state.logs, `[${timestamp()}] ${message}`].slice(-7),
    })),
}));
