import { cn } from "../../lib/utils";

interface ProgressProps {
  value: number;
  className?: string;
  indicatorClassName?: string;
  indeterminate?: boolean;
}

export function Progress({
  value,
  className,
  indicatorClassName,
  indeterminate = false,
}: ProgressProps) {
  const boundedValue = Math.max(0, Math.min(100, value));
  return (
    <div
      className={cn(
        "relative h-1.5 w-full overflow-hidden rounded-full bg-[#403327]",
        className,
      )}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={boundedValue}
    >
      <div
        className={cn(
          "h-full rounded-full bg-[#6cc349] shadow-[0_0_14px_rgba(108,195,73,0.65)] transition-[width] duration-500",
          indeterminate &&
            "absolute left-0 w-2/5 animate-[progress-sweep_1.3s_ease-in-out_infinite]",
          indicatorClassName,
        )}
        style={indeterminate ? undefined : { width: `${boundedValue}%` }}
      />
    </div>
  );
}
