import React, { useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { markdownGfmOptions } from './markdownGfmOptions';
import { remarkAutolinkBoundaries } from './remarkAutolinkBoundaries';
import { remarkStreamingTableLinks } from './remarkStreamingTableLinks';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import type { Options as RehypeSanitizeOptions } from 'rehype-sanitize';
import type { Pluggable } from 'unified';
import 'katex/dist/katex.min.css';
import { rehypeSourceRange, type MarkdownSourceRange } from './rehypeSourceRange';
import { rehypeWindowsDrivePaths } from './rehypeWindowsDrivePaths';
import { rehypeImageThumbnails } from './rehypeImageThumbnails';

function hasRawHastNode(node: any): boolean {
  if (!node || typeof node !== 'object') return false;
  if (node.type === 'raw') return true;
  const children = node.children;
  if (Array.isArray(children)) {
    for (let i = 0; i < children.length; i += 1) {
      if (hasRawHastNode(children[i])) return true;
    }
  }
  return false;
}

function rehypeRawIfPresent() {
  const runRehypeRaw = rehypeRaw();
  return (tree: any, file: any) => (hasRawHastNode(tree) ? runRehypeRaw(tree, file) : tree);
}

interface MarkdownMathRendererProps {
  markdownContent: string;
  isStreaming?: boolean;
  components: Components;
  sanitizeSchema: RehypeSanitizeOptions;
  remarkAutolinkComputerFileLinks: Pluggable;
  urlTransform: (value: string) => string;
  sourceRange?: MarkdownSourceRange;
  inline?: boolean;
  imageThumbnails?: boolean;
}

export const MarkdownMathRenderer: React.FC<MarkdownMathRendererProps> = ({
  markdownContent,
  isStreaming = false,
  components,
  sanitizeSchema,
  remarkAutolinkComputerFileLinks,
  urlTransform,
  sourceRange,
  inline = false,
  imageThumbnails = false,
}) => {
  const remarkPlugins = useMemo(
    () => [
      [remarkGfm, markdownGfmOptions],
      remarkMath,
      [remarkStreamingTableLinks, { isStreaming }],
      remarkAutolinkBoundaries,
      remarkAutolinkComputerFileLinks,
    ] as any,
    [isStreaming, remarkAutolinkComputerFileLinks],
  );
  const rehypePlugins = useMemo(
    () => [
      rehypeRawIfPresent,
      [rehypeWindowsDrivePaths, { markdownSource: markdownContent }],
      [rehypeSanitize, sanitizeSchema],
      [rehypeSourceRange, sourceRange],
      rehypeKatex,
      [rehypeImageThumbnails, { enabled: imageThumbnails }],
    ] as any,
    [markdownContent, sanitizeSchema, sourceRange, imageThumbnails],
  );
  const content = (
    <ReactMarkdown
      remarkPlugins={remarkPlugins}
      rehypePlugins={rehypePlugins}
      urlTransform={urlTransform}
      components={components}
    >
      {markdownContent}
    </ReactMarkdown>
  );
  return inline
    ? <span data-openbitfun-component="markdown" data-openbitfun-part="math">{content}</span>
    : <div data-openbitfun-component="markdown" data-openbitfun-part="math">{content}</div>;
};

export default MarkdownMathRenderer;
