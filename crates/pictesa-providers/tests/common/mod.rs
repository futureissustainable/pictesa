//! A local HTTPS server that stands in for a provider's API, and a context whose client sends the
//! provider's real host names to it. Contract tests answer with the provider's documented
//! responses and check what the adapter sent; nothing leaves the machine and no key is used.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, service::service_fn, Request, Response};
use pictesa_core::Ctx;
use serde_json::Value;
use tokio_rustls::{rustls, TlsAcceptor};

/// One request the adapter made.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    pub host: String,
    /// Path and query, e.g. `/v1/predictions?x=1`.
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Call {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

/// What the stand-in answers.
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl Reply {
    pub fn json(v: Value) -> Self {
        Self::status(200, v)
    }
    pub fn status(status: u16, v: Value) -> Self {
        Self { status, content_type: "application/json".into(), body: v.to_string().into_bytes(), headers: vec![] }
    }
    pub fn bytes(mime: &str, data: &[u8]) -> Self {
        Self { status: 200, content_type: mime.into(), body: data.to_vec(), headers: vec![] }
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

pub struct Fake {
    pub addr: SocketAddr,
    pub calls: Arc<Mutex<Vec<Call>>>,
}

impl Fake {
    /// Every request so far.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// A context for the adapter: its HTTP client resolves `hosts` to this server and trusts its
    /// throwaway certificate. `creds` are the credential fields (`api_key`, ...).
    pub fn ctx(&self, hosts: &[&str], creds: &[(&str, &str)]) -> Ctx {
        let mut b = reqwest::Client::builder().danger_accept_invalid_certs(true);
        for host in hosts {
            b = b.resolve(host, self.addr);
        }
        Ctx::new(b.build().unwrap(), creds.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }
}

/// Starts the stand-in; `answer` decides the reply for each request.
pub async fn start(answer: impl Fn(&Call) -> Reply + Send + Sync + 'static) -> Fake {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(cert.key_pair.serialize_der().into());
    let mut config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(vec![cert.cert.der().clone()], key).unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
    let answer = Arc::new(answer);
    let seen = calls.clone();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else { return };
            let (acceptor, answer, seen) = (acceptor.clone(), answer.clone(), seen.clone());
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else { return };
                let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                    let (answer, seen) = (answer.clone(), seen.clone());
                    async move {
                        let (parts, body) = req.into_parts();
                        let body = body.collect().await.map(|b| b.to_bytes().to_vec()).unwrap_or_default();
                        let call = Call {
                            method: parts.method.to_string(),
                            host: parts.headers.get("host").and_then(|h| h.to_str().ok()).unwrap_or("").split(':').next().unwrap_or("").to_string(),
                            path: parts.uri.path_and_query().map(|p| p.to_string()).unwrap_or_default(),
                            headers: parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())).collect(),
                            body,
                        };
                        let reply = answer(&call);
                        seen.lock().unwrap().push(call);
                        let mut res = Response::builder().status(reply.status).header("content-type", reply.content_type);
                        for (k, v) in reply.headers {
                            res = res.header(k, v);
                        }
                        Ok::<_, std::convert::Infallible>(res.body(Full::new(Bytes::from(reply.body))).unwrap())
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(hyper_util::rt::TokioIo::new(tls), service).await;
            });
        }
    });
    Fake { addr, calls }
}

/// A tiny valid PNG, for inputs and outputs.
pub const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0,
    0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0, 5, 0, 1, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];
