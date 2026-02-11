import { CircleHelp, ExternalLink } from "lucide-react";
import { HoverCard } from "radix-ui";
import { cn } from "~/utils/common";

const DOCS_BASE_URL = "https://www.tensorzero.com/docs";

interface HelpTooltipProps {
  children: React.ReactNode;
  /** Path appended to docs base URL (e.g. "gateway/guides/episodes") */
  docsPath?: string;
  side?: "top" | "bottom" | "left" | "right";
}

export function HelpTooltip({
  children,
  docsPath,
  side = "top",
}: HelpTooltipProps) {
  return (
    <HoverCard.Root openDelay={250} closeDelay={300}>
      <HoverCard.Trigger asChild>
        <span className="inline-flex cursor-help">
          <CircleHelp className="text-muted-foreground h-3.5 w-3.5 shrink-0" />
        </span>
      </HoverCard.Trigger>
      <HoverCard.Portal>
        <HoverCard.Content
          side={side}
          sideOffset={4}
          className={cn(
            "bg-popover text-popover-foreground z-50 max-w-xs rounded-md border p-3 shadow-md",
            "data-[state=open]:animate-in data-[state=closed]:animate-out",
            "data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
            "data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95",
            "data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2",
            "data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2",
          )}
        >
          <p className="text-sm leading-relaxed">{children}</p>
          {docsPath && (
            <a
              href={`${DOCS_BASE_URL}/${docsPath}`}
              target="_blank"
              rel="noopener noreferrer"
              className="text-muted-foreground hover:text-foreground mt-2 inline-flex items-center gap-1 text-xs transition-colors"
            >
              View docs
              <ExternalLink className="h-3 w-3" />
            </a>
          )}
        </HoverCard.Content>
      </HoverCard.Portal>
    </HoverCard.Root>
  );
}
