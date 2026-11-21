import patterns from './geographicRequestPatterns.json';
import type { MapPublicationCheck } from './figureInspection';

const geographicRequests = Object.values(patterns).map(({ map, schematic, itinerary, otherDeliverable }) => ({
  map: new RegExp(map, 'iu'), schematic: new RegExp(schematic, 'iu'),
  itinerary: new RegExp(itinerary, 'iu'), otherDeliverable: new RegExp(otherDeliverable, 'iu'),
}));

export async function mapPublication(uri: string, userRequest: string | null | undefined, html: string): Promise<MapPublicationCheck> {
  const message = userRequest?.trim() ?? '';
  const first = message.split(/[。！？.!?\n]/u, 1)[0];
  let geographic = geographicRequests.some(({ map, schematic }) => map.test(first) && !schematic.test(first));
  const schematic = geographicRequests.some(pattern => pattern.schematic.test(first));
  const subject = first.match(/^(?:please\s+)?(?:draw|show|make|create)\s+(?:me\s+)?(?:(?:a|the)\s+)?map of (.+?)(?:[,;]|\s+and\b|$)/iu)?.[1].trim();
  if (subject && !schematic && !geographic) {
    const { isAtlasPlace } = await import('./regionalAtlas');
    geographic = await isAtlasPlace(subject);
  }
  const ambiguous = !!subject && !geographic;
  const fragment = document.createElement('template');
  fragment.innerHTML = html;
  const declared = !!fragment.content.querySelector('[data-visual-map],script[data-visual-atlas]');
  const mapArtifact = declared || (!ambiguous && !schematic && /\/map-[^/]+\.html$/.test(uri));
  const multipleOutputs = geographicRequests.some(pattern => pattern.otherDeliverable.test(message));
  const mapRequested = geographic && (mapArtifact || !multipleOutputs);
  const headings = Array.from(fragment.content.querySelectorAll('h1,h2,h3'), el => el.textContent).join(' ');
  return {
    mapExpected: mapArtifact || mapRequested,
    mapRequested,
    itineraryExpected: declared && geographicRequests.some(pattern => pattern.itinerary.test(message + ' ' + headings)),
  };
}
