use crate::USER_AGENT;
use anyhow::{Result, bail};
use log::debug;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, USER_AGENT as USER_AGENT_HEADER};
use reqwest::{Client, Method, Request, RequestBuilder, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

pub trait TokenGen: Send + Sync {
    fn generate_token(
        &self,
        method: Method,
        url: &Url,
        body: Option<&str>,
        req: RequestBuilder,
    ) -> Result<RequestBuilder>;
}

#[derive(Clone)]
pub struct JsonApi {
    client: Client,
    base: Url,
    /// Custom token generator per request
    token_gen: Option<Arc<dyn TokenGen>>,
}

impl JsonApi {
    pub fn new(base: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT_HEADER, USER_AGENT.parse()?);
        headers.insert(ACCEPT, "application/json; charset=utf-8".parse()?);

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            base: base.parse()?,
            token_gen: None,
        })
    }

    pub fn token(base: &str, token: &str, allow_invalid_certs: bool) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT_HEADER, USER_AGENT.parse()?);
        headers.insert(AUTHORIZATION, token.parse()?);
        headers.insert(ACCEPT, "application/json; charset=utf-8".parse()?);

        let client = Client::builder()
            .danger_accept_invalid_certs(allow_invalid_certs)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            base: base.parse()?,
            token_gen: None,
        })
    }

    pub fn token_gen(
        base: &str,
        allow_invalid_certs: bool,
        tg: impl TokenGen + 'static,
    ) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT_HEADER, USER_AGENT.parse()?);
        headers.insert(ACCEPT, "application/json; charset=utf-8".parse()?);

        let client = Client::builder()
            .danger_accept_invalid_certs(allow_invalid_certs)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            base: base.parse()?,
            token_gen: Some(Arc::new(tg)),
        })
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.req::<T, ()>(Method::GET, path, None).await
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn post<T: DeserializeOwned, R: Serialize>(&self, path: &str, body: R) -> Result<T> {
        self.req(Method::POST, path, Some(body)).await
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn put<T: DeserializeOwned, R: Serialize>(&self, path: &str, body: R) -> Result<T> {
        self.req(Method::PUT, path, Some(body)).await
    }

    pub fn build_req(
        &self,
        method: Method,
        path: &str,
        body: Option<impl Serialize>,
    ) -> Result<Request> {
        let url = self.base.join(path)?;
        let mut req = self
            .client
            .request(method.clone(), url.clone())
            .header(ACCEPT, "application/json");
        let req = if let Some(body) = body {
            let body = serde_json::to_string(&body)?;
            if let Some(token_gen) = self.token_gen.as_ref() {
                req = token_gen.generate_token(method.clone(), &url, Some(&body), req)?;
            }
            debug!(">> {} {}: {}", method.clone(), path, &body);
            req.header(CONTENT_TYPE, "application/json; charset=utf-8")
                .body(body)
                .build()?
        } else {
            if let Some(token_gen) = self.token_gen.as_ref() {
                req = token_gen.generate_token(method.clone(), &url, None, req)?;
            }
            req.build()?
        };
        debug!(">> HEADERS {:?}", req.headers());
        Ok(req)
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn req<T: DeserializeOwned, R: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<R>,
    ) -> Result<T> {
        let req = self.build_req(method.clone(), path, body)?;
        let rsp = match self.client.execute(req).await {
            Ok(rsp) => rsp,
            Err(e) => {
                bail!(
                    "Failed to send request: {} source={}",
                    e,
                    e.source()
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "None".to_owned())
                )
            }
        };

        let status = rsp.status();
        let text = rsp.text().await?;
        #[cfg(debug_assertions)]
        debug!("<< {}", text);
        if status.is_success() {
            match serde_json::from_str(&text) {
                Ok(t) => Ok(t),
                Err(e) => {
                    bail!("Failed to parse JSON from {}: {} {}", path, text, e);
                }
            }
        } else {
            bail!("{} {}: {}: {}", method, path, status, &text);
        }
    }

    /// Make a request and only return the status code
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn req_status<R: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<R>,
    ) -> Result<u16> {
        let req = self.build_req(method.clone(), path, body)?;
        let rsp = self.client.execute(req).await?;

        let status = rsp.status();
        let text = rsp.text().await?;
        #[cfg(debug_assertions)]
        debug!("<< {}", text);
        if status.is_success() {
            Ok(status.as_u16())
        } else {
            bail!("{} {}: {}: {}", method, path, status, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_api_new() {
        let api = JsonApi::new("https://api.example.com").unwrap();
        assert_eq!(api.base().as_str(), "https://api.example.com/");
    }

    #[test]
    fn test_json_api_new_invalid_url() {
        let result = JsonApi::new("not a valid url");
        assert!(result.is_err());
    }

    #[test]
    fn test_json_api_token() {
        let api = JsonApi::token("https://api.example.com", "Bearer token123", false).unwrap();
        assert_eq!(api.base().as_str(), "https://api.example.com/");
    }

    #[test]
    fn test_json_api_base() {
        let api = JsonApi::new("https://api.example.com/v1/").unwrap();
        assert_eq!(api.base().as_str(), "https://api.example.com/v1/");
    }

    #[test]
    fn test_json_api_build_req_get() {
        let api = JsonApi::new("https://api.example.com").unwrap();
        let req = api.build_req(Method::GET, "/test", None::<()>).unwrap();
        assert_eq!(req.method(), Method::GET);
        assert_eq!(req.url().path(), "/test");
    }

    #[test]
    fn test_json_api_build_req_post_with_body() {
        let api = JsonApi::new("https://api.example.com").unwrap();
        let body = serde_json::json!({"key": "value"});
        let req = api.build_req(Method::POST, "/test", Some(body)).unwrap();
        assert_eq!(req.method(), Method::POST);
        assert!(req.headers().get(CONTENT_TYPE).is_some());
    }

    struct TestTokenGen;
    impl TokenGen for TestTokenGen {
        fn generate_token(
            &self,
            _method: Method,
            _url: &Url,
            _body: Option<&str>,
            req: RequestBuilder,
        ) -> Result<RequestBuilder> {
            Ok(req.header("X-Custom-Token", "test123"))
        }
    }

    #[test]
    fn test_json_api_token_gen() {
        let api = JsonApi::token_gen("https://api.example.com", false, TestTokenGen).unwrap();
        assert_eq!(api.base().as_str(), "https://api.example.com/");
    }

    #[test]
    fn test_json_api_build_req_with_token_gen() {
        let api = JsonApi::token_gen("https://api.example.com", false, TestTokenGen).unwrap();
        let req = api.build_req(Method::GET, "/test", None::<()>).unwrap();
        assert_eq!(
            req.headers().get("X-Custom-Token").unwrap().to_str().unwrap(),
            "test123"
        );
    }

    #[test]
    fn test_json_api_build_req_with_token_gen_and_body() {
        let api = JsonApi::token_gen("https://api.example.com", false, TestTokenGen).unwrap();
        let body = serde_json::json!({"test": true});
        let req = api.build_req(Method::POST, "/test", Some(body)).unwrap();
        assert_eq!(
            req.headers().get("X-Custom-Token").unwrap().to_str().unwrap(),
            "test123"
        );
    }
}
