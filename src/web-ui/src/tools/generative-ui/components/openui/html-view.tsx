import { defineComponent, useIsStreaming } from '@openuidev/react-lang';
import { useMemo, useRef } from 'react';
import { z } from 'zod/v4';
import { useColorScheme } from '@/infrastructure/appearance/hooks/useHostAppearance';
import { sanitizeStaticHtml } from '@/shared/ui/static-document/sanitizeStaticHtml';

const MIN_HEIGHT = 80;
const MAX_HEIGHT = 2400;

export const HtmlView = defineComponent({
  name: "HtmlView",
  props: z.object({
    html: z.string(),
    height: z.number().optional(),
    title: z.string().optional(),
  }),
  description: "Read-only HTML from a saved message.",
  component: function HtmlViewView({ props }) {
    const streaming = useIsStreaming();
    const box = useRef<HTMLDivElement>(null);
    const scheme = useColorScheme(box);
    const fixed = typeof props.height === "number" && props.height > 0 ? Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, props.height)) : null;
    const html = typeof props.html === "string" ? props.html : "";
    const srcDoc = useMemo(() => sanitizeStaticHtml({ html }, undefined, '', scheme).document, [html, scheme]);
    const height = fixed ?? 360;
    return (
      <div ref={box} className="iui-html-view" style={{ height }}>
        {streaming ? (
          <div className="iui-html-view-wait">Preparing the interactive view…</div>
        ) : (
          <iframe title={props.title || "Interactive view"} sandbox="" srcDoc={srcDoc} referrerPolicy="no-referrer" loading="lazy" />
        )}
      </div>
    );
  },
});
