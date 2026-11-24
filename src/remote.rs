//! Acesso remoto pelo celular: HTTP mínimo em `127.0.0.1`, pareamento por token de uso único e sessão por cookie.
//! URL, token e cookie nunca vão a log.
#![allow(dead_code)] // a UI liga isto na PR do ngrok

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const IDLE: Duration = Duration::from_secs(2 * 3600);
pub const TTL: Duration = Duration::from_secs(8 * 3600);
const MAX_BODY: usize = 16 * 1024;
const PAGE: &str = include_str!("remote.html");

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn cookie(&self) -> Option<&str> {
        self.header("cookie")?
            .split(';')
            .filter_map(|c| c.trim().strip_prefix("s="))
            .next()
    }

    fn json(&self) -> bool {
        self.header("content-type")
            .is_some_and(|c| c.starts_with("application/json"))
    }
}

#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    ctype: &'static str,
    pub body: Vec<u8>,
    cookie: Option<String>,
}

impl Response {
    fn new(status: u16, ctype: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            ctype,
            body: body.into(),
            cookie: None,
        }
    }

    pub fn status(status: u16) -> Self {
        Self::new(status, "text/plain; charset=utf-8", reason(status))
    }

    pub fn json(v: &serde_json::Value) -> Self {
        Self::new(200, "application/json", v.to_string())
    }

    fn write(&self, w: &mut impl Write) -> io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
            self.status,
            reason(self.status),
            self.ctype,
            self.body.len()
        );
        if let Some(c) = &self.cookie {
            head.push_str(&format!(
                "Set-Cookie: s={c}; HttpOnly; Secure; SameSite=Strict; Path=/\r\n"
            ));
        }
        head.push_str("\r\n");
        w.write_all(head.as_bytes())?;
        w.write_all(&self.body)
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

/// ponytail: HTTP/1.1 sem keep-alive nem chunked, uma conexão por vez; `tiny_http` se um dia faltar.
pub fn read_request(r: &mut impl BufRead) -> Result<Request, u16> {
    let mut line = String::new();
    r.read_line(&mut line).map_err(|_| 400u16)?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(400);
    };
    let (method, path) = (method.to_string(), path.to_string());
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|_| 400u16)? == 0 {
            return Err(400);
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if headers.len() > 64 || h.len() > 8192 {
            return Err(400);
        }
        let (k, v) = h.split_once(':').ok_or(400u16)?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let mut req = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    let len: usize = match req.header("content-length") {
        Some(v) => v.parse().map_err(|_| 400u16)?,
        None => 0,
    };
    if len > MAX_BODY {
        return Err(413);
    }
    req.body.resize(len, 0);
    r.read_exact(&mut req.body).map_err(|_| 400u16)?;
    Ok(req)
}

pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .expect("/dev/urandom");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Um token de pareamento por vez e uma sessão por vez; parear de novo derruba a anterior.
pub struct Auth {
    token: Option<String>,
    session: Option<String>,
    started: Instant,
    last: Instant,
}

impl Auth {
    pub fn new(now: Instant) -> (Self, String) {
        let token = random_hex(16);
        let auth = Self {
            token: Some(token.clone()),
            session: None,
            started: now,
            last: now,
        };
        (auth, token)
    }

    pub fn repair(&mut self) -> String {
        let token = random_hex(16);
        self.token = Some(token.clone());
        self.session = None;
        token
    }

    pub fn pair(&mut self, token: &str, now: Instant) -> Option<String> {
        if self.expired(now) || !self.token.as_deref().is_some_and(|t| same(t, token)) {
            return None;
        }
        self.token = None;
        let s = random_hex(32);
        self.session = Some(s.clone());
        self.last = now;
        Some(s)
    }

    pub fn check(&mut self, cookie: Option<&str>, now: Instant) -> bool {
        let ok = !self.expired(now)
            && matches!((&self.session, cookie), (Some(s), Some(c)) if same(s, c));
        if ok {
            self.last = now;
        }
        ok
    }

    pub fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.last) >= IDLE || now.duration_since(self.started) >= TTL
    }

    /// O que falta até desligar sozinho: o menor dos dois limites.
    pub fn left(&self, now: Instant) -> Duration {
        let idle = IDLE.saturating_sub(now.duration_since(self.last));
        let ttl = TTL.saturating_sub(now.duration_since(self.started));
        idle.min(ttl)
    }
}

/// Chamada ao herdr: `(método, params)`. Injetada para o roteamento ser testável sem socket.
pub type Call<'a> = &'a dyn Fn(&str, serde_json::Value) -> io::Result<serde_json::Value>;

pub fn handle(req: &Request, auth: &Mutex<Auth>, now: Instant, _call: Call) -> Response {
    if req.method == "GET" && (req.path == "/" || req.path.starts_with("/?")) {
        return Response::new(200, "text/html; charset=utf-8", PAGE);
    }
    if !req.path.starts_with("/api/") {
        return Response::status(404);
    }
    if req.method == "POST" && !req.json() {
        return Response::status(415);
    }
    if req.method == "POST" && req.path == "/api/pair" {
        let token = serde_json::from_slice::<serde_json::Value>(&req.body)
            .ok()
            .and_then(|v| v["token"].as_str().map(str::to_string))
            .unwrap_or_default();
        return match auth.lock().unwrap().pair(&token, now) {
            Some(s) => Response {
                cookie: Some(s),
                ..Response::json(&serde_json::json!({ "ok": true }))
            },
            None => Response::status(401),
        };
    }
    if !auth.lock().unwrap().check(req.cookie(), now) {
        return Response::status(401);
    }
    Response::status(404)
}

pub struct Server {
    pub addr: SocketAddr,
    pub auth: Arc<Mutex<Auth>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Sobe em `127.0.0.1` numa porta livre; devolve o servidor e o token do primeiro pareamento.
    pub fn start(
        call: impl Fn(&str, serde_json::Value) -> io::Result<serde_json::Value> + Send + 'static,
    ) -> io::Result<(Self, String)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let (auth, token) = Auth::new(Instant::now());
        let auth = Arc::new(Mutex::new(auth));
        let stop = Arc::new(AtomicBool::new(false));
        let (a, s) = (auth.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut conn) = conn else { continue };
                let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = conn.set_write_timeout(Some(Duration::from_secs(5)));
                let resp = match read_request(&mut BufReader::new(&mut conn)) {
                    Ok(req) => handle(&req, &a, Instant::now(), &call),
                    Err(status) => Response::status(status),
                };
                let _ = resp.write(&mut conn);
            }
        });
        Ok((Self { addr, auth, stop }, token))
    }

    pub fn expired(&self) -> bool {
        self.auth.lock().unwrap().expired(Instant::now())
    }

    pub fn left(&self) -> Duration {
        self.auth.lock().unwrap().left(Instant::now())
    }

    pub fn repair(&self) -> String {
        self.auth.lock().unwrap().repair()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(raw: &str) -> Request {
        read_request(&mut raw.as_bytes()).unwrap()
    }

    fn no_herdr(_: &str, _: serde_json::Value) -> io::Result<serde_json::Value> {
        Err(io::Error::other("sem herdr"))
    }

    fn pair_req(token: &str) -> Request {
        let body = format!("{{\"token\":\"{token}\"}}");
        req(&format!(
            "POST /api/pair HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    fn with_cookie(path: &str, cookie: &str) -> Request {
        req(&format!(
            "GET {path} HTTP/1.1\r\nCookie: a=1; s={cookie}\r\n\r\n"
        ))
    }

    #[test]
    fn le_requisicao_com_corpo() {
        let r = req("POST /api/x HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nhello");
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/api/x"));
        assert_eq!(r.body, b"hello");
        assert_eq!(r.header("HOST"), Some("a"));
    }

    #[test]
    fn recusa_corpo_grande_e_lixo() {
        let big = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert_eq!(read_request(&mut big.as_bytes()).unwrap_err(), 413);
        assert_eq!(read_request(&mut "lixo".as_bytes()).unwrap_err(), 400);
        assert_eq!(
            read_request(&mut "GET / HTTP/1.1\r\nsemdoispontos\r\n\r\n".as_bytes()).unwrap_err(),
            400
        );
    }

    #[test]
    fn pareia_uma_vez_e_sessao_vale() {
        let now = Instant::now();
        let (a, token) = Auth::new(now);
        let a = Mutex::new(a);
        let r = handle(&pair_req(&token), &a, now, &no_herdr);
        assert_eq!(r.status, 200);
        let cookie = r.cookie.clone().unwrap();
        assert_eq!(cookie.len(), 64);
        let mut out = Vec::new();
        r.write(&mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("HttpOnly; Secure; SameSite=Strict"));
        assert_eq!(handle(&pair_req(&token), &a, now, &no_herdr).status, 401);
        assert_eq!(
            handle(&with_cookie("/api/nada", &cookie), &a, now, &no_herdr).status,
            404
        );
    }

    #[test]
    fn token_errado_e_sem_cookie_dao_401() {
        let now = Instant::now();
        let (a, token) = Auth::new(now);
        let a = Mutex::new(a);
        let wrong = "0".repeat(token.len());
        assert_eq!(handle(&pair_req(&wrong), &a, now, &no_herdr).status, 401);
        assert_eq!(
            handle(&req("GET /api/agents HTTP/1.1\r\n\r\n"), &a, now, &no_herdr).status,
            401
        );
        assert_eq!(
            handle(&with_cookie("/api/agents", &wrong), &a, now, &no_herdr).status,
            401
        );
    }

    #[test]
    fn parear_de_novo_derruba_a_sessao_anterior() {
        let now = Instant::now();
        let (a, token) = Auth::new(now);
        let a = Mutex::new(a);
        let old = handle(&pair_req(&token), &a, now, &no_herdr)
            .cookie
            .unwrap();
        let token2 = a.lock().unwrap().repair();
        assert_ne!(token, token2);
        assert_eq!(
            handle(&with_cookie("/api/x", &old), &a, now, &no_herdr).status,
            401
        );
        let new = handle(&pair_req(&token2), &a, now, &no_herdr)
            .cookie
            .unwrap();
        assert_eq!(
            handle(&with_cookie("/api/x", &new), &a, now, &no_herdr).status,
            404
        );
    }

    #[test]
    fn escrita_sem_json_e_rota_fora_da_api() {
        let now = Instant::now();
        let (a, _) = Auth::new(now);
        let a = Mutex::new(a);
        let form = req(
            "POST /api/pair HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 0\r\n\r\n",
        );
        assert_eq!(handle(&form, &a, now, &no_herdr).status, 415);
        assert_eq!(
            handle(&req("GET /etc/passwd HTTP/1.1\r\n\r\n"), &a, now, &no_herdr).status,
            404
        );
        let page = handle(&req("GET / HTTP/1.1\r\n\r\n"), &a, now, &no_herdr);
        assert_eq!(page.status, 200);
    }

    #[test]
    fn expira_por_inatividade_e_por_teto() {
        let t0 = Instant::now();
        let (a, token) = Auth::new(t0);
        let a = Mutex::new(a);
        let cookie = handle(&pair_req(&token), &a, t0, &no_herdr).cookie.unwrap();
        let mut t = t0;
        while t.duration_since(t0) < TTL - Duration::from_secs(3600) {
            t += Duration::from_secs(3600);
            assert_eq!(
                handle(&with_cookie("/api/x", &cookie), &a, t, &no_herdr).status,
                404
            );
        }
        assert_eq!(a.lock().unwrap().left(t), Duration::from_secs(3600));
        t += Duration::from_secs(3600);
        assert_eq!(
            handle(&with_cookie("/api/x", &cookie), &a, t, &no_herdr).status,
            401
        );

        let (b, token) = Auth::new(t0);
        let b = Mutex::new(b);
        let cookie = handle(&pair_req(&token), &b, t0, &no_herdr).cookie.unwrap();
        let later = t0 + IDLE;
        assert_eq!(
            handle(&with_cookie("/api/x", &cookie), &b, later, &no_herdr).status,
            401
        );
        assert!(b.lock().unwrap().expired(later));
    }

    #[test]
    fn servidor_escuta_so_local_e_serve_a_pagina() {
        let (srv, _) = Server::start(no_herdr).unwrap();
        assert!(srv.addr.ip().is_loopback());
        let mut c = TcpStream::connect(srv.addr).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        assert!(out.contains("Cache-Control: no-store"));
        assert!(out.contains("<title>Maestro</title>"));
    }

    #[test]
    fn nada_vai_para_os_dados_do_maestro() {
        let dir = std::env::var("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".local/share/maestro"))
            .unwrap();
        let ls = || {
            let mut v: Vec<_> = std::fs::read_dir(&dir)
                .map(|d| {
                    d.flatten()
                        .map(|e| {
                            (
                                e.file_name(),
                                e.metadata().ok().and_then(|m| m.modified().ok()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        };
        let before = ls();
        let (srv, token) = Server::start(no_herdr).unwrap();
        let body = format!("{{\"token\":\"{token}\"}}");
        let mut c = TcpStream::connect(srv.addr).unwrap();
        write!(c, "POST /api/pair HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200"));
        drop(srv);
        assert_eq!(before, ls());
    }
}
