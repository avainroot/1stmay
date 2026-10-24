import type { InputHTMLAttributes } from "react";
import { cn } from "../../lib/utils";

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "h-11 w-full rounded-md border border-[#66513a]/90 bg-[#17130f] px-3.5 text-sm text-[#f0e9dd] outline-none placeholder:text-slate-600 transition-colors focus:border-lime-400/70 focus:ring-2 focus:ring-lime-400/10 disabled:opacity-50",
        className,
      )}
      {...props}
    />
  );
}
