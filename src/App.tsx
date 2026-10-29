import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import packageInfo from "../package.json";
import {
  Activity,
  ArrowDownToLine,
  Check,
  CircleCheck,
  CircleX,
  Cpu,
  Download,
  FolderOpen,
  Gamepad2,
  LoaderCircle,
  LockKeyhole,
  MemoryStick,
  MonitorDown,
  Play,
  Radio,
  RefreshCw,
  Terminal,
  UserRound,
} from "lucide-react";
import { MinecraftDiamond, MinecraftIronIngot } from "minecraft-items-react";
import { Button } from "./components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "./components/ui/card";
import { Input } from "./components/ui/input";
import { Progress } from "./components/ui/progress";
import { ScrollArea } from "./components/ui/scroll-area";
import {
  useLauncherStore,
  type LauncherState,
  type MemoryStatus,
} from "./store/launcher-store";

interface ProgressEvent {
  message: string;
}

interface GameStateEvent {
  running: boolean;
  message: string;
}

interface UpdateProgressEvent {
  phase: string;
  version: string;
  downloaded: number;
  total: number | null;
}

interface AvailableUpdate {
  version: string;
}

type UpdateVisualState =
  "idle" | "checking" | "downloading" | "installing" | "current" | "available" | "error";

function ParticleBackdrop() {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;

    let frame = 0;
    let width = 0;
    let height = 0;
    let pixelRatio = 1;
    let animationFrame = 0;
    let particles: Array<{ x: number; y: number; speed: number; radius: number }> = [];

    const resize = () => {
      const bounds = canvas.getBoundingClientRect();
      width = bounds.width;
      height = bounds.height;
      pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.floor(width * pixelRatio);
      canvas.height = Math.floor(height * pixelRatio);
      context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
      particles = Array.from({ length: Math.min(64, Math.floor(width / 14)) }, () => ({
        x: Math.random() * width,
        y: Math.random() * height,
        speed: 0.14 + Math.random() * 0.42,
        radius: 0.6 + Math.random() * 1.1,
      }));
    };

    const draw = () => {
      frame += 1;
      context.clearRect(0, 0, width, height);
      for (const particle of particles) {
        particle.y += particle.speed;
        particle.x += Math.sin((frame + particle.y) / 110) * 0.08;
        if (particle.y > height + 4) {
          particle.y = -4;
          particle.x = Math.random() * width;
        }
        context.beginPath();
        context.arc(particle.x, particle.y, particle.radius, 0, Math.PI * 2);
        context.fillStyle = "rgba(134, 213, 98, 0.56)";
        context.fill();
      }

      for (let first = 0; first < particles.length; first += 1) {
        for (let second = first + 1; second < particles.length; second += 1) {
          const a = particles[first];
          const b = particles[second];
          const dx = a.x - b.x;
          const dy = a.y - b.y;
          const distance = Math.sqrt(dx * dx + dy * dy);
          if (distance < 92) {
            context.beginPath();
            context.moveTo(a.x, a.y);
            context.lineTo(b.x, b.y);
            context.strokeStyle = `rgba(108, 195, 73, ${0.12 * (1 - distance / 92)})`;
            context.stroke();
          }
        }
      }
      animationFrame = window.requestAnimationFrame(draw);
    };

    resize();
    animationFrame = window.requestAnimationFrame(draw);
    window.addEventListener("resize", resize);
    return () => {
      window.cancelAnimationFrame(animationFrame);
      window.removeEventListener("resize", resize);
    };
  }, []);

  return (
    <canvas
      ref={canvasRef}
      aria-hidden="true"
      className="pointer-events-none absolute inset-0 size-full opacity-55"
    />
  );
}

function formatError(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

function hideServerAddress(value: string) {
  return value.replace(/\b(?:\d{1,3}\.){3}\d{1,3}(?::\d{1,5})?\b/g, "remote");
}

export default function App() {
  const [availableUpdate, setAvailableUpdate] = useState<string | null>(null);
  const [updateVisualState, setUpdateVisualState] = useState<UpdateVisualState>("idle");
  const [updateChecking, setUpdateChecking] = useState(false);
  const [updateDownloading, setUpdateDownloading] = useState(false);
  const [updateStatus, setUpdateStatus] = useState(
    import.meta.env.PROD
      ? "Обновления не проверены"
      : "Обновления доступны в release-сборке",
  );
  const [updateInstalling, setUpdateInstalling] = useState(false);
  const updateCheckingRef = useRef(false);
  const launcherState = useLauncherStore((state) => state.launcherState);
  const nickname = useLauncherStore((state) => state.nickname);
  const busy = useLauncherStore((state) => state.busy);
  const gameRunning = useLauncherStore((state) => state.gameRunning);
  const memoryStatus = useLauncherStore((state) => state.memoryStatus);
  const progress = useLauncherStore((state) => state.progress);
  const stage = useLauncherStore((state) => state.stage);
  const logs = useLauncherStore((state) => state.logs);
  const setLauncherState = useLauncherStore((state) => state.setLauncherState);
  const setNickname = useLauncherStore((state) => state.setNickname);
  const setBusy = useLauncherStore((state) => state.setBusy);
  const setGameRunning = useLauncherStore((state) => state.setGameRunning);
  const setMemoryStatus = useLauncherStore((state) => state.setMemoryStatus);
  const setProgress = useLauncherStore((state) => state.setProgress);
  const setError = useLauncherStore((state) => state.setError);
  const addLog = useLauncherStore((state) => state.addLog);

  const checkForUpdates = useCallback(async () => {
    if (updateCheckingRef.current) return;
    updateCheckingRef.current = true;
    setUpdateChecking(true);
    setUpdateDownloading(false);
    setUpdateVisualState("checking");
    setUpdateStatus("Проверка обновлений…");

    try {
      const update = await invoke<AvailableUpdate | null>("check_and_download_update");
      if (update) {
        setAvailableUpdate(update.version);
        setUpdateVisualState("available");
        setUpdateStatus(`Доступна новая версия ${update.version}`);
      } else {
        setAvailableUpdate(null);
        setUpdateVisualState("current");
        setUpdateStatus("Обновлений нет");
      }
    } catch (reason) {
      const message = formatError(reason);
      setUpdateVisualState("error");
      setUpdateStatus(`Не удалось проверить обновления: ${message}`);
      addLog(`Проверка обновлений: ${message}`);
    } finally {
      updateCheckingRef.current = false;
      setUpdateChecking(false);
      setUpdateDownloading(false);
    }
  }, [addLog]);

  useEffect(() => {
    if (!import.meta.env.PROD) return;

    let mounted = true;
    let disposeProgress: (() => void) | undefined;
    void listen<UpdateProgressEvent>("launcher-update-progress", ({ payload }) => {
      if (!mounted || payload.phase !== "downloading") return;
      const percent = payload.total
        ? Math.min(100, Math.floor((payload.downloaded / payload.total) * 100))
        : null;
      setUpdateChecking(false);
      setUpdateDownloading(true);
      setUpdateVisualState("downloading");
      setUpdateStatus(
        percent === null
          ? `Загрузка обновления ${payload.version}…`
          : `Загрузка обновления: ${percent}%`,
      );
    }).then((dispose) => {
      if (mounted) disposeProgress = dispose;
      else dispose();
      void checkForUpdates();
    });
    const interval = window.setInterval(() => void checkForUpdates(), 6 * 60 * 60 * 1000);
    return () => {
      mounted = false;
      window.clearInterval(interval);
      disposeProgress?.();
    };
  }, [checkForUpdates]);

  useEffect(() => {
    let mounted = true;
    const disposers: Array<() => void> = [];
    const attach = <T,>(eventName: string, handler: (payload: T) => void) => {
      void listen<T>(eventName, (event) => handler(event.payload)).then((dispose) => {
        if (mounted) disposers.push(dispose);
        else dispose();
      });
    };

    attach<ProgressEvent>("launcher-progress", (event) => {
      if (mounted) setProgress(event.message);
    });
    attach<GameStateEvent>("launcher-game-state", (event) => {
      if (!mounted) return;
      setGameRunning(event.running);
      addLog(event.message);
      if (!event.running && event.message.includes("ошибкой")) {
        setError(event.message);
      }
    });

    const refreshLauncherState = () => {
      void invoke<LauncherState>("get_launcher_state")
        .then((state) => {
          if (mounted) setLauncherState(state);
        })
        .catch((reason: unknown) => {
          if (mounted) setError(formatError(reason));
        });
    };
    const refreshGameStatus = () => {
      void invoke<boolean>("get_game_status")
        .then((running) => {
          if (mounted) setGameRunning(running);
        })
        .catch((reason: unknown) => {
          if (mounted) setError(formatError(reason));
        });
    };
    const refreshMemoryStatus = () => {
      void invoke<MemoryStatus>("get_memory_status")
        .then((status) => {
          if (!mounted) return;
          setMemoryStatus(status);
        })
        .catch((reason: unknown) => {
          if (mounted) setError(formatError(reason));
        });
    };
    refreshGameStatus();
    const statusTimer = window.setInterval(refreshGameStatus, 1000);
    refreshLauncherState();
    const launcherStateTimer = window.setInterval(refreshLauncherState, 10000);
    refreshMemoryStatus();
    const memoryStatusTimer = window.setInterval(refreshMemoryStatus, 10000);
    const refreshCurrentState = () => {
      refreshGameStatus();
      refreshLauncherState();
      refreshMemoryStatus();
    };
    window.addEventListener("focus", refreshCurrentState);

    return () => {
      mounted = false;
      window.clearInterval(statusTimer);
      window.clearInterval(launcherStateTimer);
      window.clearInterval(memoryStatusTimer);
      window.removeEventListener("focus", refreshCurrentState);
      disposers.forEach((dispose) => dispose());
    };
  }, [addLog, setError, setGameRunning, setLauncherState, setMemoryStatus, setProgress]);

  async function chooseDirectory() {
    setError("");
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected !== "string") return;
      await invoke("save_install_directory", { path: selected });
      const next = await invoke<LauncherState>("get_launcher_state");
      setLauncherState(next);
      addLog(`Каталог установки изменён: ${selected}`);
    } catch (reason) {
      setError(formatError(reason));
    }
  }

  async function play() {
    setBusy(true);
    setError("");
    setProgress("Проверка установки Minecraft и NeoForge…");
    try {
      const running = await invoke<boolean>("play", {
        nickname: launcherState?.nickname ? null : nickname,
      });
      if (running) {
        addLog("Игра запущена — окно лаунчера свёрнуто.");
        await getCurrentWindow().minimize();
      } else {
        addLog("Игровой процесс уже завершился.");
      }
    } catch (reason) {
      setGameRunning(false);
      setError(formatError(reason));
    } finally {
      setBusy(false);
      try {
        const next = await invoke<LauncherState>("get_launcher_state");
        setLauncherState(next);
      } catch (reason) {
        if (!useLauncherStore.getState().error) setError(formatError(reason));
      }
    }
  }

  async function updateLauncher() {
    setUpdateInstalling(true);
    setUpdateVisualState("installing");
    setUpdateStatus("Установка обновления и перезапуск…");
    try {
      await invoke("install_downloaded_update");
    } catch (reason) {
      const message = formatError(reason);
      setUpdateVisualState("error");
      setUpdateStatus(`Ошибка обновления: ${message}`);
      addLog(`Не удалось установить обновление: ${message}`);
    } finally {
      setUpdateInstalling(false);
    }
  }

  const isInstalled = launcherState?.installed ?? false;

  return (
    <main className="relative isolate flex h-screen min-h-0 w-screen flex-col overflow-hidden bg-[#15120f] font-mono text-[#f0e9dd] selection:bg-lime-400/25 [&_svg]:shrink-0">
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 bg-[linear-gradient(rgba(129,86,58,0.15)_1px,transparent_1px),linear-gradient(90deg,rgba(129,86,58,0.15)_1px,transparent_1px)] bg-[size:36px_36px] [mask-image:radial-gradient(ellipse_at_center,black_20%,transparent_100%)]"
      />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -left-36 -top-48 size-[480px] rounded-full bg-lime-500/[0.07] blur-[120px]"
      />
      <div
        aria-hidden="true"
        className="pointer-events-none absolute -bottom-52 -right-32 size-[460px] rounded-full bg-amber-800/[0.12] blur-[130px]"
      />
      <ParticleBackdrop />

      <ScrollArea className="relative z-10 mx-auto w-full max-w-6xl flex-1">
        <div className="flex min-h-full flex-col gap-3 p-4 sm:p-5">
          <header className="flex min-h-12 items-center justify-between gap-4 border-b border-[#49392a]/80 pb-3">
            <div className="flex items-center gap-3">
              <div className="flex size-9 items-center justify-center rounded-md border border-lime-400/30 bg-lime-400/[0.08] text-lime-200">
                <MinecraftDiamond size={22} className="[image-rendering:pixelated]" />
              </div>
              <div>
                <h1 className="text-sm font-semibold tracking-[0.12em] text-[#f0e9dd]">
                  1stMay <span className="text-lime-300">Server</span>
                </h1>
                <p className="mt-0.5 text-[9px] uppercase tracking-[0.2em] text-[#9a8872]">
                  Client control interface
                </p>
              </div>
            </div>
            <div className="flex min-w-0 flex-wrap items-center justify-end gap-3">
              <div className="flex items-center gap-2">
                <span
                  className={`flex size-9 shrink-0 items-center justify-center rounded-full border bg-[#211a13]/80 ${
                    updateVisualState === "current"
                      ? "border-emerald-400/30 text-emerald-300 shadow-[0_0_14px_rgba(52,211,153,0.1)]"
                      : updateVisualState === "error"
                        ? "border-red-400/30 text-red-400 shadow-[0_0_14px_rgba(248,113,113,0.1)]"
                        : updateVisualState === "available"
                          ? "border-lime-300/30 text-lime-300"
                          : updateVisualState === "downloading" ||
                              updateVisualState === "installing"
                            ? "border-lime-300/30 text-lime-300"
                            : "border-[#66513a]/70 text-[#a6957e]"
                  }`}
                  title={updateStatus}
                  role="status"
                  aria-label={updateStatus}
                >
                  {updateVisualState === "current" ? (
                    <CircleCheck size={17} />
                  ) : updateVisualState === "error" ? (
                    <CircleX size={17} />
                  ) : updateVisualState === "checking" ? (
                    <LoaderCircle size={16} className="animate-spin" />
                  ) : updateVisualState === "downloading" ? (
                    <Download size={16} className="animate-bounce" />
                  ) : updateVisualState === "installing" ? (
                    <MonitorDown size={16} className="animate-pulse" />
                  ) : updateVisualState === "available" ? (
                    <ArrowDownToLine size={16} />
                  ) : (
                    <RefreshCw size={15} />
                  )}
                </span>
                <Button
                  variant="primary"
                  size="sm"
                  className="h-9 min-w-28 rounded-lg px-3.5 text-[10px] leading-none tracking-[0.08em] shadow-[0_4px_18px_rgba(108,195,73,0.13)]"
                  onClick={() =>
                    void (availableUpdate ? updateLauncher() : checkForUpdates())
                  }
                  disabled={
                    busy ||
                    gameRunning ||
                    updateChecking ||
                    updateDownloading ||
                    updateInstalling
                  }
                  title={updateStatus}
                  aria-label={
                    availableUpdate
                      ? `Установить обновление ${availableUpdate}`
                      : "Проверить наличие обновления"
                  }
                >
                  {updateInstalling ? (
                    <>
                      <MonitorDown size={13} className="animate-pulse" />
                      Установка…
                    </>
                  ) : updateDownloading ? (
                    <>
                      <Download size={13} className="animate-bounce" />
                      {updateStatus.startsWith("Загрузка обновления:")
                        ? updateStatus.replace("Загрузка обновления: ", "")
                        : "Загрузка…"}
                    </>
                  ) : updateChecking ? (
                    <>
                      <LoaderCircle size={13} className="animate-spin" />
                      Проверка…
                    </>
                  ) : availableUpdate ? (
                    "Установить"
                  ) : updateVisualState === "error" ? (
                    "Повторить"
                  ) : (
                    "Проверить"
                  )}
                </Button>
              </div>
              <div className="hidden shrink-0 items-center gap-2 text-[10px] uppercase tracking-widest text-[#9a8872] sm:flex">
                <LockKeyhole size={13} className="text-lime-400/70" />
                local profile
              </div>
              <div
                className={`inline-flex h-7 shrink-0 items-center justify-center gap-2 rounded-full border px-3 text-[9px] font-semibold uppercase leading-none tracking-[0.16em] ${
                  gameRunning
                    ? "border-emerald-400/25 bg-emerald-400/[0.07] text-emerald-300"
                    : "border-[#66513a] bg-[#211a13]/90 text-[#c4b8a5]"
                }`}
              >
                <span
                  className={`size-1.5 rounded-full ${
                    gameRunning
                      ? "animate-pulse bg-emerald-300 shadow-[0_0_9px_rgba(110,231,183,0.9)]"
                      : "bg-[#806d57]"
                  }`}
                />
                {gameRunning
                  ? "Client active"
                  : busy
                    ? "Preparing"
                    : !launcherState
                      ? "Checking"
                      : isInstalled
                        ? "Client ready"
                        : "Install required"}
              </div>
            </div>
          </header>

          <section className="flex items-end justify-between gap-4">
            <div>
              <p className="mb-1 flex items-center gap-2 text-[9px] uppercase tracking-[0.2em] text-lime-300/80">
                <Activity size={12} />
                Session gateway / 01
              </p>
              <h2 className="text-lg font-medium tracking-tight text-[#f0e9dd] sm:text-xl">
                {gameRunning ? "Игровая сессия активна" : "Готовность игрового контура"}
              </h2>
            </div>
            <div className="hidden gap-2 text-right sm:block">
              <p className="text-[9px] uppercase tracking-[0.16em] text-[#806d57]">
                build channel
              </p>
              <p className="mt-1 text-[10px] text-[#b2a38e]">
                MC 1.21.1 // NEOFORGE 21.1.264
              </p>
              <p className="mt-1 text-[10px] text-lime-300/80">
                1stMay Launcher v{packageInfo.version}
              </p>
            </div>
          </section>

          <div className="grid min-h-max flex-1 grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)] grid-rows-[auto_auto] gap-3">
            <Card className="flex min-h-0 flex-col overflow-hidden">
              <CardContent className="flex flex-1 flex-col p-4 sm:p-5">
                <CardHeader className="mb-5">
                  <CardTitle className="flex items-center gap-2 leading-none">
                    <UserRound size={14} className="text-lime-300" />
                    Профиль игрока
                  </CardTitle>
                  <span className="inline-flex h-5 items-center justify-center gap-1.5 leading-none text-[9px] uppercase tracking-widest text-emerald-400/80">
                    <MinecraftIronIngot
                      size={14}
                      className="[image-rendering:pixelated]"
                    />
                    offline
                  </span>
                </CardHeader>

                <label className="mb-2 text-[9px] uppercase tracking-[0.16em] text-[#9a8872]">
                  Никнейм Minecraft
                </label>
                {launcherState?.nickname ? (
                  <div className="flex h-11 items-center gap-3 rounded-md border border-[#49392a] bg-[#17130f] px-3.5">
                    <span className="size-1.5 rounded-full bg-lime-400" />
                    <span className="text-sm text-[#f0e9dd]">
                      {launcherState.nickname}
                    </span>
                    <Check size={14} className="ml-auto text-emerald-400" />
                  </div>
                ) : (
                  <Input
                    autoComplete="off"
                    maxLength={16}
                    placeholder="Введите игровой ник"
                    value={nickname}
                    onChange={(event) => setNickname(event.target.value)}
                    disabled={busy || updateInstalling}
                    aria-label="Никнейм Minecraft"
                  />
                )}
                <p className="mt-2 text-[9px] leading-relaxed text-[#806d57]">
                  {launcherState?.nickname
                    ? "Локальный профиль • без Microsoft-авторизации"
                    : "3–16 символов · латиница, цифры, подчёркивание"}
                </p>

                {!isInstalled && (
                  <div className="mt-5 border-t border-[#49392a]/80 pt-4">
                    <div className="mb-2 flex items-center justify-between gap-2">
                      <label className="text-[9px] uppercase tracking-[0.16em] text-[#9a8872]">
                        Каталог установки
                      </label>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7"
                        onClick={() => void chooseDirectory()}
                        disabled={busy || updateInstalling || !launcherState}
                        title="Выбрать папку"
                      >
                        <FolderOpen size={13} />
                        Обзор
                      </Button>
                    </div>
                    <p className="truncate rounded border border-[#49392a]/80 bg-black/20 px-2.5 py-2 font-mono text-[10px] text-[#b2a38e]">
                      {launcherState?.installDir ?? "Автовыбор диска"}
                    </p>
                    {launcherState?.driveError && (
                      <p className="mt-2 text-[9px] leading-relaxed text-amber-300/80">
                        {launcherState.driveError}
                      </p>
                    )}
                  </div>
                )}

                <div className="mt-auto pt-5">
                  {busy && (
                    <div className="mb-3">
                      <div className="mb-2 flex items-center justify-between gap-3 text-[9px] uppercase tracking-wider text-lime-200/90">
                        <span className="flex min-w-0 items-center gap-2">
                          <LoaderCircle size={12} className="shrink-0 animate-spin" />
                          <span className="truncate leading-none">{stage}</span>
                        </span>
                        <span>{progress}%</span>
                      </div>
                      <Progress value={progress} indeterminate />
                    </div>
                  )}
                  <Button
                    size="play"
                    onClick={() => void play()}
                    disabled={busy || updateInstalling || gameRunning || !launcherState}
                  >
                    {busy ? (
                      <LoaderCircle size={16} className="animate-spin" />
                    ) : gameRunning ? (
                      <Radio size={16} />
                    ) : (
                      <Play size={16} fill="currentColor" />
                    )}
                    {busy
                      ? "Подготовка сессии"
                      : gameRunning
                        ? "Игра запущена"
                        : "Играть"}
                  </Button>
                </div>
              </CardContent>
            </Card>

            <Card className="flex flex-col">
              <CardContent className="flex flex-1 flex-col p-4 sm:p-5">
                <CardHeader className="mb-5">
                  <CardTitle className="flex items-center gap-2">
                    <Cpu size={14} className="text-lime-300" />
                    Состояние системы
                  </CardTitle>
                  <span className="text-[9px] uppercase tracking-wider text-[#806d57]">
                    live telemetry
                  </span>
                </CardHeader>

                <div className="space-y-2">
                  <div className="flex items-center justify-between border-b border-[#49392a]/70 pb-2">
                    <span className="flex items-center gap-2 text-[10px] leading-none text-[#9a8872]">
                      <Gamepad2 size={13} />
                      Игровой клиент
                    </span>
                    <span
                      className={`text-[10px] font-medium ${
                        gameRunning ? "text-emerald-300" : "text-[#b2a38e]"
                      }`}
                    >
                      {gameRunning ? "АКТИВЕН" : "НЕ ЗАПУЩЕН"}
                    </span>
                  </div>
                  <div className="flex items-center justify-between border-b border-[#49392a]/70 pb-2">
                    <span className="flex items-center gap-2 text-[10px] leading-none text-[#9a8872]">
                      <Activity size={13} />
                      Загрузчик
                    </span>
                    <span className="text-[10px] text-[#d0c4b3]">
                      {!launcherState
                        ? "ПРОВЕРКА"
                        : busy &&
                            !/(синхронизация|сверка списка модов|загрузка мода)/i.test(
                              stage,
                            )
                          ? "ПОДГОТОВКА"
                          : isInstalled
                            ? "NEOFORGE / 21.1.264"
                            : "НЕ УСТАНОВЛЕН"}
                    </span>
                  </div>
                  <div className="flex items-center justify-between border-b border-[#49392a]/70 pb-2">
                    <span className="flex items-center gap-2 text-[10px] leading-none text-[#9a8872]">
                      <ArrowDownToLine size={13} />
                      Управляемые моды
                    </span>
                    <span className="text-[10px] text-[#d0c4b3]">
                      {!launcherState
                        ? "ПРОВЕРКА"
                        : busy &&
                            /(синхронизация|сверка списка модов|загрузка мода)/i.test(
                              stage,
                            )
                          ? "СИНХРОНИЗАЦИЯ"
                          : isInstalled
                            ? "СВЕРКА ПРИ ЗАПУСКЕ"
                            : "ОЖИДАЮТ УСТАНОВКИ"}
                    </span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="flex items-center gap-2 text-[10px] leading-none text-[#9a8872]">
                      <Radio size={13} />
                      Профиль входа
                    </span>
                    <span className="text-[10px] text-emerald-300">
                      OFFLINE / NICKNAME
                    </span>
                  </div>
                </div>

                <div className="mt-3 grid grid-cols-2 gap-2">
                  <div className="rounded-md border border-[#49392a]/80 bg-black/20 p-2.5">
                    <div className="mb-1.5 flex items-center gap-1.5 text-[9px] uppercase tracking-wider text-[#9a8872]">
                      <MemoryStick size={12} className="text-lime-300/80" />
                      ОЗУ системы
                    </div>
                    <p className="text-sm font-semibold text-[#e5d9c8]">
                      {memoryStatus ? formatMemory(memoryStatus.totalMib) : "—"}
                    </p>
                  </div>
                  <div className="rounded-md border border-lime-700/40 bg-lime-950/20 p-2.5">
                    <div className="mb-1.5 flex items-center gap-1.5 text-[9px] uppercase tracking-wider text-[#9a8872]">
                      <Cpu size={12} className="text-lime-300/80" />
                      Лимит heap Java
                    </div>
                    <p className="text-sm font-semibold text-lime-200">
                      {!memoryStatus
                        ? "Определяется"
                        : memoryStatus.heapMib === null
                          ? "Недостаточно"
                          : formatMemory(memoryStatus.heapMib)}
                    </p>
                  </div>
                </div>
                {memoryStatus && (
                  <p className="mt-1 text-[8px] text-[#806d57]">
                    Свободно сейчас: {formatMemory(memoryStatus.availableMib)}
                  </p>
                )}

                <div className="mt-3 rounded-md border border-[#49392a]/80 bg-black/20 p-2.5">
                  <div className="mb-2 flex items-center gap-2 text-[9px] uppercase tracking-[0.15em] text-[#9a8872]">
                    <HardDriveIcon />
                    Рабочий каталог
                  </div>
                  <p className="break-all font-mono text-[9px] leading-relaxed text-[#b2a38e]">
                    {launcherState?.installDir ??
                      (launcherState?.driveError
                        ? "Выберите папку установки"
                        : launcherState
                          ? "Автовыбор диска"
                          : "Определяется автоматически")}
                  </p>
                </div>
              </CardContent>
            </Card>

            <Card className="col-span-2 min-h-0 overflow-hidden">
              <CardContent className="px-4 pb-2 pt-3">
                <CardHeader className="mb-1">
                  <CardTitle className="flex items-center gap-2 text-[9px]">
                    <Terminal size={13} className="text-lime-300/80" />
                    Консоль действий
                  </CardTitle>
                  <span className="text-[9px] text-[#66513a]">SESSION / REMOTE</span>
                </CardHeader>
                <div
                  className="space-y-0.5 overflow-hidden font-mono text-[9px] leading-relaxed sm:text-[10px]"
                  aria-live="polite"
                >
                  {logs.slice(-4).map((line, index) => (
                    <p
                      key={`${index}-${line}`}
                      className={
                        line.includes("ERROR")
                          ? "max-h-10 overflow-hidden whitespace-normal break-all text-rose-300"
                          : `truncate ${
                              index === logs.slice(-4).length - 1
                                ? "text-lime-100/90"
                                : "text-[#806d57]"
                            }`
                      }
                      role={line.includes("ERROR") ? "alert" : undefined}
                    >
                      <span className="mr-2 text-lime-500/70">&gt;</span>
                      {hideServerAddress(line)}
                    </p>
                  ))}
                </div>
              </CardContent>
            </Card>
          </div>

          <footer className="flex items-center justify-between gap-3 text-[8px] uppercase tracking-[0.16em] text-[#66513a]">
            <span>1stMay client // remote session</span>
            <span className="flex items-center gap-1.5">
              <span className="size-1 rounded-full bg-emerald-400/70" />
              {!launcherState
                ? "Checking"
                : isInstalled
                  ? "Client installed"
                  : "Install required"}
            </span>
          </footer>
        </div>
      </ScrollArea>
    </main>
  );
}

function HardDriveIcon() {
  return <FolderOpen size={12} className="text-lime-300/70" />;
}

function formatMemory(mib: number) {
  return `${(mib / 1024).toFixed(1)} ГБ`;
}
