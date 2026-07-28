import { useEffect, useMemo, useRef, useState } from "react";
import { LANGS, type Lang, type Messages } from "./i18n";

// Hand-rolled dropdown for the language setting: a trigger button opening
// an anchored panel with a search field and the language list. The native
// <select> renders as an OS menu in wry's webview and can't be styled or
// searched, so the panel is ours.
export default function LangPicker({
  lang,
  onChange,
  t,
}: {
  lang: Lang;
  onChange: (l: Lang) => void;
  t: Messages;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return LANGS;
    return LANGS.filter(
      (l) =>
        l.native.toLowerCase().includes(q) ||
        Object.values(l.names).some((n) => n.toLowerCase().includes(q)),
    );
  }, [query]);

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
    setActive(Math.max(0, LANGS.findIndex((l) => l.value === lang)));
    setOpen(true);
  };

  const choose = (l: Lang) => {
    onChange(l);
    setOpen(false);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      // Close only the picker — App's window-level Escape would close settings.
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
      const l = filtered[active];
      if (l) choose(l.value);
    }
  };

  const current = LANGS.find((l) => l.value === lang);

  return (
    <div className="lang-picker" ref={rootRef}>
      <button
        className="lang-picker-btn"
        onClick={() => (open ? setOpen(false) : openPicker())}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span>{current?.names[lang] ?? lang}</span>
        <span className="lang-picker-chevron" aria-hidden>
          ⌄
        </span>
      </button>
      {open && (
        <div className="lang-picker-pop" onKeyDown={onKeyDown}>
          <input
            className="lang-picker-search"
            autoFocus
            value={query}
            placeholder={t.searchLanguage}
            onChange={(e) => {
              setQuery(e.target.value);
              setActive(0);
            }}
          />
          <ul className="lang-picker-list" role="listbox" ref={listRef}>
            {filtered.map((l, i) => (
              <li
                key={l.value}
                role="option"
                aria-selected={l.value === lang}
                className={`lang-picker-item ${i === active ? "active" : ""}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => choose(l.value)}
              >
                <span className="lang-native">{l.native}</span>
                {l.names[lang] !== l.native && (
                  <span className="lang-localized">・{l.names[lang]}</span>
                )}
                {l.value === lang && (
                  <span className="lang-check" aria-hidden>
                    ✓
                  </span>
                )}
              </li>
            ))}
            {filtered.length === 0 && (
              <li className="lang-picker-empty">{t.noMatches}</li>
            )}
          </ul>
        </div>
      )}
    </div>
  );
}
