import { useEffect, useMemo, useRef, useState } from "react";

export interface PickerOption<V extends string> {
  value: V;
  primary: string;
  secondary?: string;
  // Extra strings the search matches besides primary/secondary.
  keywords?: string[];
}

// Hand-rolled dropdown for settings rows: a trigger button opening an
// anchored panel with an optional search field and the option list. The
// native <select> renders as an OS menu in wry's webview and can't be
// styled or searched, so the panel is ours.
export default function Picker<V extends string>({
  value,
  options,
  onChange,
  searchPlaceholder,
  noMatchesLabel,
}: {
  value: V;
  options: PickerOption<V>[];
  onChange: (v: V) => void;
  // Present = show the search field.
  searchPlaceholder?: string;
  noMatchesLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return options;
    return options.filter((o) =>
      [o.primary, o.secondary ?? "", ...(o.keywords ?? [])].some((s) =>
        s.toLowerCase().includes(q),
      ),
    );
  }, [query, options]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  useEffect(() => {
    listRef.current?.children[active]?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const openPicker = () => {
    setQuery("");
    setActive(
      Math.max(
        0,
        options.findIndex((o) => o.value === value),
      ),
    );
    setOpen(true);
  };

  const choose = (v: V) => {
    onChange(v);
    setOpen(false);
  };

  // Keys are taken off window in the CAPTURE phase rather than from a handler
  // inside the popup: WebKit doesn't focus a <button> when it is clicked, so
  // with no search field to autofocus the keydowns land on <body> and never
  // pass through the picker's subtree at all. Capturing also means Escape
  // closes just this popup — App's window-level Escape, which would close the
  // whole settings view, never sees the event.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        setActive((a) => Math.min(a + 1, filtered.length - 1));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setActive((a) => Math.max(a - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        const o = filtered[active];
        if (o) {
          onChange(o.value);
          setOpen(false);
        }
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, filtered, active, onChange]);

  const current = options.find((o) => o.value === value);

  return (
    <div className="picker" ref={rootRef}>
      <button
        className="picker-btn"
        onClick={() => (open ? setOpen(false) : openPicker())}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span>{current?.primary ?? value}</span>
        <span className="picker-chevron" aria-hidden>
          ⌄
        </span>
      </button>
      {open && (
        <div className="picker-pop">
          {searchPlaceholder !== undefined && (
            <input
              className="picker-search"
              autoFocus
              value={query}
              placeholder={searchPlaceholder}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(0);
              }}
            />
          )}
          <ul className="picker-list" role="listbox" ref={listRef}>
            {filtered.map((o, i) => (
              <li
                key={o.value}
                role="option"
                aria-selected={o.value === value}
                className={`picker-item ${i === active ? "active" : ""}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => choose(o.value)}
              >
                <span className="picker-primary">{o.primary}</span>
                {o.secondary && (
                  <span className="picker-secondary">・{o.secondary}</span>
                )}
                {o.value === value && (
                  <span className="picker-check" aria-hidden>
                    ✓
                  </span>
                )}
              </li>
            ))}
            {filtered.length === 0 && (
              <li className="picker-empty">{noMatchesLabel}</li>
            )}
          </ul>
        </div>
      )}
    </div>
  );
}
