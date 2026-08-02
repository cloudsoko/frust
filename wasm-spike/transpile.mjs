import { transpile, writeFiles } from "@bytecodealliance/jco-transpile";

const [componentPath, outDir] = process.argv.slice(2);
if (!componentPath || !outDir) {
  throw new Error("usage: node transpile.mjs <component.wasm> <output-directory>");
}

const { files } = await transpile(componentPath, { outDir });
await writeFiles(files);
