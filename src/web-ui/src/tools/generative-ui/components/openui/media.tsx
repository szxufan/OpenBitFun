"use client";

import { defineComponent } from "@openuidev/react-lang";
import { ImageIcon } from "lucide-react";
import { useState } from "react";
import { z } from "zod/v4";
import { Glyph } from "./glyph";
import { color, cx, fg, radius, radiusSchema, webUrl } from "./style";

const ICON_SIZE = { sm: 14, md: 18, lg: 24, xl: 32 } as const;

export const Icon = defineComponent({
  name: "Icon",
  props: z.object({
    name: z.string(),
    size: z.enum(["sm", "md", "lg", "xl"]).optional(),
    color: color.optional(),
  }),
  description: 'Lucide icon by name, e.g. "map-pin".',
  component: ({ props }) => (
    <span className="iui-icon" style={{ color: fg(props.color) }}>
      <Glyph name={props.name} size={ICON_SIZE[props.size ?? "md"] ?? 18} />
    </span>
  ),
});

type PhotoProps = { query: string; src?: string | null; aspectRatio?: number | null; radius?: unknown; fit?: string | null };

/** Only a URL recorded in the program can supply an archived photo. */
function Photo(props: PhotoProps) {
  const alt = typeof props.query === "string" ? props.query.trim() : "";
  const src = webUrl(props.src);
  const [failed, setFailed] = useState<string>();
  const [loaded, setLoaded] = useState<string | null>(null);
  const current = src !== failed ? src : undefined;
  const ratio = typeof props.aspectRatio === "number" && props.aspectRatio > 0.2 && props.aspectRatio < 5 ? props.aspectRatio : 1.5;
  return (
    <div
      className={cx("iui-image", props.fit === "contain" && "is-contain", loaded === current && "is-loaded")}
      style={{ aspectRatio: ratio, borderRadius: radius(props.radius ?? "lg") }}
      role={current ? undefined : "img"}
      aria-label={current ? undefined : alt}
    >
      {current ? (
        <img
          key={current}
          src={current}
          alt={alt}
          loading="lazy"
          referrerPolicy="no-referrer"
          onLoad={() => setLoaded(current)}
          onError={() => setFailed(current)}
        />
      ) : (
        <span><ImageIcon aria-hidden className="iui-image-empty" strokeWidth={1.5} /> {alt}</span>
      )}
    </div>
  );
}

const fit = z.enum(["cover", "contain"]).optional();

export const SearchImage = defineComponent({
  name: "SearchImage",
  props: z.object({ query: z.string(), aspectRatio: z.number().optional(), radius: radiusSchema.optional(), fit }),
  description: "Image search label recorded in a saved message.",
  component: ({ props }) => <Photo {...props} />,
});

export const Image = defineComponent({
  name: "Image",
  props: z.object({ src: z.string(), alt: z.string(), aspectRatio: z.number().optional(), radius: radiusSchema.optional(), fit }),
  description: "Photo URL and alternative text recorded in a saved message.",
  component: ({ props }) => <Photo query={props.alt} src={props.src} aspectRatio={props.aspectRatio} radius={props.radius} fit={props.fit} />,
});
