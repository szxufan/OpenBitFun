import type { GlobalSearchScope } from './types';

const WORD_BOUNDARY = /[\s._/\\:-]+/u;
const CJK = /[\u3400-\u9fff]/u;

export interface ParsedGlobalSearchQuery {
  query: string;
  scope: GlobalSearchScope;
  scopeForcedByPrefix: boolean;
}
export function parseGlobalSearchQuery(
  rawQuery: string,
  selectedScope: GlobalSearchScope,
): ParsedGlobalSearchQuery {
  const trimmed = rawQuery.trim();
  if (trimmed.startsWith('>')) {
    return {
      query: trimmed.slice(1).trim(),
      scope: 'actions',
      scopeForcedByPrefix: true,
    };
  }
  return { query: trimmed, scope: selectedScope, scopeForcedByPrefix: false };
}

const tokensCache = new Map<string, string[]>();

function queryTokens(query: string): string[] {
  const cached = tokensCache.get(query);
  if (cached) return cached;
  const tokens = query
    .split(WORD_BOUNDARY)
    .map(token => token.trim())
    .filter(token => token.length >= 2 || CJK.test(token));
  if (tokensCache.size > 256) tokensCache.clear();
  tokensCache.set(query, tokens);
  return tokens;
}

const wordsCache = new Map<string, { words: string[]; acronym: string }>();

function getFieldWords(field: string): { words: string[]; acronym: string } {
  const cached = wordsCache.get(field);
  if (cached) return cached;
  const words = field.split(WORD_BOUNDARY).filter(Boolean);
  const acronym = words.map(word => word[0]).join('');
  const entry = { words, acronym };
  if (wordsCache.size > 512) wordsCache.clear();
  wordsCache.set(field, entry);
  return entry;
}

const normalizedFieldCache = new Map<string, string>();

/** Trims and lowercases a field once. Every row is scored again on the next keystroke. */
function normalizeField(rawField: string | null | undefined): string {
  if (!rawField) return '';
  const cached = normalizedFieldCache.get(rawField);
  if (cached !== undefined) return cached;
  const normalized = rawField.trim().toLocaleLowerCase();
  // Empty results are not worth a cache slot: whitespace padding is free to re-trim.
  if (normalized) {
    if (normalizedFieldCache.size > 1024) normalizedFieldCache.clear();
    normalizedFieldCache.set(rawField, normalized);
  }
  return normalized;
}

function scoreSingleTerm(query: string, field: string): number {
  if (field === query) return 100;
  if (field.startsWith(query)) return 94;

  const { words, acronym } = getFieldWords(field);
  if (words.some(word => word.startsWith(query))) return 88;
  if (field.includes(query)) return 80;

  // Require consecutive initials so long workflow prose does not gain
  // unrelated capability-name matches from arbitrary skipped words.
  if (query.length > 1 && acronym.includes(query)) return 66;
  return 0;
}

/**
 * Small deterministic matcher shared by in-memory providers.
 * Backend content providers keep ownership of their own relevance scores.
 */
export function scoreTextMatch(query: string, fields: Array<string | null | undefined>): number {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  if (!normalizedQuery) return 70;

  const tokens = queryTokens(normalizedQuery);
  const multiToken = tokens.length > 1;

  let best = 0;
  let normalizedFields: string[] | undefined;
  for (const rawField of fields) {
    const field = normalizeField(rawField);
    if (!field) continue;
    // Only the multi-token branches read this list, so single-token queries (the common case)
    // no longer allocate an array per row.
    if (multiToken) (normalizedFields ??= []).push(field);
    best = Math.max(best, scoreSingleTerm(normalizedQuery, field));

    if (multiToken && tokens.every(token => field.includes(token))) {
      best = Math.max(best, 74);
    }
  }

  if (
    multiToken
    && tokens.every(token => (normalizedFields ?? []).some(field => field.includes(token)))
  ) {
    best = Math.max(best, 72);
  }

  // Natural-language discovery queries often contain bilingual alternatives or
  // near-synonyms (for example "pet companion mascot"). Treat those tokens as
  // evidence that accumulates instead of an all-or-nothing AND expression.
  // Exact/full-query matches above still outrank partial token coverage.
  if (multiToken) {
    const matchedScores: number[] = [];
    for (const token of tokens) {
      let strongest = 0;
      // A loop instead of Math.max(...fields.map(...)): the spread allocates an argument list
      // per token, and this runs once per candidate row per keystroke.
      for (const field of normalizedFields ?? []) {
        const score = scoreSingleTerm(token, field);
        if (score > strongest) strongest = score;
      }
      if (strongest > 0) matchedScores.push(strongest);
    }
    if (matchedScores.length > 0) {
      const coverage = matchedScores.length / tokens.length;
      const strongest = Math.max(...matchedScores);
      best = Math.max(best, Math.round(38 + coverage * 28 + strongest * 0.08));
    }
  }
  return best;
}
