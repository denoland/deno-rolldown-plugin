use deno_cache_dir::file_fetcher::HeaderMap;
use deno_cache_dir::file_fetcher::HeaderName;
use deno_cache_dir::file_fetcher::HeaderValue;
use deno_cache_dir::file_fetcher::SendError;
use deno_cache_dir::file_fetcher::SendResponse;
use deno_cache_dir::file_fetcher::StatusCode;
use deno_error::JsErrorBox;
use deno_npm_cache::NpmCacheHttpClientResponse;
use js_sys::Object;
use js_sys::Reflect;
use serde::Deserialize;
use url::Url;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;

#[wasm_bindgen(module = "/helpers.js")]
extern "C" {
  async fn fetch_specifier(specifier: String, headers: JsValue) -> JsValue;
}

enum FetchResult {
  Response(Response),
  Error(FetchError),
}

#[derive(Deserialize)]
struct FetchError {
  pub error: String,
}

struct Response {
  pub status: u16,
  pub body: Vec<u8>,
  pub headers: HeaderMap,
}

async fn fetch_specifier_typed(
  specifier: &str,
  headers: Vec<(String, String)>,
) -> Result<FetchResult, anyhow::Error> {
  let headers = headers_to_js_object(&headers);
  let response = fetch_specifier(specifier.to_string(), headers).await;
  parse_fetch_result(response).map_err(|err| {

    if let Some(s) = err.as_string() {
        anyhow::anyhow!(s)
    } else {
        // Optionally stringify complex JS error objects
        anyhow::anyhow!(format!("{:?}", err))
    }
  })
}

#[derive(Debug, Default, Clone)]
pub struct WasmHttpClient;

#[async_trait::async_trait(?Send)]
impl deno_cache_dir::file_fetcher::HttpClient for WasmHttpClient {
  async fn send_no_follow(
    &self,
    url: &Url,
    headers: HeaderMap,
  ) -> Result<SendResponse, SendError> {
    let headers = headers
      .into_iter()
      .filter_map(|(k, v)| Some((k?.to_string(), v.to_str().ok()?.to_string())))
      .collect::<Vec<(String, String)>>();
    let result = fetch_specifier_typed(url.as_str(), headers)
      .await
      .map_err(|err| {
        SendError::Failed(Box::new(std::io::Error::new(
          std::io::ErrorKind::Other,
          err.to_string(),
        )))
      })?;
    let response = match result {
      FetchResult::Response(response) => response,
      FetchResult::Error(fetch_error) => {
        return Err(SendError::Failed(fetch_error.error.into()))
      }
    };
    match response.status {
      304 => Ok(SendResponse::NotModified),
      300..=399 => Ok(SendResponse::Redirect(response.headers)),
      404 => Err(SendError::NotFound),
      200..=299 => Ok(SendResponse::Success(
        response.headers,
        response.body,
      )),
      _ => Err(SendError::StatusCode(
        StatusCode::from_u16(response.status).unwrap(),
      )),
    }
  }
}

#[async_trait::async_trait(?Send)]
impl deno_npm_cache::NpmCacheHttpClient for WasmHttpClient {
  // todo: implement retrying
  async fn download_with_retries_on_any_tokio_runtime(
    &self,
    url: Url,
    maybe_auth: Option<String>,
    maybe_etag: Option<String>,
  ) -> Result<NpmCacheHttpClientResponse, deno_npm_cache::DownloadError> {
    let mut headers = Vec::new();
    if let Some(auth) = maybe_auth {
      headers.push(("authorization".to_string(), auth));
    }
    if let Some(etag) = maybe_etag {
      headers.push(("if-none-match".to_string(), etag));
    }

    let result = fetch_specifier_typed(url.as_str(), headers)
      .await
      .map_err(|err| deno_npm_cache::DownloadError {
        status_code: None,
        error: JsErrorBox::generic(err.to_string()),
      })?;

    let response = match result {
      FetchResult::Response(res) => res,
      FetchResult::Error(fetch_error) => {
        return Err(deno_npm_cache::DownloadError {
          status_code: None,
          error: JsErrorBox::generic(fetch_error.error),
        });
      }
    };

    match response.status {
      200..=299 => {
        let etag = response.headers
          .iter()
          .find_map(|(k, v)| {
            if k.as_str().eq_ignore_ascii_case("etag") {
              Some(v.to_str().ok()?.to_string())
            } else {
              None
            }
          });

        Ok(NpmCacheHttpClientResponse::Bytes(
          deno_npm_cache::NpmCacheHttpClientBytesResponse {
            etag,
            bytes: response.body.into(),
          },
        ))
      }
      304 => Ok(NpmCacheHttpClientResponse::NotModified),
      404 => Ok(NpmCacheHttpClientResponse::NotFound),
      code => Err(deno_npm_cache::DownloadError {
        status_code: Some(code as u16),
        error: JsErrorBox::generic(format!("Unexpected status: {code}")),
      }),
    }
  }
}

fn headers_to_js_object(headers: &[(String, String)]) -> JsValue {
  let obj = Object::new();
  for (key, value) in headers {
    Reflect::set(&obj, &JsValue::from_str(key), &JsValue::from_str(value))
      .unwrap();
  }
  obj.into()
}

fn parse_fetch_result(js_value: JsValue) -> Result<FetchResult, JsValue> {
    let has_error = Reflect::has(&js_value, &JsValue::from_str("error"))?;
    if has_error {
        let error: FetchError = serde_wasm_bindgen::from_value(js_value)?;
        return Ok(FetchResult::Error(error));
    }
    Ok(FetchResult::Response(parse_response(js_value)?))
}

fn parse_response(js_value: JsValue) -> Result<Response, JsValue> {
    let status = Reflect::get(&js_value, &JsValue::from_str("status"))?
        .as_f64()
        .ok_or_else(|| JsValue::from_str("status must be a number"))? as u16;

    let body_js = Reflect::get(&js_value, &JsValue::from_str("body"))?;
    let body: Vec<u8> = serde_wasm_bindgen::from_value(body_js)?;

    let headers_js = Reflect::get(&js_value, &JsValue::from_str("headers"))?;
    let headers = response_headers_to_headermap(headers_js);

    Ok(Response {
        status,
        body,
        headers,
    })
}

fn response_headers_to_headermap(headers: JsValue) -> HeaderMap {
  let mut map = HeaderMap::new();

  if !headers.is_object() {
    return map;
  }

  let obj = Object::from(headers);
  let entries = Object::entries(&obj);

  for i in 0..entries.length() {
    let entry = entries.get(i);
    if !entry.is_object() {
      continue;
    }
    let pair = js_sys::Array::from(&entry);
    if pair.length() != 2 {
      continue;
    }

    let key = pair.get(0).as_string();
    let value = pair.get(1).as_string();

    if let (Some(k), Some(v)) = (key, value) {
      if let (Ok(name), Ok(val)) = (
        HeaderName::from_bytes(k.as_bytes()),
        HeaderValue::from_str(&v),
      ) {
        map.append(name, val);
      }
    }
  }

  map
}
