import {
  type DenoLoader,
  DenoWorkspace,
  type DenoWorkspaceOptions,
  type LoadResponse,
  MediaType,
  ResolutionMode,
} from "@deno/loader";

interface Module {
  specifier: string;
  code: string;
}

/** Options for creating the Deno plugin. */
export interface DenoPluginOptions extends DenoWorkspaceOptions {
}

export interface BuildStartOptions {
  input: string | string[] | Record<string, string>;
}

export interface ResolveIdOptions {
  kind: "import-statement" | "dynamic-import" | "require-call";
}

export interface DenoPlugin extends Disposable {
  name: string;
  buildStart(options: BuildStartOptions): Promise<void>;
  resolveId(
    source: string,
    importer: string | undefined,
    options: ResolveIdOptions,
  ): Promise<string | { id: string; external: boolean }>;
  load(id: string): string | undefined;
}

/**
 * Creates a deno plugin for use with rolldown or rollup.
 * @returns The plugin.
 */
export default function denoPlugin(
  pluginOptions: DenoPluginOptions = {},
): DenoPlugin {
  let loader: DenoLoader;
  const loads = new Map<string, Promise<LoadResponse | undefined>>();
  const modules = new Map<string, Module | undefined>();

  return {
    name: "deno-plugin",
    [Symbol.dispose]() {
      loader?.[Symbol.dispose]();
    },
    async buildStart(options: BuildStartOptions) {
      const inputs = Array.isArray(options.input)
        ? options.input
        : typeof options.input === "object"
        ? Object.values(options.input)
        : [options.input];

      const workspace = new DenoWorkspace({
        ...pluginOptions,
      });
      loader = await workspace.createLoader({
        entrypoints: inputs,
      });
    },
    async resolveId(
      source: string,
      importer: string | undefined,
      options: ResolveIdOptions,
    ) {
      const resolutionMode = resolveKindToResolutionMode(options.kind);
      importer = importer == null
        ? undefined
        : (modules.get(importer)?.specifier ?? importer);
      const resolvedSpecifier = loader.resolve(
        source,
        importer,
        resolutionMode,
      );

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
      if (result.kind === "external") {
        return {
          id: result.specifier,
          external: true,
        };
      }
      const ext = mediaTypeToExtension(result.mediaType);
      let specifier = result.specifier;
      if (!specifier.endsWith(ext)) {
        specifier += +".rolldown" + ext;
        if (pluginOptions.debug) {
          console.error("Remapped", result.specifier, "to", specifier);
        }
      }
      modules.set(specifier, {
        specifier: result.specifier,
        code: new TextDecoder().decode(result.code),
      });
      return specifier;
    },
    load(id: string) {
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
