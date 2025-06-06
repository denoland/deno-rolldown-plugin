import { DenoWorkspace, type DenoLoader, MediaType, type LoadResponse, ResolutionMode, type DenoWorkspaceOptions } from "@deno/loader";

interface Module {
  specifier: string;
  code: string;
}

/** Options for creating the Deno plugin. */
export interface DenoPluginOptions extends DenoWorkspaceOptions {
}

/**
 * Creates a deno plugin for use with rolldown or rollup.
 * @returns The plugin.
 */
export default function denoPlugin(pluginOptions: DenoPluginOptions = {}): any {
  let loader: DenoLoader;
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

      const workspace = new DenoWorkspace({
        ...pluginOptions
      });
      loader = await workspace.createLoader({
        entrypoints: inputs,
      });
    },
    async resolveId(
      source: string,
      importer: string | undefined,
      options: any,
    ) {
      const resolutionMode = resolveKindToResolutionMode(options.kind);
      importer = importer == null
        ? undefined
        : (modules.get(importer)?.specifier ?? importer);
      const resolvedSpecifier = loader.resolve(source, importer, resolutionMode);

      // now load
      let loadPromise = loads.get(resolvedSpecifier);
      if (loadPromise == null) {
        loadPromise = loader.load(resolvedSpecifier);
      }
      const result = await loadPromise;
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
        code: new TextDecoder().decode(result.code),
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
    case "dynamic-import":
      return ResolutionMode.Import;
    case "require-call":
      return ResolutionMode.Require;
    default:
      throw new Error("not implemented: " + kind);
  }
}
