export interface MapPublicationCheck {
  mapExpected: boolean;
  replay?: boolean;
  mapRequested?: boolean;
  itineraryExpected?: boolean;
}

// The listener is scoped to one frame/document, including cancellation on retry or unmount.
export function watchFigureInspection(frame: HTMLIFrameElement, done: (issue: string | null) => void,
  publication?: MapPublicationCheck) {
  let port: MessagePort | undefined;
  const timer = window.setTimeout(onTimeout, 12000);
  const stop = () => {
    window.clearTimeout(timer);
    window.removeEventListener('message', receive);
    port?.close();
  };
  const finish = (issue: string | null) => {
    stop();
    done(issue);
  };
  function onTimeout() {
    finish('The visualization renderer did not finish loading.');
  }
  function receive(e: MessageEvent) {
    if (
      e.source !== frame.contentWindow ||
      e.data?.openBitFunVisual !== 2 ||
      e.data.type !== 'inspect-render' ||
      !e.ports[0] ||
      port
    )
      return;
    port = e.ports[0];
    port.onmessage = ({ data }) => {
      if (data === null || typeof data === 'string') finish(data);
    };
    port.postMessage(publication ?? 'inspect');
  }
  window.addEventListener('message', receive);
  return stop;
}
