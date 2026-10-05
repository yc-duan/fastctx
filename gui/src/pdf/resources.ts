import resources from "virtual:world-pdf/resources";

async function bytes(name: string): Promise<Uint8Array> {
  const load = resources[name];
  if (!load) throw new Error(`Unknown PDF resource: ${name}`);
  const { default: base64 } = await load();
  return Uint8Array.from(atob(base64), (character) => character.charCodeAt(0));
}

export class CMapReaderFactory {
  async fetch({ name }: { name: string }): Promise<{ cMapData: Uint8Array; isCompressed: true }> {
    return { cMapData: await bytes(`cmaps/${name}.bcmap`), isCompressed: true };
  }
}

export class StandardFontDataFactory {
  fetch({ filename }: { filename: string }): Promise<Uint8Array> {
    return bytes(`standard_fonts/${filename}`);
  }
}
