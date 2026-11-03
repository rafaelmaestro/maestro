//! Revisão automática por IA: config, banco de produção só leitura e o guard de SQL do hook.

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
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
