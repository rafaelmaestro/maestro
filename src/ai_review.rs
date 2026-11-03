//! Revisão automática por IA: config, banco de produção só leitura e o guard de SQL do hook.

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub daily_budget: f64,
    pub auto_comment: bool,
    /// prefixos de path que nunca são aprovados sozinhos
    pub never_approve: Vec<String>,
    /// nomes (ou ids) de conexão do dbx com credencial só de leitura
    pub read_only_connections: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            enabled: false,
            daily_budget: 20.,
            auto_comment: true,
            never_approve: vec!["ctbz/devops/".into()],
            read_only_connections: vec![],
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::var_os("MAESTRO_AI_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config/maestro/ai-review.json"))
}

impl Config {
    pub fn save(&self) {
        let p = config_path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap());
    }

    pub fn load() -> Config {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// O servidor `dbx` do `~/.claude.json`, para o revisor subir só ele.
fn dbx_server() -> Option<Value> {
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(home().join(".claude.json")).ok()?).ok()?;
    let s = &cfg["mcpServers"]["dbx"];
    (!s.is_null()).then(|| s.clone())
}

/// O que o `claude -p` do revisor recebe quando há banco: o MCP, o hook que barra escrita e as conexões.
pub struct Db {
    pub mcp: String,
    pub settings: String,
    pub connections: Vec<String>,
}

pub const DB_TOOLS: &[&str] = &[
    "mcp__dbx__dbx_execute_query",
    "mcp__dbx__dbx_describe_table",
    "mcp__dbx__dbx_list_tables",
    "mcp__dbx__dbx_list_databases",
    "mcp__dbx__dbx_get_schema_context",
];

pub fn db(cfg: &Config) -> Option<Db> {
    if cfg.read_only_connections.is_empty() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let exe = crate::feature::sh_quote(&exe.to_string_lossy());
    Some(Db {
        mcp: json!({ "mcpServers": { "dbx": dbx_server()? } }).to_string(),
        settings: json!({ "hooks": { "PreToolUse": [ {
            "matcher": "mcp__dbx__.*",
            "hooks": [ { "type": "command", "command": format!("{exe} sql-guard") } ]
        } ] } })
        .to_string(),
        connections: cfg.read_only_connections.clone(),
    })
}

const WRITE_WORDS: &[&str] = &[
    "insert",
    "update",
    "delete",
    "merge",
    "upsert",
    "drop",
    "alter",
    "create",
    "truncate",
    "grant",
    "revoke",
    "copy",
    "call",
    "do",
    "set",
    "reset",
    "begin",
    "start",
    "commit",
    "rollback",
    "savepoint",
    "vacuum",
    "lock",
    "refresh",
    "reindex",
    "cluster",
    "comment",
    "execute",
    "prepare",
    "deallocate",
    "listen",
    "notify",
    "discard",
    "import",
    "into",
    "replace",
    "rename",
    "attach",
    "detach",
    "pragma",
    "load",
];

fn strip_comments(sql: &str) -> String {
    let mut out = String::new();
    let mut rest = sql;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("--") {
            rest = r.find('\n').map_or("", |i| &r[i..]);
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.find("*/").map_or("", |i| &r[i + 2..]);
            out.push(' ');
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// `None` = é uma consulta só de leitura. Sem parser: palavra suspeita em qualquer lugar barra.
pub fn read_only_sql(sql: &str) -> Option<String> {
    let s = strip_comments(sql).to_lowercase();
    let s = s.trim().trim_end_matches(';').trim();
    if s.contains(';') {
        return Some("uma consulta por vez".into());
    }
    let words: Vec<&str> = s
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    if !matches!(
        words.first().copied(),
        Some("select" | "with" | "explain" | "show" | "table" | "values")
    ) {
        return Some("só SELECT/WITH/EXPLAIN".into());
    }
    if let Some(w) = words.iter().find(|w| {
        WRITE_WORDS.contains(w)
            || w.starts_with("pg_")
            || w.starts_with("lo_")
            || w.starts_with("dblink")
            || **w == "set_config"
    }) {
        return Some(format!("`{w}` não é de leitura"));
    }
    None
}

/// O hook PreToolUse (`maestro sql-guard`): conexão fora da lista, sessão ou SQL de escrita → nega.
pub fn sql_guard(stdin: &str, allowed: &[String]) -> Option<String> {
    let v: Value = serde_json::from_str(stdin).ok()?;
    let tool = v["tool_name"].as_str().unwrap_or_default();
    let i = &v["tool_input"];
    if !DB_TOOLS.contains(&tool) {
        return Some(format!("{tool} não é permitido na revisão"));
    }
    if !i["session_id"].is_null() {
        return Some("sem sessão: passe connection_name".into());
    }
    let conn = i["connection_name"]
        .as_str()
        .or(i["connection_id"].as_str())
        .unwrap_or_default();
    if !allowed.iter().any(|a| a == conn) {
        return Some(format!(
            "conexão `{conn}` não está entre as somente leitura (use uma destas: {})",
            allowed.join(", ")
        ));
    }
    if tool == "mcp__dbx__dbx_execute_query" {
        return read_only_sql(i["sql"].as_str().unwrap_or_default());
    }
    None
}

pub fn guard_main(stdin: &str) {
    if let Some(r) = sql_guard(stdin, &Config::load().read_only_connections) {
        println!(
            "{}",
            json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": r
            } })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_leitura() {
        assert_eq!(
            read_only_sql("select count(*) from t where created_at > now()"),
            None
        );
        assert_eq!(
            read_only_sql("-- x\nWITH a AS (select 1) select * from a;"),
            None
        );
        for bad in [
            "delete from t",
            "select 1; drop table t",
            "with x as (delete from t returning *) select * from x",
            "select * into novo from t",
            "select pg_sleep(10)",
            "select set_config('transaction_read_only','off',false)",
            "/* select */ update t set a=1",
            "begin read write",
        ] {
            assert!(read_only_sql(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn guard_do_hook() {
        let ok = vec!["PRD - bots_api".to_string()];
        let call = |tool: &str, input: Value| {
            sql_guard(
                &json!({"tool_name": tool, "tool_input": input}).to_string(),
                &ok,
            )
        };
        assert_eq!(
            call(
                "mcp__dbx__dbx_execute_query",
                json!({"connection_name":"PRD - bots_api","sql":"select 1"})
            ),
            None
        );
        assert!(
            call(
                "mcp__dbx__dbx_execute_query",
                json!({"connection_name":"PRD - outro","sql":"select 1"})
            )
            .is_some()
        );
        assert!(
            call(
                "mcp__dbx__dbx_execute_query",
                json!({"connection_name":"PRD - bots_api","sql":"delete from t"})
            )
            .is_some()
        );
        assert!(
            call(
                "mcp__dbx__dbx_execute_query",
                json!({"session_id":"s","connection_name":"PRD - bots_api","sql":"select 1"})
            )
            .is_some()
        );
        assert!(
            call(
                "mcp__dbx__dbx_execute_batch",
                json!({"connection_name":"PRD - bots_api"})
            )
            .is_some()
        );
        assert_eq!(
            call(
                "mcp__dbx__dbx_list_tables",
                json!({"connection_name":"PRD - bots_api"})
            ),
            None
        );
    }

    #[test]
    fn config_com_default() {
        let c: Config = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(c.enabled && c.auto_comment);
        assert_eq!(c.never_approve, ["ctbz/devops/"]);
        assert!(db(&c).is_none());
    }
}

// ---------- fila automática e gate ----------

use crate::mrs::{self, Finding, Report, Row};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub sha: String,
    /// `queue`, `run`, `skip` ou `done`
    pub status: String,
    pub why: String,
    pub report: Option<Report>,
    pub cost: f64,
    pub at: u64,
    pub approved_sha: Option<String>,
    pub posted: usize,
    /// a barra de comentário já rodou nesta sha
    pub commented: bool,
}

impl Entry {
    /// A pill do protótipo (`AIP`): (tipo, texto). `ok`, `bad`, `inc` ou vazio.
    pub fn pill(&self) -> (&'static str, String) {
        match self.status.as_str() {
            "run" => ("", "⟳ revisando".into()),
            "queue" => ("", "na fila".into()),
            "skip" | "ignored" => ("", "pulado".into()),
            _ if self.approved_sha.as_deref() == Some(self.sha.as_str()) => {
                ("ok", "aprovado pela IA".into())
            }
            _ if self.report.as_ref().is_some_and(|r| r.verdict == "changes") => {
                ("bad", format!("pediu mudanças · {}", self.posted))
            }
            _ if self.posted > 0 => ("inc", format!("comentou · {}", self.posted)),
            _ => ("inc", "revise você".into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub day: String,
    pub spent: f64,
    pub following_at: u64,
    pub following: Vec<String>,
    pub mrs: BTreeMap<String, Entry>,
}

pub fn state_path() -> PathBuf {
    std::env::var_os("MAESTRO_AI_REVIEW")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share/maestro/ai-review.json"))
}

impl State {
    pub fn load() -> State {
        std::fs::read_to_string(state_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        let p = state_path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string(self).unwrap());
    }
}

/// O que o gate olha, tudo já lido do GitLab.
#[derive(Clone, Copy)]
pub struct Facts<'a> {
    pub report: &'a Report,
    pub path: &'a str,
    pub author: &'a str,
    pub reviewed_sha: &'a str,
    pub head_sha: &'a str,
    pub draft: bool,
    pub ci_ok: bool,
    pub i_approved: bool,
    pub prev_approved_sha: Option<&'a str>,
    pub following: &'a [String],
    pub cfg: &'a Config,
}

const MIN_CONFIDENCE: f64 = 0.8;

/// O veredito do modelo só propõe: quem aprova é esta função.
pub fn may_approve(f: &Facts) -> Result<(), String> {
    let r = f.report;
    if r.verdict != "approve" {
        return Err(if r.summary.is_empty() {
            "a IA não chegou num veredito".into()
        } else {
            r.summary.clone()
        });
    }
    if r.confidence < MIN_CONFIDENCE {
        return Err(format!("confiança baixa ({:.0}%)", r.confidence * 100.));
    }
    if r.findings
        .iter()
        .any(|x| matches!(x.severity.as_str(), "blocker" | "major"))
    {
        return Err("tem achado blocker/major".into());
    }
    if let Some(c) = r.claims.iter().find(|c| c.status != "confirmed") {
        return Err(format!("afirmação não confirmada: {}", c.claim));
    }
    if r.checks.is_empty() || r.checks.iter().any(|c| c.status != "pass") {
        return Err("nem todos os checks passaram".into());
    }
    if f.draft {
        return Err("MR em draft".into());
    }
    if !f.ci_ok {
        return Err("pipeline não está verde".into());
    }
    if f.head_sha != f.reviewed_sha {
        return Err("o MR recebeu commit depois da revisão".into());
    }
    if f.i_approved {
        return Err("você já aprovou".into());
    }
    if f.prev_approved_sha == Some(f.reviewed_sha) {
        return Err("você desaprovou esta versão".into());
    }
    if let Some(p) = f
        .cfg
        .never_approve
        .iter()
        .find(|p| f.path.starts_with(p.as_str()))
    {
        return Err(format!("repo em {p}: nunca aprova sozinha"));
    }
    if !f.following.iter().any(|u| u == f.author) {
        return Err(format!("você não segue @{}", f.author));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Approve,
    Comment(Vec<Finding>, String),
    Human(String),
}

pub fn decide(f: &Facts) -> Action {
    let why = match may_approve(f) {
        Ok(()) => return Action::Approve,
        Err(w) => w,
    };
    if f.cfg.auto_comment {
        let c = bar(f.report);
        if !c.is_empty() {
            return Action::Comment(c, why);
        }
    }
    Action::Human(why)
}

/// Os achados que passam na barra de comentário (seção 6 da skill).
fn bar(r: &Report) -> Vec<Finding> {
    r.findings
        .iter()
        .filter(|x| x.post && !x.outside && x.line.is_some())
        .filter(|x| {
            matches!(x.severity.as_str(), "blocker" | "major")
                || (x.severity == "minor" && matches!(x.category.as_str(), "sec" | "contract"))
        })
        .cloned()
        .collect()
}

/// Publica os achados como uma review só; devolve quantos saíram e o erro, se houve.
fn post(
    r: &Row,
    sha: &str,
    opened: &Result<mrs::Opened, String>,
    f: &[Finding],
) -> (usize, Result<(), String>) {
    let mut drafts: Vec<mrs::Draft> = f
        .iter()
        .enumerate()
        .map(|(i, x)| mrs::Draft {
            id: i,
            file: x.file.clone(),
            line: x.line,
            old: None,
            body: x.body.clone(),
            by: "Claude".into(),
            sev: None,
            label: None,
            head: sha.to_string(),
            gitlab_id: None,
            error: None,
            general: false,
            reply_to: None,
        })
        .collect();
    let res = opened
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|o| mrs::publish(&r.path, r.iid, o, &mut drafts));
    (drafts.iter().filter(|d| d.gitlab_id.is_some()).count(), res)
}

fn posted_why(n: usize, res: &Result<(), String>, why: &str) -> String {
    match res {
        Ok(()) => format!(
            "{n} comentário{} publicado{} · {why}",
            if n == 1 { "" } else { "s" },
            if n == 1 { "" } else { "s" }
        ),
        Err(err) => format!("comentários não saíram ({err}) · {why}"),
    }
}

/// MRs revisados antes de a IA comentar no "revise você": publica do relatório salvo, sem rever.
fn backfill(st: &mut State, rows: &[Row]) {
    let me = mrs::me().unwrap_or_default();
    let me = me.as_str();
    for r in rows {
        let Some(e) = st.mrs.get_mut(&r.key()) else {
            continue;
        };
        if e.commented || e.status != "done" || e.sha != r.sha {
            continue;
        }
        e.commented = true;
        let Some(rep) = &e.report else { continue };
        let f = bar(rep);
        if f.is_empty() || e.posted > 0 || e.approved_sha.as_deref() == Some(e.sha.as_str()) {
            continue;
        }
        let base = format!("projects/{}/merge_requests/{}", mrs::enc(&r.path), r.iid);
        let notes = mrs::read(&format!("{base}/notes?per_page=100"), true).unwrap_or_default();
        let mine = |n: &Value| n["system"] != true && n["author"]["username"].as_str() == Some(me);
        if me.is_empty() || notes.as_array().is_none_or(|v| v.iter().any(mine)) {
            continue;
        }
        let opened = mrs::open(&r.path, r.iid);
        let head = opened.as_ref().map_or("", |o| {
            o.detail["diff_refs"]["head_sha"]
                .as_str()
                .unwrap_or_default()
        });
        if head != e.sha {
            continue;
        }
        let (n, res) = post(r, &e.sha, &opened, &f);
        e.posted = n;
        e.why = posted_why(n, &res, &e.why);
    }
}

fn following() -> Result<Vec<String>, String> {
    let id = mrs::read("user", false)?["id"].as_u64().ok_or("sem id")?;
    let v = mrs::read(&format!("users/{id}/following?per_page=100"), true)?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| u["username"].as_str().map(String::from))
        .collect())
}

/// "Pular": some da aba IA até o MR receber commit novo.
pub fn ignore(key: &str, sha: &str) {
    let mut st = State::load();
    set(&mut st, key, sha, "ignored", "pulado por você");
    st.save();
}

const FAILED: &str = "revisão falhou: ";

fn set(st: &mut State, key: &str, sha: &str, status: &str, why: &str) {
    let e = st.mrs.entry(key.to_string()).or_default();
    if e.sha != sha {
        *e = Entry {
            approved_sha: e.approved_sha.take(),
            ..Default::default()
        };
    }
    e.sha = sha.into();
    e.status = status.into();
    e.why = why.into();
}

/// Atualiza o estado de todos e escolhe o próximo MR a revisar (já salvo como `run`).
/// `force` roda mesmo com a fila pausada (o botão "Revisar agora"); `only` revisa só esse MR,
/// mesmo já revisado, em draft ou com CI vermelho.
pub fn next(rows: &[Row], force: bool, only: Option<&str>) -> Option<(Row, Value)> {
    let cfg = Config::load();
    if !cfg.enabled && !force {
        return None;
    }
    let mut st = State::load();
    let today = crate::vault::today();
    if st.day != today {
        st.day = today;
        st.spent = 0.;
    }
    let now = crate::news::now();
    if now.saturating_sub(st.following_at) > 3600
        && let Ok(f) = following()
    {
        st.following = f;
        st.following_at = now;
    }
    if cfg.auto_comment && only.is_none() {
        backfill(&mut st, rows);
    }
    let mut pick = None;
    for r in rows.iter().filter(|r| !r.sha.is_empty()) {
        let key = r.key();
        let forced = only == Some(key.as_str());
        if only.is_some() && !forced {
            continue;
        }
        if !forced
            && st.mrs.get(&key).is_some_and(|e| {
                e.sha == r.sha
                    && (e.status == "done"
                        || e.status == "ignored"
                        || e.why.starts_with(FAILED) && now.saturating_sub(e.at) < 3600)
            })
        {
            continue;
        }
        let base = format!("projects/{}/merge_requests/{}", mrs::enc(&r.path), r.iid);
        let Ok(detail) = mrs::read(&base, false) else {
            continue;
        };
        let (status, why) = if forced && st.spent < cfg.daily_budget {
            pick = Some((r.clone(), detail));
            ("run", "revisando")
        } else if detail["draft"] == true {
            ("skip", "MR em draft")
        } else {
            match mrs::ci_of(&detail) {
                crate::mr::Status::Fail => ("skip", "pipeline falhou"),
                crate::mr::Status::Running => ("queue", "esperando o CI ficar verde"),
                _ if st.spent >= cfg.daily_budget => ("queue", "teto do dia atingido"),
                _ if pick.is_some() => ("queue", "na fila"),
                _ => {
                    pick = Some((r.clone(), detail));
                    ("run", "revisando")
                }
            }
        };
        set(&mut st, &key, &r.sha, status, why);
    }
    st.save();
    pick
}

/// Revisa o MR escolhido por `next`. Devolve (chave, texto) para notificar quando precisa de você
/// e se a revisão chegou ao fim.
pub fn run(r: Row, detail: Value) -> (Option<(String, String)>, bool) {
    let cfg = Config::load();
    let now = crate::news::now();
    let st = State::load();
    let key = r.key();
    let prev = st.mrs.get(&key).cloned().unwrap_or_default();
    let since = prev
        .report
        .as_ref()
        .filter(|p| p.verdict == "approve" && prev.status == "done")
        .map(|_| prev.sha.clone());
    let base = format!("projects/{}/merge_requests/{}", mrs::enc(&r.path), r.iid);
    let notes = mrs::read(&format!("{base}/notes?per_page=100"), true).unwrap_or_default();
    let out = std::sync::Mutex::new(Err("o Claude parou sem devolver o relatório".to_string()));
    mrs::review(
        &r.path,
        r.iid,
        &r.title,
        &r.target,
        detail["description"].as_str().unwrap_or_default(),
        &notes,
        since,
        |m| match m {
            mrs::Run::Done(rep, c) => *out.lock().unwrap() = Ok((rep, c)),
            mrs::Run::Failed(e) => *out.lock().unwrap() = Err(e),
            _ => {}
        },
    );
    let mut st = State::load();
    let (mut rep, cost) = match out.into_inner().unwrap() {
        Ok(x) => x,
        Err(e) => {
            set(&mut st, &key, &r.sha, "skip", &format!("{FAILED}{e}"));
            if let Some(e) = st.mrs.get_mut(&key) {
                e.at = now;
            }
            st.save();
            return (None, false);
        }
    };
    st.spent += cost;
    let opened = mrs::open(&r.path, r.iid);
    let raw: Vec<(String, String)> = opened
        .as_ref()
        .map(|o| o.raw.iter().map(|x| (x.0.clone(), x.2.clone())).collect())
        .unwrap_or_default();
    mrs::mark_outside(&mut rep.findings, &raw);
    let head = opened.as_ref().map_or(String::new(), |o| {
        o.detail["diff_refs"]["head_sha"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    let action = decide(&Facts {
        report: &rep,
        path: &r.path,
        author: r.who.trim_start_matches('@'),
        reviewed_sha: &r.sha,
        head_sha: &head,
        draft: opened.as_ref().map_or(true, |o| o.detail["draft"] == true),
        ci_ok: opened
            .as_ref()
            .is_ok_and(|o| mrs::ci_of(&o.detail) == crate::mr::Status::Ok),
        i_approved: opened
            .as_ref()
            .is_ok_and(|o| o.approvals["user_has_approved"] == true),
        prev_approved_sha: prev.approved_sha.as_deref(),
        following: &st.following,
        cfg: &cfg,
    });
    set(&mut st, &key, &r.sha, "done", "");
    let e = st.mrs.get_mut(&key).unwrap();
    let notice = match action {
        Action::Approve => match mrs::approve(&r.path, r.iid, &r.sha) {
            Ok(()) => {
                e.approved_sha = Some(r.sha.clone());
                e.why = rep.summary.clone();
                None
            }
            Err(err) => {
                e.why = format!("não consegui aprovar: {err}");
                Some("não consegui aprovar".to_string())
            }
        },
        Action::Comment(f, why) => {
            let (n, res) = post(&r, &r.sha, &opened, &f);
            e.posted = n;
            e.why = posted_why(n, &res, &why);
            let head = if rep.verdict == "changes" {
                "pediu mudanças"
            } else {
                "revise você"
            };
            Some(match &res {
                Ok(()) => format!("{head} · {}", e.why.split(" · ").next().unwrap_or_default()),
                Err(_) => format!("{head} · comentários não saíram"),
            })
        }
        Action::Human(why) => {
            let short = if why.chars().count() <= 60 {
                format!("revise você · {why}")
            } else {
                "revise você".into()
            };
            e.why = why;
            Some(short)
        }
    };
    e.report = Some(rep);
    e.commented = true;
    e.cost = cost;
    e.at = now;
    st.save();
    (
        notice.map(|n| (key, format!("{} !{} · {n}", r.proj(), r.iid))),
        true,
    )
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::mrs::Check;

    fn ok_report() -> Report {
        Report {
            verdict: "approve".into(),
            confidence: 0.9,
            checks: ["correct", "sec"]
                .iter()
                .map(|d| Check {
                    dimension: d.to_string(),
                    status: "pass".into(),
                    note: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn gate_de_aprovacao() {
        let cfg = Config::default();
        let fol = vec!["ana".to_string()];
        let rep = ok_report();
        let base = || Facts {
            report: &rep,
            path: "ctbz/experts/bots-api",
            author: "ana",
            reviewed_sha: "s1",
            head_sha: "s1",
            draft: false,
            ci_ok: true,
            i_approved: false,
            prev_approved_sha: None,
            following: &fol,
            cfg: &cfg,
        };
        assert_eq!(decide(&base()), Action::Approve);
        let no = |f: Facts| may_approve(&f).unwrap_err();
        assert!(
            no(Facts {
                author: "zé",
                ..base()
            })
            .contains("não segue")
        );
        assert!(
            no(Facts {
                path: "ctbz/devops/infra",
                ..base()
            })
            .contains("ctbz/devops/")
        );
        assert!(
            no(Facts {
                head_sha: "s2",
                ..base()
            })
            .contains("commit")
        );
        assert!(
            no(Facts {
                draft: true,
                ..base()
            })
            .contains("draft")
        );
        assert!(
            no(Facts {
                ci_ok: false,
                ..base()
            })
            .contains("pipeline")
        );
        assert!(
            no(Facts {
                i_approved: true,
                ..base()
            })
            .contains("já aprovou")
        );
        assert!(
            no(Facts {
                prev_approved_sha: Some("s1"),
                ..base()
            })
            .contains("desaprovou")
        );
        let low = Report {
            confidence: 0.5,
            ..ok_report()
        };
        assert!(
            no(Facts {
                report: &low,
                ..base()
            })
            .contains("confiança")
        );
        let claim = Report {
            claims: vec![mrs::Claim {
                claim: "x".into(),
                status: "unverified".into(),
                ..Default::default()
            }],
            ..ok_report()
        };
        assert!(
            no(Facts {
                report: &claim,
                ..base()
            })
            .contains("afirmação")
        );
        let unk = Report {
            checks: vec![Check {
                status: "unknown".into(),
                ..Default::default()
            }],
            ..ok_report()
        };
        assert!(
            no(Facts {
                report: &unk,
                ..base()
            })
            .contains("checks")
        );
        let human = Report {
            verdict: "needs_human".into(),
            summary: "dúvida".into(),
            ..ok_report()
        };
        assert_eq!(
            decide(&Facts {
                report: &human,
                ..base()
            }),
            Action::Human("dúvida".into())
        );
    }

    #[test]
    fn comenta_quem_voce_nao_segue() {
        let cfg = Config::default();
        let f = |sev: &str, cat: &str, post: bool, line| Finding {
            file: "a".into(),
            line,
            severity: sev.into(),
            category: cat.into(),
            post,
            ..Default::default()
        };
        let rep = Report {
            verdict: "changes".into(),
            findings: vec![
                f("major", "correct", true, Some(1)),
                f("minor", "read", true, Some(2)),
                f("minor", "sec", true, Some(3)),
                f("major", "perf", false, Some(4)),
                f("blocker", "sec", true, None),
            ],
            ..ok_report()
        };
        let facts = Facts {
            report: &rep,
            path: "g/p",
            author: "zé",
            reviewed_sha: "s",
            head_sha: "s",
            draft: false,
            ci_ok: true,
            i_approved: false,
            prev_approved_sha: None,
            following: &[],
            cfg: &cfg,
        };
        let Action::Comment(c, _) = decide(&facts) else {
            panic!()
        };
        assert_eq!(
            c.iter().map(|x| x.line).collect::<Vec<_>>(),
            [Some(1), Some(3)]
        );
        let unsure = Report {
            verdict: "needs_human".into(),
            ..rep.clone()
        };
        assert!(matches!(
            decide(&Facts {
                report: &unsure,
                ..facts
            }),
            Action::Comment(..)
        ));
        let quiet = Config {
            auto_comment: false,
            ..Config::default()
        };
        assert!(matches!(
            decide(&Facts {
                cfg: &quiet,
                ..facts
            }),
            Action::Human(_)
        ));
    }

    #[test]
    fn pill_da_aba() {
        let e = |status: &str, verdict: &str, appr: Option<&str>| Entry {
            sha: "s".into(),
            status: status.into(),
            report: Some(Report {
                verdict: verdict.into(),
                ..Default::default()
            }),
            approved_sha: appr.map(String::from),
            posted: 2,
            ..Default::default()
        };
        assert_eq!(e("done", "approve", Some("s")).pill().1, "aprovado pela IA");
        assert_eq!(e("done", "needs_human", None).pill().1, "comentou · 2");
        let quiet = Entry {
            posted: 0,
            ..e("done", "approve", Some("old"))
        };
        assert_eq!(quiet.pill().1, "revise você");
        assert_eq!(e("done", "changes", None).pill().1, "pediu mudanças · 2");
        assert_eq!(e("queue", "", None).pill().1, "na fila");
    }

    #[test]
    fn entrada_zera_com_sha_nova_mas_guarda_o_approve() {
        let mut st = State::default();
        set(&mut st, "k", "s1", "done", "ok");
        st.mrs.get_mut("k").unwrap().approved_sha = Some("s1".into());
        st.mrs.get_mut("k").unwrap().posted = 2;
        set(&mut st, "k", "s2", "queue", "na fila");
        let e = &st.mrs["k"];
        assert_eq!(
            (e.posted, e.approved_sha.as_deref(), e.status.as_str()),
            (0, Some("s1"), "queue")
        );
        let round: State = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(round.mrs["k"].sha, "s2");
    }
}
