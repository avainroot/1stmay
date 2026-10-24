import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area";
import type { HTMLAttributes } from "react";
import { cn } from "../../lib/utils";

export function ScrollArea({
  className,
  children,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  return (
    <ScrollAreaPrimitive.Root
      className={cn("relative min-h-0 overflow-hidden", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Viewport className="size-full overflow-auto rounded-[inherit] outline-none">
        {children}
      </ScrollAreaPrimitive.Viewport>
      <ScrollAreaPrimitive.Scrollbar
        orientation="vertical"
        className="absolute right-0 top-0 flex h-full w-2.5 touch-none p-[2px] transition-colors"
      >
        <ScrollAreaPrimitive.Thumb className="relative flex-1 rounded-full bg-[#806d57]/70 transition-colors hover:bg-lime-300/80" />
      </ScrollAreaPrimitive.Scrollbar>
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  );
}
