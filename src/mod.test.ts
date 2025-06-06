import denoPlugin from "./mod.ts";
import { assertEquals } from "@std/assert";

Deno.test("should load and resolve", async () => {
  const plugin = denoPlugin();
  await plugin.buildStart({
    input: import.meta.url,
  });
  const value = await plugin.resolveId("./mod.ts", import.meta.url, {
    kind: "import-statement",
  });
  assertEquals(value, import.meta.resolve("./mod.ts"));
  const text = await plugin.load(value);
  assertEquals(text, Deno.readTextFileSync(new URL(value)));
});
