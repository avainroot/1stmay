import { useEffect, useRef } from "react";
import { RotateCcw } from "lucide-react";
import { IdleAnimation, SkinViewer as SkinViewer3D } from "skinview3d";
import { Button } from "./ui/button";

export type SkinModel = "default" | "slim";
export type TextureType = "skins" | "capes" | "elytras";

const VIEWER_WIDTH = 360;
const VIEWER_HEIGHT = 480;

function createNeutralSkinTexture() {
  const texture = document.createElement("canvas");
  texture.width = 64;
  texture.height = 64;
  const context = texture.getContext("2d");
  if (!context) throw new Error("Не удалось создать нейтральную текстуру персонажа.");

  const base = "#9a8a7d";
  const skin = "#d9c5a1";
  const shirt = "#4d6d8b";
  const pants = "#5b4b3d";

  context.fillStyle = base;
  context.fillRect(0, 0, texture.width, texture.height);

  context.fillStyle = skin;
  context.fillRect(16, 8, 32, 16);
  context.fillRect(20, 24, 8, 8);
  context.fillRect(36, 24, 8, 8);
  context.fillRect(16, 32, 8, 18);
  context.fillRect(40, 32, 8, 18);
  context.fillRect(24, 32, 16, 18);

  context.fillStyle = shirt;
  context.fillRect(20, 24, 24, 12);

  context.fillStyle = pants;
  context.fillRect(20, 50, 8, 10);
  context.fillRect(36, 50, 8, 10);

  context.fillStyle = "#000";
  context.fillRect(25, 12, 4, 4);
  context.fillRect(35, 12, 4, 4);
  context.fillRect(26, 18, 12, 2);

  return texture;
}

interface SkinViewerProps {
  source: string | null;
  backSource: string | null;
  backEquipment: "cape" | "elytra" | null;
  model: SkinModel;
  onError: (message: string) => void;
  onModelDetected: (model: SkinModel) => void;
}

export function SkinViewer({
  source,
  backSource,
  backEquipment,
  model,
  onError,
  onModelDetected,
}: SkinViewerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<SkinViewer3D | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const viewer = new SkinViewer3D({
      canvas,
      width: VIEWER_WIDTH,
      height: VIEWER_HEIGHT,
      pixelRatio: 1,
      fov: 36,
      zoom: 0.9,
      animation: new IdleAnimation(),
    });
    viewerRef.current = viewer;
    viewer.background = 0x17130f;
    viewer.globalLight.intensity = 2.2;
    viewer.cameraLight.intensity = 0.7;
    viewer.controls.enableRotate = true;
    viewer.controls.enableZoom = false;
    viewer.controls.enablePan = false;
    viewer.controls.rotateSpeed = 0.8;
    viewer.controls.enableDamping = true;

    return () => {
      viewer.dispose();
      viewerRef.current = null;
    };
  }, []);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;

    try {
      if (source === null) {
        viewer.loadSkin(createNeutralSkinTexture(), {
          model: viewer.playerObject.skin.modelType,
        });
        return;
      }

      let active = true;
      const loading = viewer.loadSkin(source, { model: "auto-detect" });
      if (loading instanceof Promise) {
        void loading
          .then(() => {
            if (active) {
              onModelDetected(viewer.playerObject.skin.modelType);
            }
          })
          .catch((reason: unknown) => {
            if (active) {
              onError(reason instanceof Error ? reason.message : String(reason));
            }
          });
      }
      return () => {
        active = false;
      };
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : String(reason));
    }
  }, [onError, onModelDetected, source]);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (!viewer) return;

    let active = true;
    if (backSource === null || backEquipment === null) {
      viewer.loadCape(null);
      return;
    }

    void viewer.loadCape(backSource, { backEquipment }).catch((reason: unknown) => {
      if (active) {
        onError(reason instanceof Error ? reason.message : String(reason));
      }
    });
    return () => {
      active = false;
    };
  }, [backEquipment, backSource, onError]);

  useEffect(() => {
    const viewer = viewerRef.current;
    if (viewer) {
      viewer.playerObject.skin.modelType = model;
    }
  }, [model]);

  return (
    <div className="relative flex min-h-[260px] min-w-0 flex-1 items-center justify-center overflow-hidden rounded-md border border-[#49392a]/80 bg-[#17130f]">
      <div className="relative aspect-[9/11] w-full max-w-[360px]">
        <canvas
          ref={canvasRef}
          className="absolute inset-0 !size-full touch-none"
          aria-label="3D-предпросмотр скина Minecraft"
        />
      </div>
      {!source && (
        <span className="pointer-events-none absolute bottom-6 left-2 text-[8px] uppercase tracking-widest text-[#9a8872]/70">
          Нейтральная модель
        </span>
      )}
      <Button
        variant="ghost"
        size="icon"
        className="absolute right-2 top-2 size-8 bg-black/40"
        onClick={() => viewerRef.current?.resetCameraPose()}
        aria-label="Сбросить положение модели"
        title="Сбросить положение модели"
      >
        <RotateCcw size={14} />
      </Button>
      <span className="pointer-events-none absolute bottom-2 left-2 text-[8px] uppercase tracking-widest text-[#9a8872]/70">
        Перетаскивайте мышью для вращения
      </span>
    </div>
  );
}
