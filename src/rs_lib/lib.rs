mod http_client;
mod module_analyzer;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_cache_dir::file_fetcher::CacheSetting;
use deno_cache_dir::file_fetcher::NullBlobStore;
use deno_graph::MediaType;
use deno_graph::Module;
use deno_graph::ModuleGraph;
use deno_npm_installer::lifecycle_scripts::NullLifecycleScriptsExecutor;
use deno_npm_installer::NpmInstallerFactory;
use deno_npm_installer::NpmInstallerFactoryOptions;
use deno_resolver::factory::ResolverFactory;
use deno_resolver::factory::ResolverFactoryOptions;
use deno_resolver::factory::WorkspaceFactory;
use deno_resolver::file_fetcher::DenoGraphLoader;
use deno_resolver::file_fetcher::DenoGraphLoaderOptions;
use deno_resolver::file_fetcher::PermissionedFileFetcher;
use deno_resolver::file_fetcher::PermissionedFileFetcherOptions;
use deno_resolver::graph::DefaultDenoResolverRc;
use deno_resolver::workspace::ScopedJsxImportSourceConfig;
use serde::Serialize;
use sys_traits::impls::RealSys;
use sys_traits::EnvCurrentDir;
use url::Url;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;

use self::http_client::WasmHttpClient;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadResponse {
  pub specifier: String,
  pub media_type: u8,
  pub code: String,
}

#[wasm_bindgen]
pub struct DenoPlugin {
  cwd: PathBuf,
  resolver: DefaultDenoResolverRc<RealSys>,
  file_fetcher:
    Arc<PermissionedFileFetcher<NullBlobStore, RealSys, WasmHttpClient>>,
  graph: ModuleGraph,
}

#[wasm_bindgen]
impl DenoPlugin {
  pub async fn create(entrypoints: Vec<String>) -> Result<Self, String> {
    console_error_panic_hook::set_once();
    DenoPlugin::create_inner(entrypoints)
      .await
      .map_err(|err| err.to_string())
  }

  async fn create_inner(entrypoints: Vec<String>) -> Result<Self, anyhow::Error> {
    let sys = RealSys;
    let cwd = sys.env_current_dir()?;
    let roots = entrypoints.iter().map(|e| parse_entrypoint(e, &cwd)).collect::<Result<Vec<_>, _>>()?;
    let workspace_factory =
      Arc::new(WorkspaceFactory::new(sys.clone(), cwd, Default::default()));
    let cwd = workspace_factory.initial_cwd();
    let resolver_factory = Arc::new(ResolverFactory::new(
      workspace_factory.clone(),
      ResolverFactoryOptions {
        is_cjs_resolution_mode:
          deno_resolver::cjs::IsCjsResolutionMode::ImplicitTypeCommonJs,
        unstable_sloppy_imports: true,
        ..Default::default()
      },
    ));
    let wasm_http_client = WasmHttpClient::default();
    let npm_installer_factory = NpmInstallerFactory::new(
      resolver_factory.clone(),
      Arc::new(wasm_http_client.clone()),
      Arc::new(NullLifecycleScriptsExecutor),
      deno_npm_installer::LogReporter,
      NpmInstallerFactoryOptions {
        cache_setting: deno_npm_cache::NpmCacheSetting::Use,
        caching_strategy: deno_npm_installer::graph::NpmCachingStrategy::Eager,
        lifecycle_scripts_config: deno_npm_installer::LifecycleScriptsConfig {
          allowed: deno_npm_installer::PackagesAllowedScripts::None,
          initial_cwd: cwd.clone(),
          root_dir: workspace_factory
            .workspace_directory()?
            .workspace
            .root_dir_path(),
          explicit_install: false,
        },
        resolve_npm_resolution_snapshot: Box::new(|| Ok(None)),
      },
    );
    npm_installer_factory
      .initialize_npm_resolution_if_managed()
      .await?;
    let npm_package_info_provider =
      npm_installer_factory.lockfile_npm_package_info_provider()?;
    let lockfile = workspace_factory
      .maybe_lockfile(npm_package_info_provider)
      .await?;
    let resolver = resolver_factory.deno_resolver().await?;
    let cjs_tracker = resolver_factory.cjs_tracker()?;
    let jsx_config = ScopedJsxImportSourceConfig::from_workspace_dir(
      workspace_factory.workspace_directory()?,
    )?;

    let file_fetcher = Arc::new(PermissionedFileFetcher::new(
      NullBlobStore,
      Arc::new(workspace_factory.http_cache()?.clone()),
      wasm_http_client,
      sys.clone(),
      PermissionedFileFetcherOptions {
        allow_remote: true,
        cache_setting: CacheSetting::Use,
      },
    ));
    let graph_resolver = resolver.as_graph_resolver(cjs_tracker, &jsx_config);
    let loader = DenoGraphLoader::new(
      file_fetcher.clone(),
      workspace_factory.global_http_cache()?.clone(),
      resolver_factory.in_npm_package_checker()?.clone(),
      workspace_factory.sys().clone(),
      DenoGraphLoaderOptions {
        file_header_overrides: Default::default(),
        permissions: None,
      },
    );

    let mut locker = lockfile.as_ref().map(|l| l.as_deno_graph_locker());
    let mut graph =
      deno_graph::ModuleGraph::new(deno_graph::GraphKind::CodeOnly);
    let npm_resolver = npm_installer_factory.npm_deno_graph_resolver().await?;
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        deno_graph::BuildOptions {
          is_dynamic: false,
          skip_dynamic_deps: false,
          module_info_cacher: Default::default(),
          executor: Default::default(),
          locker: locker.as_mut().map(|l| l as _),
          file_system: &sys,
          jsr_url_provider: Default::default(),
          passthrough_jsr_specifiers: false,
          module_analyzer: &module_analyzer::OxcModuleAnalyzer,
          npm_resolver: Some(npm_resolver.as_ref()),
          reporter: None,
          resolver: Some(&graph_resolver),
        },
      )
      .await;
    graph.valid()?;

    Ok(Self {
      cwd: cwd.clone(),
      file_fetcher,
      resolver: resolver.clone(),
      graph,
    })
  }

  pub fn resolve(
    &self,
    specifier: String,
    importer: Option<String>,
    resolution_mode: u8,
  ) -> Result<String, String> {
    let resolution_mode = match resolution_mode {
      0 => node_resolver::ResolutionMode::Require,
      _ => node_resolver::ResolutionMode::Import,
    };
    self
      .resolve_inner(specifier, importer, resolution_mode)
      .map_err(|err| err.to_string())
  }

  fn resolve_inner(
    &self,
    specifier: String,
    importer: Option<String>,
    resolution_mode: node_resolver::ResolutionMode,
  ) -> Result<String, anyhow::Error> {
    let referrer = match &importer {
      Some(referrer) if referrer.starts_with("http:") || referrer.starts_with("https:") || referrer.starts_with("file:") => Url::parse(referrer)?,
      Some(referrer) => deno_path_util::url_from_file_path(&PathBuf::from(referrer))?,
      None => {
        return Ok(parse_entrypoint(&specifier, &self.cwd)?.to_string())
      },
    };
    let resolved = self.resolver.resolve_with_graph(
      &self.graph,
      &specifier,
      &referrer,
      deno_graph::Position::zeroed(),
      resolution_mode,
      node_resolver::NodeResolutionKind::Execution,
    )?;
    Ok(resolved.to_string())
  }

  pub async fn load(&self, url: String) -> Result<JsValue, String> {
    let response = self.load_inner(url).await.map_err(|err| err.to_string())?;
    let value =
      serde_wasm_bindgen::to_value(&response).map_err(|err| err.to_string())?;
    Ok(value)
  }

  async fn load_inner(
    &self,
    url: String,
  ) -> Result<Option<LoadResponse>, anyhow::Error> {
    let url = Url::parse(&url)?;

    match self.graph.get(&url) {
      Some(Module::Js(js)) => Ok(Some(LoadResponse {
        specifier: js.specifier.to_string(),
        code: js.source.to_string(),
        media_type: media_type_to_u8(js.media_type),
      })),
      Some(Module::Json(json)) => Ok(Some(LoadResponse {
        specifier: json.specifier.to_string(),
        media_type: media_type_to_u8(MediaType::Json),
        code: json.source.to_string(),
      })),
      Some(Module::Wasm(_wasm)) => {
        anyhow::bail!("Wasm is not supported.")
      }
      Some(Module::Npm(_) | Module::Node(_) | Module::External(_)) | None => {
        let file = self.file_fetcher.fetch_bypass_permissions(&url).await?;
        Ok(Some(LoadResponse {
          specifier: file.url.to_string(),
          media_type: media_type_to_u8(MediaType::from_specifier_and_headers(
            &url,
            file.maybe_headers.as_ref(),
          )),
          code: String::from_utf8_lossy(&file.source).into(),
        }))
      }
    }
  }
}

fn parse_entrypoint(entrypoint: &str, cwd:& Path) -> Result<Url, anyhow::Error> {
  if entrypoint.starts_with("jsr:")
      || entrypoint.starts_with("https:")
      || entrypoint.starts_with("file:")
    {
      Ok(Url::parse(&entrypoint)?)
    } else {
      Ok(deno_path_util::url_from_file_path(&cwd.join(entrypoint))?)
    }
}

fn media_type_to_u8(media_type: MediaType) -> u8 {
  match media_type {
    MediaType::JavaScript => 0,
    MediaType::Jsx => 1,
    MediaType::Mjs => 2,
    MediaType::Cjs => 3,
    MediaType::TypeScript => 4,
    MediaType::Mts => 5,
    MediaType::Cts => 6,
    MediaType::Dts => 7,
    MediaType::Dmts => 8,
    MediaType::Dcts => 9,
    MediaType::Tsx => 10,
    MediaType::Css => 11,
    MediaType::Json => 12,
    MediaType::Html => 13,
    MediaType::Sql => 14,
    MediaType::Wasm => 15,
    MediaType::SourceMap => 16,
    MediaType::Unknown => 17,
  }
}
