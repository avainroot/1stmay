import { cva, type VariantProps } from "class-variance-authority";
import type { ButtonHTMLAttributes } from "react";
import { cn } from "../../lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-xs font-semibold uppercase tracking-[0.12em] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-lime-400/70 disabled:pointer-events-none disabled:opacity-45",
  {
    variants: {
      variant: {
        primary:
          "border border-lime-300/50 bg-[#6cc349] text-[#17140f] shadow-[0_0_26px_rgba(108,195,73,0.16)] hover:bg-[#86d562]",
        outline:
          "border border-[#66513a] bg-[#17130f]/80 text-[#d0c4b3] hover:border-lime-500/50 hover:text-lime-200",
        ghost: "text-[#9a8872] hover:bg-[#49392a]/60 hover:text-[#e5d9c8]",
      },
      size: {
        default: "h-10 px-4",
        sm: "h-8 px-3 text-[10px]",
        icon: "size-9",
        play: "h-12 w-full px-5 text-sm",
      },
    },
    defaultVariants: {
      variant: "primary",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {}

export function Button({ className, variant, size, ...props }: ButtonProps) {
  return (
    <button className={cn(buttonVariants({ variant, size }), className)} {...props} />
  );
}
