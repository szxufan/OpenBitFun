const WORD_BOUNDARY = /[\s._/\\:-]+/u;
const CJK = /[\u3400-\u9fff]/u;

function normalize(value) {
  return String(value ?? '').toLocaleLowerCase().trim();
}

export function queryTokens(query) {
  return normalize(query).split(WORD_BOUNDARY)
    .filter((token) => token.length >= 2 || CJK.test(token));
}

function scoreSingleTerm(query, field) {
  if (field === query) return 100;
  if (field.startsWith(query)) return 94;
  const words = field.split(WORD_BOUNDARY).filter(Boolean);
  if (words.some((word) => word.startsWith(query))) return 88;
  if (field.includes(query)) return 80;
  const acronym = words.map((word) => word[0]).join('');
  // Match consecutive initials; arbitrary skipped words make long prose
  // spuriously match unrelated capability names.
  return query.length > 1 && acronym.includes(query) ? 66 : 0;
}

export function scoreTextMatch(query, fields) {
  const normalizedQuery = normalize(query);
  if (!normalizedQuery) return 70;
  const normalizedFields = fields.map(normalize).filter(Boolean);
  let best = Math.max(0, ...normalizedFields.map((field) =>
    scoreSingleTerm(normalizedQuery, field)));
  const tokens = queryTokens(normalizedQuery);
  if (tokens.length > 1) {
    const tokenScores = tokens.map((token) => Math.max(
      0,
      ...normalizedFields.map((field) => scoreSingleTerm(token, field)),
    ));
    if (tokenScores.every((value) => value > 0)) best = Math.max(best, 72);
    const matched = tokenScores.filter((value) => value > 0);
    if (matched.length) {
      const coverage = matched.length / tokens.length;
      best = Math.max(best, Math.round(38 + coverage * 28 + Math.max(...matched) * 0.08));
    }
  }
  return best;
}

export function scoreCapability(capability, query) {
  if (!normalize(query)) return 1;
  const primaryScore = scoreTextMatch(query, [
    capability.id,
    capability.titleZh,
    capability.titleEn,
    ...(capability.searchTerms ?? []),
  ]);
  const directBonus = Math.min(8, (capability.operations?.length ?? 0) + (capability.options?.length ?? 0));
  const delegatedBonus = capability.items?.some(({ control }) => control?.kind === 'delegate') ? 4 : 0;
  if (primaryScore > 0) {
    // Match the native and Web UI catalogs: control coverage breaks textual ties.
    return primaryScore + (directBonus + delegatedBonus) / 100;
  }

  // Secondary human-prose fallback so searching phrases from summaries, scenarios,
  // tips, entry paths, or item descriptions still surfaces relevant guides without
  // overtaking curated primary title/keyword matches.
  const proseFields = [
    capability.summaryZh,
    capability.summaryEn,
    capability.entryPathZh,
    capability.entryPathEn,
    ...(capability.scenariosZh ?? []),
    ...(capability.scenariosEn ?? []),
    ...(capability.tipsZh ?? []),
    ...(capability.tipsEn ?? []),
    ...(capability.items ?? []).flatMap((item) => [item.descriptionZh, item.descriptionEn]),
  ].filter(Boolean);
  const proseScore = scoreTextMatch(query, proseFields);
  if (!proseScore) return 0;
  return Math.min(36, Math.round(proseScore * 0.4)) + (directBonus + delegatedBonus) / 100;
}

export function matchingItems(capability, query) {
  if (!normalize(query)) return [];
  const tokens = queryTokens(query);
  return capability.items
    .map((item) => {
      const directScore = scoreTextMatch(query, [item.titleZh, item.titleEn]);
      const partialScore = Math.max(
        0,
        ...tokens.map((token) => scoreTextMatch(token, [item.titleZh, item.titleEn])),
      );
      let rank = directScore > 0 ? 100 + directScore : Math.floor(partialScore / 2);
      if (rank === 0 && (item.descriptionZh || item.descriptionEn)) {
        const descScore = scoreTextMatch(query, [item.descriptionZh, item.descriptionEn]);
        if (descScore > 0) rank = Math.min(24, Math.floor(descScore / 4));
      }
      return { item, rank };
    })
    .filter(({ rank }) => rank > 0)
    .sort((left, right) => right.rank - left.rank)
    .map(({ item }) => item);
}

export function searchCapabilities(catalog, query) {
  return catalog.capabilities
    .map((capability) => ({ capability, rank: scoreCapability(capability, query) }))
    .filter(({ rank }) => rank > 0)
    .sort((left, right) => right.rank - left.rank
      || (left.capability.order ?? 0) - (right.capability.order ?? 0)
      || left.capability.kind.localeCompare(right.capability.kind)
      || left.capability.id.localeCompare(right.capability.id));
}
