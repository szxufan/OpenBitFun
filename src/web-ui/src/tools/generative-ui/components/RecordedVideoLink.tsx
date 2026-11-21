/** Recorded media stays accessible without starting a third-party player during replay. */
export function RecordedVideoLink({ video, title }: { video: unknown; title?: unknown }) {
  if (typeof video !== 'string') return null;
  const match = video.trim().match(/(?:youtu\.be\/|[?&]v=|\/embed\/|\/shorts\/)([\w-]{11})|^([\w-]{11})$/);
  const id = match?.[1] ?? match?.[2];
  if (!id) return null;
  return <a href={`https://www.youtube.com/watch?v=${id}`} target="_blank" rel="noreferrer">
    {typeof title === 'string' && title.trim() ? title : 'YouTube'}
  </a>;
}
