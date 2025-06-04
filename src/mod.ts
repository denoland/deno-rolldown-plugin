import { DenoPlugin } from "./lib/rs_lib.js";

enum MediaType {
  JavaScript = 0,
  Jsx = 1,
  Mjs = 2,
  Cjs = 3,
  TypeScript = 4,
  Mts = 5,
  Cts = 6,
  Dts = 7,
  Dmts = 8,
  Dcts = 9,
  Tsx = 10,
  Css = 11,
  Json = 12,
  Html = 13,
  Sql = 14,
  Wasm = 15,
  SourceMap = 16,
  Unknown = 17,
}

enum ResolutionMode {
  Require = 0,
  Import = 1,
}

interface LoadResponse {
  specifier: string;
  mediaType: MediaType;
  code: string;
}

interface Module {
  specifier: string;
  code: string;
}

export default function denoPlugin({ debug = false }: { debug?: boolean }) {
  let plugin: DenoPlugin;
  const loads = new Map<string, Promise<LoadResponse | undefined>>();
  const modules = new Map<string, Module | undefined>();

  return {
    name: "deno-plugin",
    async buildStart(options: any) {
      const inputs = Array.isArray(options.input)
        ? options.input
        : typeof options.input === "object"
        ? Object.values(options.input)
        : [options.input];

      if (debug) {
        console.error("Inputs:", inputs);
      }
      try {
        plugin = await DenoPlugin.create(inputs);
      } catch (err: any) {
        throw new Error(err);
      }
    },
    async resolveId(
      source: string,
      importer: string | undefined,
      options: any,
    ) {
      const resolutionMode = resolveKindToResolutionMode(options.kind);
      let resolvedSpecifier: string;
      importer = importer == null
        ? undefined
        : (modules.get(importer)?.specifier ?? importer);
      if (debug) {
        console.error("Resolving", source, "from", importer);
      }
      try {
        resolvedSpecifier = plugin.resolve(source, importer, resolutionMode);
      } catch (err: any) {
        throw new Error(err);
      }
      if (debug) {
        console.error("Resolved", source, "to", resolvedSpecifier);
      }

      // now load
      let loadPromise = loads.get(resolvedSpecifier);
      if (loadPromise == null) {
        loadPromise = plugin.load(resolvedSpecifier);
      }
      let result: LoadResponse | undefined;
      try {
        result = await loadPromise;
      } catch (err: any) {
        throw new Error(err);
      }
      if (result == null) {
        modules.set(resolvedSpecifier, undefined);
        return resolvedSpecifier;
      }
      const ext = mediaTypeToExtension(result.mediaType);
      let specifier = result.specifier;
      if (!specifier.endsWith(ext)) {
        specifier += +".rolldown" + ext;
        if (debug) {
          console.error("Remapped", result.specifier, "to", specifier);
        }
      }
      modules.set(specifier, {
        specifier: result.specifier,
        code: result.code,
      });
      return specifier;
    },
    load(id: any) {
      return modules.get(id)?.code;
    },
  };
}

function mediaTypeToExtension(mediaType: MediaType) {
  switch (mediaType) {
    case MediaType.JavaScript:
    case MediaType.Mjs:
      return ".js";
    case MediaType.Cjs:
      return ".cjs";
    case MediaType.Jsx:
      return ".jsx";
    case MediaType.TypeScript:
    case MediaType.Mts:
      return ".ts";
    case MediaType.Cts:
      return ".cts";
    case MediaType.Dts:
      return ".d.ts";
    case MediaType.Dmts:
      return ".d.mts";
    case MediaType.Dcts:
      return ".d.cts";
    case MediaType.Tsx:
      return ".tsx";
    case MediaType.Css:
      return ".css";
    case MediaType.Json:
      return ".json";
    case MediaType.Html:
      return ".html";
    case MediaType.Sql:
      return ".sql";
    case MediaType.Wasm:
      return ".wasm";
    case MediaType.SourceMap:
      return ".map";
    case MediaType.Unknown:
    default:
      return "";
  }
}

function resolveKindToResolutionMode(kind: string): ResolutionMode {
  switch (kind) {
    case "import-statement":
      return ResolutionMode.Import;
    default:
      throw new Error("not implemented: " + kind);
  }
}
