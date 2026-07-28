import { useRef, useState } from "react";

export type WidthSpec = { def: number; min: number; max: number };

export default function ResizeHandle({
  className,
  label,
  width,
  spec,
  dir,
  onWidth,
  onResizing,
}: {
  className: string;
  label: string;
  width: number;
  spec: WidthSpec;
  // +1: dragging right widens the panel (left sidebar);
  // -1: dragging right narrows it (right detail pane).
  dir: 1 | -1;
  onWidth: (w: number) => void;
  onResizing: (active: boolean) => void;
}) {
  const start = useRef<{ x: number; w: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  const end = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    start.current = null;
    setDragging(false);
    onResizing(false);
    e.currentTarget.releasePointerCapture(e.pointerId);
  };

  return (
    <div
      className={`pane-resizer ${className} ${dragging ? "active" : ""}`}
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      title={label}
      aria-valuenow={width}
      aria-valuemin={spec.min}
      aria-valuemax={spec.max}
      tabIndex={0}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        // Capture keeps move events on the handle even over the xterm canvas.
        e.currentTarget.setPointerCapture(e.pointerId);
        start.current = { x: e.clientX, w: width };
        setDragging(true);
        onResizing(true);
      }}
      onPointerMove={(e) => {
        if (!start.current) return;
        onWidth(start.current.w + (e.clientX - start.current.x) * dir);
      }}
      onPointerUp={end}
      onPointerCancel={end}
      onDoubleClick={() => onWidth(spec.def)}
      onKeyDown={(e) => {
        // Arrows move the divider itself, whichever side the panel is on.
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          onWidth(width - 16 * dir);
        } else if (e.key === "ArrowRight") {
          e.preventDefault();
          onWidth(width + 16 * dir);
        }
      }}
    />
  );
}
