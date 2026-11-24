//! Acesso remoto pelo celular: HTTP mínimo em `127.0.0.1`, pareamento por token de uso único e sessão por cookie.
//! URL, token e cookie nunca vão a log.

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

const KEYS: &[&str] = &["1", "2", "3", "enter", "esc"];
const GONE: &str = "O Claude não está mais rodando nesta feature";

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (
            b[i],
            s.get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok()),
        ) {
            (b'%', Some(x)) => {
                out.push(x);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// O que o celular vê de cada agente: a feature (branch da worktree), o repo, o estado e o título do Claude.
fn summary(a: &serde_json::Value, labels: &serde_json::Value) -> serde_json::Value {
    let cwd = std::path::Path::new(a["cwd"].as_str().unwrap_or(""));
    let root = crate::herdr::repo_root(cwd);
    let label = root
        .and_then(crate::herdr::branch)
        .or_else(|| {
            labels.as_array()?.iter().find_map(|w| {
                (w["workspace_id"] == a["workspace_id"])
                    .then(|| w["label"].as_str().map(str::to_string))?
            })
        })
        .unwrap_or_else(|| a["agent"].as_str().unwrap_or("").to_string());
    let repo = root
        .map(crate::herdr::main_repo)
        .and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let title = a["terminal_title_stripped"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches(|c: char| !c.is_alphanumeric());
    serde_json::json!({
        "pane": a["pane_id"],
        "label": label,
        "repo": repo,
        "status": a["agent_status"],
        "title": title,
    })
}

fn api(req: &Request, call: Call) -> io::Result<Response> {
    let list = call("agent.list", serde_json::json!({}))?;
    let agents = list["agents"].as_array().cloned().unwrap_or_default();
    let rest = &req.path["/api/agents".len()..];
    if req.method == "GET" && rest.is_empty() {
        let labels = call("workspace.list", serde_json::json!({}))
            .map(|mut w| w["workspaces"].take())
            .unwrap_or_default();
        let out: Vec<_> = agents.iter().map(|a| summary(a, &labels)).collect();
        return Ok(Response::json(&serde_json::json!({ "agents": out })));
    }
    let Some((pane, action)) = rest.strip_prefix('/').and_then(|r| r.rsplit_once('/')) else {
        return Ok(Response::status(404));
    };
    let pane = unescape(pane);
    if !agents.iter().any(|a| a["pane_id"] == pane.as_str()) {
        return Ok(Response::new(404, "text/plain; charset=utf-8", GONE));
    }
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
    match (req.method.as_str(), action) {
        ("GET", "screen") => {
            let r = call(
                "agent.read",
                serde_json::json!({ "target": pane, "source": "recent_unwrapped", "lines": 60, "format": "text" }),
            )?;
            Ok(Response::json(
                &serde_json::json!({ "text": r["read"]["text"] }),
            ))
        }
        ("POST", "keys") => {
            let Some(key) = body["key"].as_str().filter(|k| KEYS.contains(k)) else {
                return Ok(Response::status(400));
            };
            call(
                "agent.send_keys",
                serde_json::json!({ "target": pane, "keys": [key] }),
            )?;
            Ok(Response::json(&serde_json::json!({ "ok": true })))
        }
        ("POST", "prompt") => {
            let Some(text) = body["text"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            else {
                return Ok(Response::status(400));
            };
            call(
                "agent.prompt",
                serde_json::json!({ "target": pane, "text": text }),
            )?;
            Ok(Response::json(&serde_json::json!({ "ok": true })))
        }
        _ => Ok(Response::status(404)),
    }
}

pub fn handle(req: &Request, auth: &Mutex<Auth>, now: Instant, call: Call) -> Response {
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
    if req.path != "/api/agents" && !req.path.starts_with("/api/agents/") {
        return Response::status(404);
    }
    api(req, call).unwrap_or_else(|_| Response::status(502))
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

    /// Por que desligar agora, se for hora.
    pub fn expiry(&self) -> Option<&'static str> {
        let a = self.auth.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(a.started) >= TTL {
            Some("Acesso remoto desligado: chegou ao limite de 8 h")
        } else if a.expired(now) {
            Some("Acesso remoto desligado por inatividade")
        } else {
            None
        }
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

/// O túnel e o servidor juntos: existe enquanto o acesso remoto está ligado.
pub struct Remote {
    pub server: Server,
    url: String,
    token: String,
    group: u32,
}

/// O ngrok roda sob um `sh` vigia, num grupo de processos próprio: o vigia mata o ngrok quando o Maestro
/// morre por qualquer caminho (fechar a janela, crash), sem depender de gancho de saída.
const WATCH: &str = r#"p=$1; shift; "$@" & n=$!; while kill -0 "$p" 2>/dev/null && kill -0 "$n" 2>/dev/null; do sleep 1; done; kill "$n" 2>/dev/null"#;

impl Remote {
    /// Sobe o servidor e o `ngrok` (`bin`) e espera a URL pública. Bloqueia: chamar fora da thread da UI.
    pub fn start(
        bin: &str,
        call: impl Fn(&str, serde_json::Value) -> io::Result<serde_json::Value> + Send + 'static,
    ) -> Result<Self, String> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let (server, token) = Server::start(call).map_err(|e| e.to_string())?;
        let target = format!("127.0.0.1:{}", server.addr.port());
        let mut child = Command::new("sh")
            .args([
                "-c",
                WATCH,
                "sh",
                &std::process::id().to_string(),
                bin,
                "http",
                &target,
            ])
            .args(["--inspect=false", "--log", "stdout", "--log-format", "json"])
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ngrok: {e}"))?;
        let group = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        let out = child.stdout.take().unwrap();
        // a saída do ngrok é lida até o fim (cano cheio travaria o túnel) e jogada fora: nada vai a arquivo
        std::thread::spawn(move || {
            let mut tx = Some(tx);
            let mut err = String::new();
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                if let Some(u) = v["url"].as_str().filter(|u| u.starts_with("https://")) {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(Ok(u.to_string()));
                    }
                } else if let Some(e) = v["err"].as_str().filter(|e| *e != "<nil>") {
                    err = e.to_string();
                }
            }
            if let Some(tx) = tx {
                let _ = tx.send(Err(err));
            }
        });
        let last_err = Arc::new(Mutex::new(String::new()));
        let (e_out, e_last) = (child.stderr.take().unwrap(), last_err.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(e_out).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    *e_last.lock().unwrap() = line;
                }
            }
        });
        let reaped = std::thread::spawn(move || child.wait());
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(url)) => Ok(Self {
                server,
                url,
                token,
                group,
            }),
            other => {
                let _ = kill_group(group);
                let _ = reaped.join();
                let stderr = last_err.lock().unwrap().clone();
                let why = match other {
                    Ok(Err(e)) if !e.is_empty() => e,
                    Err(_) => "não deu a URL em 20 s".into(),
                    _ if !stderr.is_empty() => stderr,
                    _ => "saiu sem dar a URL".into(),
                };
                Err(format!("ngrok: {}", why.replace("https://", "")))
            }
        }
    }

    /// A URL sem o token, para mostrar.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// O que vai no QR: o token no fragmento, que não sai do navegador.
    pub fn pair_url(&self) -> String {
        format!("{}/#t={}", self.url, self.token)
    }

    pub fn repair(&mut self) {
        self.token = self.server.repair();
    }
}

fn kill_group(group: u32) -> io::Result<()> {
    std::process::Command::new("kill")
        .args(["-TERM", "--", &format!("-{group}")])
        .stderr(std::process::Stdio::null())
        .status()
        .map(drop)
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = kill_group(self.group);
    }
}

/// A matriz do QR (`true` = módulo escuro), já com a borda branca.
pub fn qr(text: &str) -> Vec<Vec<bool>> {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return Vec::new();
    };
    let w = code.width();
    let colors = code.into_colors();
    let quiet = 2;
    (0..w + 2 * quiet)
        .map(|y| {
            (0..w + 2 * quiet)
                .map(|x| {
                    (quiet..w + quiet).contains(&x)
                        && (quiet..w + quiet).contains(&y)
                        && colors[(y - quiet) * w + (x - quiet)] == qrcode::Color::Dark
                })
                .collect()
        })
        .collect()
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

    type Log = std::sync::Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    fn fake(log: Log) -> impl Fn(&str, serde_json::Value) -> io::Result<serde_json::Value> {
        move |m: &str, p: serde_json::Value| {
            log.lock().unwrap().push((m.to_string(), p));
            Ok(match m {
                "agent.list" => serde_json::json!({ "agents": [
                    { "pane_id": "w1:p1", "workspace_id": "w1", "agent": "claude", "agent_status": "blocked",
                      "cwd": "/nao/existe", "terminal_title_stripped": "✳ pediu permissão" }
                ]}),
                "workspace.list" => {
                    serde_json::json!({ "workspaces": [{ "workspace_id": "w1", "label": "fix/slack" }] })
                }
                "agent.read" => {
                    serde_json::json!({ "read": { "text": "Do you want to proceed?" } })
                }
                _ => serde_json::json!({}),
            })
        }
    }

    fn paired() -> (Mutex<Auth>, String, Instant) {
        let now = Instant::now();
        let (a, token) = Auth::new(now);
        let a = Mutex::new(a);
        let c = handle(&pair_req(&token), &a, now, &no_herdr)
            .cookie
            .unwrap();
        (a, c, now)
    }

    fn post(path: &str, cookie: &str, body: &str) -> Request {
        req(&format!(
            "POST {path} HTTP/1.1\r\nCookie: s={cookie}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    #[test]
    fn lista_os_agentes_com_a_feature() {
        let (a, c, now) = paired();
        let log = Log::default();
        let r = handle(&with_cookie("/api/agents", &c), &a, now, &fake(log.clone()));
        assert_eq!(r.status, 200);
        let v: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(v["agents"][0]["pane"], "w1:p1");
        assert_eq!(v["agents"][0]["label"], "fix/slack");
        assert_eq!(v["agents"][0]["status"], "blocked");
        assert_eq!(v["agents"][0]["title"], "pediu permissão");
    }

    #[test]
    fn tela_tecla_e_prompt_vao_ao_pane_do_agente() {
        let (a, c, now) = paired();
        let log = Log::default();
        let f = fake(log.clone());
        let r = handle(&with_cookie("/api/agents/w1%3Ap1/screen", &c), &a, now, &f);
        assert_eq!(r.status, 200);
        assert!(String::from_utf8_lossy(&r.body).contains("proceed"));
        assert_eq!(
            handle(
                &post("/api/agents/w1%3Ap1/keys", &c, r#"{"key":"1"}"#),
                &a,
                now,
                &f
            )
            .status,
            200
        );
        assert_eq!(
            handle(
                &post(
                    "/api/agents/w1:p1/prompt",
                    &c,
                    r#"{"text":"roda os testes"}"#
                ),
                &a,
                now,
                &f
            )
            .status,
            200
        );
        let log = log.lock().unwrap();
        let sent: Vec<_> = log.iter().filter(|(m, _)| m != "agent.list").collect();
        assert_eq!(sent[0].1["target"], "w1:p1");
        assert_eq!(
            sent[1],
            &(
                "agent.send_keys".to_string(),
                serde_json::json!({ "target": "w1:p1", "keys": ["1"] })
            )
        );
        assert_eq!(sent[2].1["text"], "roda os testes");
    }

    #[test]
    fn recusa_tecla_fora_da_lista_e_pane_que_nao_e_agente() {
        let (a, c, now) = paired();
        let log = Log::default();
        let f = fake(log.clone());
        assert_eq!(
            handle(
                &post("/api/agents/w1:p1/keys", &c, r#"{"key":"ctrl+c"}"#),
                &a,
                now,
                &f
            )
            .status,
            400
        );
        let shell = handle(
            &post("/api/agents/w1:p2/prompt", &c, r#"{"text":"rm -rf /"}"#),
            &a,
            now,
            &f,
        );
        assert_eq!(shell.status, 404);
        assert_eq!(String::from_utf8_lossy(&shell.body), GONE);
        assert_eq!(
            handle(&with_cookie("/api/agents/w1:p1/qualquer", &c), &a, now, &f).status,
            404
        );
        assert!(log.lock().unwrap().iter().all(|(m, _)| m == "agent.list"));
    }

    fn fake_ngrok(name: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("maestro-remote-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("ngrok");
        let log = dir.join("args");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\necho \"$$ $*\" > '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log)
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn ngrok_da_a_url_sem_inspetor_e_morre_no_drop() {
        let (bin, log) = fake_ngrok(
            "ok",
            r#"echo '{"lvl":"info","msg":"started tunnel","url":"https://abc.ngrok-free.app"}'; exec sleep 30"#,
        );
        let r = Remote::start(bin.to_str().unwrap(), no_herdr).unwrap();
        assert_eq!(r.url(), "https://abc.ngrok-free.app");
        assert!(r.pair_url().starts_with("https://abc.ngrok-free.app/#t="));
        let args = std::fs::read_to_string(&log).unwrap();
        let pid = args.split_whitespace().next().unwrap().to_string();
        assert!(
            args.contains(&format!(
                "http 127.0.0.1:{} --inspect=false --log stdout --log-format json",
                r.server.addr.port()
            )),
            "{args}"
        );
        assert!(alive(&pid));
        drop(r);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive(&pid));
        let _ = std::fs::remove_dir_all(bin.parent().unwrap());
    }

    #[test]
    fn ngrok_que_falha_vira_erro_legivel() {
        let (bin, _) = fake_ngrok(
            "err",
            r#"echo '{"lvl":"eror","msg":"session closing","err":"authentication failed: Your authtoken is invalid"}'; echo 'ERROR: authtoken inválido' >&2; exit 1"#,
        );
        let e = Remote::start(bin.to_str().unwrap(), no_herdr)
            .err()
            .unwrap();
        assert_eq!(e, "ngrok: authentication failed: Your authtoken is invalid");
        let (bin2, _) = fake_ngrok("stderr", "echo 'ERROR: sem rede' >&2; exit 1");
        assert_eq!(
            Remote::start(bin2.to_str().unwrap(), no_herdr)
                .err()
                .unwrap(),
            "ngrok: ERROR: sem rede"
        );
        let _ = std::fs::remove_dir_all(bin.parent().unwrap());
        let _ = std::fs::remove_dir_all(bin2.parent().unwrap());
    }

    #[test]
    fn vigia_mata_o_ngrok_quando_o_maestro_morre() {
        let (bin, log) = fake_ngrok("watch", "exec sleep 30");
        let mut maestro = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut watch = std::process::Command::new("sh")
            .args([
                "-c",
                WATCH,
                "sh",
                &maestro.id().to_string(),
                bin.to_str().unwrap(),
            ])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let pid = std::fs::read_to_string(&log)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_string();
        assert!(alive(&pid));
        maestro.kill().unwrap();
        maestro.wait().unwrap();
        watch.wait().unwrap();
        assert!(!alive(&pid));
        let _ = std::fs::remove_dir_all(bin.parent().unwrap());
    }

    #[test]
    fn qr_tem_borda_e_os_quadrados_de_canto() {
        let m = qr("https://abc.ngrok-free.app/#t=0123456789abcdef0123456789abcdef");
        let n = m.len();
        assert!(n > 25 && m.iter().all(|r| r.len() == n));
        assert!(!m[0][0] && !m[1][1]);
        assert!(m[2][2] && m[2][8] && m[8][2] && !m[3][3] && m[4][4]);
        assert!(m[2][n - 3] && m[n - 3][2]);
    }

    #[test]
    fn herdr_fora_vira_502() {
        let (a, c, now) = paired();
        assert_eq!(
            handle(&with_cookie("/api/agents", &c), &a, now, &no_herdr).status,
            502
        );
    }
}

/// Contra a sessão `maestro-test` com um agente simulado em `w1:p1` (claude → cat) e um shell em `w1:p2`:
/// `MAESTRO_HERDR_SESSION=maestro-test cargo test -- --ignored remote_herdr`
#[cfg(test)]
mod remote_herdr {
    use super::*;

    fn http(addr: SocketAddr, raw: String) -> (u16, String, String) {
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        let status = head[9..12].parse().unwrap();
        (status, head.to_string(), body.to_string())
    }

    #[test]
    #[ignore]
    fn ponta_a_ponta_no_herdr_de_teste() {
        assert_eq!(
            std::env::var("MAESTRO_HERDR_SESSION").as_deref(),
            Ok("maestro-test")
        );
        let herdr = crate::herdr::Herdr::from_env().unwrap();
        let (srv, token) = Server::start(move |m, p| herdr.call(m, p)).unwrap();
        let body = format!("{{\"token\":\"{token}\"}}");
        let (st, head, _) = http(
            srv.addr,
            format!(
                "POST /api/pair HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert_eq!(st, 200);
        let cookie = head
            .split("s=")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let get = |p: &str| {
            http(
                srv.addr,
                format!("GET {p} HTTP/1.1\r\nCookie: s={cookie}\r\n\r\n"),
            )
        };
        let post = |p: &str, b: &str| {
            http(
                srv.addr,
                format!(
                    "POST {p} HTTP/1.1\r\nCookie: s={cookie}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
                    b.len()
                ),
            )
        };

        let (st, _, list) = get("/api/agents");
        assert_eq!(st, 200, "{list}");
        assert!(list.contains("\"label\":\"fix/slack-demo\""), "{list}");
        assert!(list.contains("\"status\":\"blocked\""), "{list}");

        assert_eq!(
            post("/api/agents/w1%3Ap1/prompt", r#"{"text":"oi do celular"}"#).0,
            200
        );
        for k in ["1", "enter", "esc"] {
            assert_eq!(
                post("/api/agents/w1%3Ap1/keys", &format!("{{\"key\":\"{k}\"}}")).0,
                200,
                "{k}"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
        let (st, _, screen) = get("/api/agents/w1%3Ap1/screen");
        assert_eq!(st, 200);
        assert!(screen.contains("oi do celular"), "{screen}");

        assert_eq!(
            post("/api/agents/w1%3Ap1/keys", r#"{"key":"ctrl+c"}"#).0,
            400
        );
        assert_eq!(
            post("/api/agents/w1%3Ap2/prompt", r#"{"text":"ls"}"#).0,
            404
        );
        assert_eq!(get("/api/agents/w1%3Ap2/screen").0, 404);
    }
}
