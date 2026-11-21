"use client";

import { useEffect, useState, type RefObject } from "react";

const appearanceAttributes = [
  "data-openbitfun-appearance-revision",
  "data-openbitfun-appearance-mode",
  "data-openbitfun-skin-id",
  "data-color-scheme",
  "data-contrast",
];

/** A new skin or Appearance revision can change scoped tokens within the same color scheme. */
export function useHostAppearance(ref: RefObject<HTMLElement | null>) {
  const [appearance, setAppearance] = useState({ scheme: "light" as "light" | "dark", revision: 0 });
  useEffect(() => {
    const read = () => {
      const value = getComputedStyle(ref.current ?? document.documentElement).colorScheme;
      setAppearance(previous => ({
        scheme: value.trim() === "dark" ? "dark" : "light",
        revision: previous.revision + 1,
      }));
    };
    read();
    const observer = new MutationObserver(read);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: appearanceAttributes });
    return () => observer.disconnect();
  }, [ref]);
  return appearance;
}

export function useColorScheme(ref: RefObject<HTMLElement | null>) {
  return useHostAppearance(ref).scheme;
}
