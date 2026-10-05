import "pdfjs-dist/legacy/build/pdf.worker.mjs";

// Decode base64 or detach an IPC header away from the UI thread. PDF.js
// otherwise copies subarrays synchronously before transferring them.
self.addEventListener("message", (event: MessageEvent) => {
  if (event.data?.type !== "world-pdf-data") return;
  try {
    let bytes: Uint8Array<ArrayBuffer>;
    if (event.data.bytes instanceof Uint8Array) {
      bytes = event.data.bytes.slice();
    } else {
      const binary = atob(event.data.base64);
      bytes = new Uint8Array(binary.length);
      for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
    }
    self.postMessage({ type: "world-pdf-bytes", bytes }, { transfer: [bytes.buffer] });
  } catch (error) {
    self.postMessage({ type: "world-pdf-bytes", error: String(error) });
  }
});
