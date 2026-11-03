//! MRs de outras pessoas e o revisor: o port do `~/mr-review.sh`, uma armadilha por função.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};

use crate::mr::Status;

/// Uma linha da inbox (o `MRS` do protótipo).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `ctbz/experts/bots-api`
    pub path: String,
    pub iid: u64,
    pub title: String,
    pub who: String,
    pub sha: String,
    pub source: String,
    pub target: String,
    pub web_url: String,
    /// pipeline, lido do detalhe do MR (a lista não traz)
    pub ci: Option<Status>,
    /// `user_notes_count` e `updated_at`: mudou um dos dois, vale reler as discussões
    pub notes: u64,
    pub updated: String,
}

impl Row {
    pub fn proj(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
    pub fn key(&self) -> String {
        format!("{}!{}", self.path, self.iid)
    }
}

pub fn rows(list: &Value) -> Vec<Row> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|m| Row {
            path: m["references"]["full"]
                .as_str()
                .unwrap_or_default()
                .split('!')
                .next()
                .unwrap_or_default()
                .into(),
            iid: m["iid"].as_u64().unwrap_or_default(),
            title: m["title"].as_str().unwrap_or_default().into(),
            who: format!("@{}", m["author"]["username"].as_str().unwrap_or_default()),
            sha: m["sha"].as_str().unwrap_or_default().into(),
            source: m["source_branch"].as_str().unwrap_or_default().into(),
            target: m["target_branch"].as_str().unwrap_or_default().into(),
            web_url: m["web_url"].as_str().unwrap_or_default().into(),
            ci: None,
            notes: m["user_notes_count"].as_u64().unwrap_or_default(),
            updated: m["updated_at"].as_str().unwrap_or_default().into(),
        })
        .collect()
}

pub fn enc(s: &str) -> String {
    s.replace('/', "%2F")
}

// ---------- as 11 armadilhas do script ----------

/// 1. Linha de contexto precisa de `old_line` (senão 400 "line_code can't be blank"): o awk do script.
pub fn old_line(diff: &str, target: u32) -> Option<u32> {
    let (mut o, mut n) = (0u32, 0u32);
    for l in diff.lines() {
        if let Some(h) = l.strip_prefix("@@ ") {
            let mut parts = h.split_whitespace();
            let num = |p: Option<&str>, sign: char| {
                p.and_then(|x| x.strip_prefix(sign))
                    .and_then(|x| x.split(',').next())
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0)
            };
            o = num(parts.next(), '-');
            n = num(parts.next(), '+');
        } else if l.starts_with('-') {
            o += 1;
        } else if l.starts_with('+') {
            if n == target {
                return None;
            }
            n += 1;
        } else if l.starts_with('\\') {
            continue;
        } else {
            if n == target {
                return Some(o);
            }
            o += 1;
            n += 1;
        }
    }
    None
}

/// A linha nova aparece no diff (adicionada ou de contexto)?
pub fn in_diff(diff: &str, target: u32) -> bool {
    let mut n = 0u32;
    for l in diff.lines() {
        if let Some(h) = l.strip_prefix("@@ ") {
            n = h
                .split_whitespace()
                .nth(1)
                .and_then(|x| x.strip_prefix('+'))
                .and_then(|x| x.split(',').next())
                .and_then(|x| x.parse().ok())
                .unwrap_or(0);
        } else if l.starts_with('-') || l.starts_with('\\') {
            continue;
        } else {
            if n == target {
                return true;
            }
            n += 1;
        }
    }
    false
}

/// 2. `-H Content-Type`: sem ele a API devolve 415 (`--input -` não põe o header).
pub fn write_args(method: &str, endpoint: &str) -> Vec<String> {
    vec![
        "api".into(),
        "-X".into(),
        method.into(),
        "-H".into(),
        "Content-Type: application/json".into(),
        endpoint.into(),
        "--input".into(),
        "-".into(),
    ]
}

/// 3. O glab sai 0 até em HTTP 4xx: sucesso é ter `.id` na resposta.
pub fn created_id(out: &str) -> Result<String, String> {
    // nota tem id número; discussão (resolver) tem id texto
    serde_json::from_str::<Value>(out)
        .ok()
        .and_then(|v| match &v["id"] {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| out.chars().take(200).collect::<String>().replace('\n', " "))
}

/// 4. `refs/merge-requests/N/head` existe mesmo se a branch de origem já foi apagada.
pub fn refspecs(iid: u64, target: &str) -> Vec<String> {
    vec![
        format!("+refs/merge-requests/{iid}/head:refs/mr/{iid}"),
        format!("+refs/heads/{target}:refs/remotes/origin/{target}"),
    ]
}

/// 5. Basename que colide (dois `zendesk-agents`): pasta própria quando o origin não é este projeto.
pub fn repo_dir(repos: &Path, path: &str, origin: Option<&str>) -> PathBuf {
    let base = repos.join(path.rsplit('/').next().unwrap_or(path));
    let ours = origin.is_none_or(|o| {
        let o = o.trim().trim_end_matches(".git");
        o.ends_with(&format!(":{path}")) || o.ends_with(&format!("/{path}"))
    });
    if ours {
        base
    } else {
        repos.join(path.replace('/', "_"))
    }
}

/// 6. Pipeline vermelho ou rodando pede confirmação; sem CI passa.
pub fn pipeline_ok(status: &str) -> bool {
    matches!(status, "success" | "none" | "skipped" | "manual" | "")
}

/// 7. Todas as notas que já existem (humanas, GitLab Duo, bots), com a linha de cada uma, para o dedup.
pub fn notes_text(notes: &Value) -> String {
    let mut out = String::new();
    for n in notes.as_array().into_iter().flatten() {
        if n["system"] == true {
            continue;
        }
        let at = match n["position"]["new_path"].as_str() {
            Some(p) => format!(
                " em {p}:{}",
                n["position"]["new_line"]
                    .as_u64()
                    .or(n["position"]["old_line"].as_u64())
                    .map_or(String::new(), |l| l.to_string())
            ),
            None => String::new(),
        };
        out.push_str(&format!(
            "--- @{}{at}\n{}\n",
            n["author"]["username"].as_str().unwrap_or_default(),
            n["body"].as_str().unwrap_or_default()
        ));
    }
    out.chars().take(20000).collect()
}

pub fn prompt(
    iid: u64,
    title: &str,
    target: &str,
    desc: &str,
    notes: &str,
    since: Option<&str>,
) -> String {
    let diff = match since {
        Some(sha) => {
            format!("git diff {sha}...HEAD (só os commits novos desde a sua última revisão)")
        }
        None => format!("git diff origin/{target}...HEAD"),
    };
    let desc: String = desc.chars().take(3000).collect();
    format!(
        "Revise o MR !{iid}: \"{title}\". O diff é: {diff}

Descrição escrita pelo autor:
<<<DESC
{desc}
DESC

Comentários que JÁ existem neste MR — humanos e de outras ferramentas de revisão. Leia todos. Não repita nenhum ponto já levantado, nem reformulado com outras palavras. Se todos os seus achados já estão aí, devolva findings vazio.
<<<NOTES
{notes}
NOTES

Se houver skill de code review disponível (ex.: /code-review) ou instruções em CLAUDE.md / .claude/ deste repo, siga-as — elas mandam mais que este prompt.
Compare o que a descrição promete com o que o diff faz: sobra, falta ou divergência é achado.

Cada achado vira um comentário separado e tem que se sustentar sozinho.
- file/line: caminho relativo à raiz do repo e linha DO ARQUIVO NOVO, que precisa aparecer no diff (adicionada ou de contexto). Sem linha exata: line null.
- severity: blocker, major, minor ou nit. label: issue, suggestion, question, nitpick ou praise. category: uma das dimensões dos checks.
- body: markdown curto, o comentário como ele vai aparecer no MR. post: true só se passa na barra para virar comentário.
- verdict, confidence, summary, checks e claims: como a skill de revisão manda."
    )
}

/// O schema da saída do revisor (fim do `sed '/^```/d'`).
pub const SCHEMA: &str = r#"{"type":"object","properties":{"verdict":{"type":"string","enum":["approve","changes","needs_human"]},"confidence":{"type":"number","minimum":0,"maximum":1},"summary":{"type":"string"},"checks":{"type":"array","items":{"type":"object","properties":{"dimension":{"type":"string","enum":["correct","sec","read","perf","tests","contract","desc","inject"]},"status":{"type":"string","enum":["pass","fail","unknown"]},"note":{"type":"string"}},"required":["dimension","status","note"]}},"claims":{"type":"array","items":{"type":"object","properties":{"claim":{"type":"string"},"status":{"type":"string","enum":["confirmed","refuted","unverified"]},"source":{"type":"string"},"query":{"type":"string"},"result":{"type":"string"}},"required":["claim","status","source","query","result"]}},"findings":{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"line":{"type":["integer","null"]},"severity":{"type":"string","enum":["blocker","major","minor","nit"]},"category":{"type":"string"},"label":{"type":"string"},"body":{"type":"string"},"post":{"type":"boolean"}},"required":["file","line","severity","category","label","body","post"]}}},"required":["verdict","confidence","summary","checks","claims","findings"]}"#;

/// A skill do revisor; a rubrica do `~/.claude` (a mesma do `~/mr-review.sh`) entra depois como regra de tom.
pub const SKILL: &str = include_str!("review-skill.md");

/// 11. A skill e a rubrica no system prompt e só ferramentas de leitura; com `db`, o dbx atrás do `sql-guard`.
pub fn claude_args(rubric: Option<&Path>, db: Option<&crate::ai_review::Db>) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--json-schema",
        SCHEMA,
        "--max-budget-usd",
        "4.00",
        "--no-session-persistence",
        // roda no checkout do MR: o `.claude/` dele (hooks) não pode carregar; skills do usuário ficam
        "--setting-sources",
        "user",
        "--strict-mcp-config",
        "--tools",
        "Read,Grep,Glob,Bash,Skill,Agent",
        "--allowedTools",
        "Bash(git diff:*)",
        "Bash(git log:*)",
        "Bash(git blame:*)",
        "Bash(git show:*)",
        "Read",
        "Grep",
        "Glob",
        "Skill",
        "Agent",
    ]
    .map(String::from)
    .to_vec();
    let mut system = SKILL.to_string();
    match db {
        Some(d) => {
            a.extend(crate::ai_review::DB_TOOLS.iter().map(|t| t.to_string()));
            a.extend(["--mcp-config".into(), d.mcp.clone(), "--settings".into(), d.settings.clone()]);
            system.push_str(&format!(
                "\n## Banco de produção\n\nConexões do dbx só de leitura (passe sempre `connection_name`, nunca `session_id`): {}.\n",
                d.connections.join(", ")
            ));
        }
        None => system.push_str("\n## Banco de produção\n\nNenhum banco disponível: toda afirmação sobre produção fica `unverified`.\n"),
    }
    if let Some(r) = rubric.and_then(|r| std::fs::read_to_string(r).ok()) {
        system.push_str("\n## Tom do comentário (vale só para o `body`)\n\n");
        system.push_str(&r);
    }
    a.push("--append-system-prompt".into());
    a.push(system);
    a
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Finding {
    pub file: String,
    pub line: Option<u32>,
    pub severity: String,
    pub category: String,
    pub label: String,
    pub body: String,
    /// o achado passa na barra para virar comentário
    pub post: bool,
    /// 8. linha fora do diff: vai como nota geral e vem desmarcado
    pub outside: bool,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Check {
    pub dimension: String,
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Claim {
    pub claim: String,
    pub status: String,
    pub source: String,
    pub query: String,
    pub result: String,
}

/// O relatório do revisor. Saída antiga (só `findings`) vira `needs_human` sem checks.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Report {
    pub verdict: String,
    pub confidence: f64,
    pub summary: String,
    pub checks: Vec<Check>,
    pub claims: Vec<Claim>,
    pub findings: Vec<Finding>,
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().into()
}

pub fn parse_report(o: &Value) -> Report {
    let arr = |k: &str| o[k].as_array().cloned().unwrap_or_default();
    Report {
        verdict: o["verdict"].as_str().unwrap_or("needs_human").into(),
        confidence: o["confidence"].as_f64().unwrap_or(0.),
        summary: s(o, "summary"),
        checks: arr("checks")
            .iter()
            .map(|c| Check {
                dimension: s(c, "dimension"),
                status: c["status"].as_str().unwrap_or("unknown").into(),
                note: s(c, "note"),
            })
            .collect(),
        claims: arr("claims")
            .iter()
            .map(|c| Claim {
                claim: s(c, "claim"),
                status: c["status"].as_str().unwrap_or("unverified").into(),
                source: s(c, "source"),
                query: s(c, "query"),
                result: s(c, "result"),
            })
            .collect(),
        findings: arr("findings")
            .iter()
            .map(|f| Finding {
                file: s(f, "file"),
                line: f["line"].as_u64().map(|l| l as u32),
                severity: f["severity"].as_str().unwrap_or("minor").into(),
                category: s(f, "category"),
                label: s(f, "label"),
                body: s(f, "body"),
                post: f["post"].as_bool().unwrap_or(true),
                outside: false,
            })
            .collect(),
    }
}

/// Um evento do `stream-json`: o passo para a tela de progresso, ou o resultado.
#[derive(Debug, PartialEq)]
pub enum Step {
    Tool(String),
    Done(Report, f64),
    Failed(String),
}

pub fn parse_event(line: &str) -> Option<Step> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "assistant" => v["message"]["content"]
            .as_array()?
            .iter()
            .find(|c| c["type"] == "tool_use" && c["name"] != "StructuredOutput")
            .map(|c| {
                let i = &c["input"];
                let arg = i["file_path"]
                    .as_str()
                    .or(i["command"].as_str())
                    .or(i["pattern"].as_str())
                    .unwrap_or_default();
                Step::Tool(format!(
                    "{} {}",
                    c["name"].as_str().unwrap_or_default(),
                    arg
                ))
            }),
        "result" => {
            if v["is_error"] == true || v["subtype"] != "success" {
                let budget = v["subtype"].as_str().is_some_and(|s| s.contains("budget"));
                return Some(Step::Failed(
                    v["result"]
                        .as_str()
                        .unwrap_or(if budget {
                            "teto de gasto atingido"
                        } else {
                            "o Claude falhou"
                        })
                        .into(),
                ));
            }
            Some(Step::Done(
                parse_report(&v["structured_output"]),
                v["total_cost_usd"].as_f64().unwrap_or(0.),
            ))
        }
        _ => None,
    }
}

/// 8. Achado cuja linha não está no diff → nota geral (desmarcada por padrão).
pub fn mark_outside(f: &mut [Finding], diffs: &[(String, String)]) {
    for x in f {
        x.outside = match x.line {
            None => true,
            Some(l) => !diffs.iter().any(|(p, d)| *p == x.file && in_diff(d, l)),
        };
    }
}

/// 9. MR fechado ou mergeado: revisa, mas não publica.
pub fn can_publish(state: &str) -> bool {
    state == "opened"
}

/// 10. Já revisado neste sha (o `~/.mr-review-seen` do script conta também).
pub fn seen(state: &str, path: &str, iid: u64, sha: &str) -> bool {
    let k = format!("{path}!{iid}@{sha}");
    state.lines().any(|l| l.trim() == k)
}

/// O seu estado de revisor no GitLab (`reviewed`, `approved`, `requested_changes`, `unreviewed`…).
pub fn my_review_state(reviewers: &Value, me: &str) -> Option<String> {
    reviewers
        .as_array()?
        .iter()
        .find(|r| r["user"]["username"] == me)
        .and_then(|r| r["state"].as_str())
        .map(String::from)
}

/// Já revisado: você revisou este sha (aqui ou no `mr-review.sh`) ou o GitLab diz que você revisou/aprovou.
pub fn reviewed(state: &str, r: &Row, gitlab: Option<&str>) -> bool {
    seen(state, &r.path, r.iid, &r.sha)
        || matches!(gitlab, Some("reviewed" | "approved" | "requested_changes"))
}

/// O último sha revisado deste MR (para "+N commits" e a revisão incremental).
pub fn last_seen(state: &str, path: &str, iid: u64) -> Option<String> {
    let p = format!("{path}!{iid}@");
    state
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix(&p).map(String::from))
}

/// A `position` de um comentário em linha (Draft Notes aceita a mesma das discussões).
pub fn position(refs: &Value, file: &str, old_path: &str, line: u32, diff: &str) -> Value {
    let mut p = json!({
        "position_type": "text",
        "base_sha": refs["base_sha"],
        "start_sha": refs["start_sha"],
        "head_sha": refs["head_sha"],
        "new_path": file,
        "old_path": old_path,
        "new_line": line,
    });
    if let Some(o) = old_line(diff, line) {
        p["old_line"] = json!(o);
    }
    p
}

/// Linha removida: só `old_line`, sem `new_line`.
pub fn position_old(refs: &Value, file: &str, old_path: &str, old_line: u32) -> Value {
    json!({
        "position_type": "text",
        "base_sha": refs["base_sha"],
        "start_sha": refs["start_sha"],
        "head_sha": refs["head_sha"],
        "new_path": file,
        "old_path": old_path,
        "old_line": old_line,
    })
}

/// O corpo de nota geral: o `arquivo:linha` vai no texto.
pub fn general_body(body: &str, file: &str, line: Option<u32>) -> String {
    if file.is_empty() {
        return body.into();
    }
    format!(
        "{body}\n\n`{file}{}`",
        line.map_or(String::new(), |l| format!(":{l}"))
    )
}

// ---------- acesso ao GitLab ----------

pub fn read(endpoint: &str, paginate: bool) -> Result<Value, String> {
    let mut c = Command::new("glab");
    c.arg("api");
    if paginate {
        c.arg("--paginate");
    }
    let out = c.arg(endpoint).output().map_err(|e| format!("glab: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .next()
            .unwrap_or("glab falhou")
            .into());
    }
    join_pages(&out.stdout)
}

/// `--paginate` cola os arrays das páginas (`[..][..]`): lidos em sequência e achatados.
fn join_pages(raw: &[u8]) -> Result<Value, String> {
    let mut all = Vec::new();
    for v in serde_json::Deserializer::from_slice(raw).into_iter::<Value>() {
        match v.map_err(|e| e.to_string())? {
            Value::Array(a) => all.extend(a),
            other => return Ok(other),
        }
    }
    Ok(Value::Array(all))
}

/// Toda escrita no GitLab passa por aqui (e só aqui): POST/PUT/DELETE com o header certo e o `.id` conferido.
pub fn write(method: &str, endpoint: &str, body: &Value) -> Result<Value, String> {
    use std::io::Write;
    let mut child = Command::new("glab")
        .args(write_args(method, endpoint))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("glab: {e}"))?;
    child
        .stdin
        .take()
        .map(|mut s| s.write_all(body.to_string().as_bytes()))
        .transpose()
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if method == "DELETE" {
        return if out.status.success() {
            Ok(Value::Null)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into())
        };
    }
    if endpoint.ends_with("/bulk_publish") || endpoint.ends_with("/unapprove") {
        // respondem sem corpo
        return if out.status.success() {
            Ok(Value::Null)
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into())
        };
    }
    if endpoint == "graphql" {
        return gql_ok(&text);
    }
    created_id(&text).map(|_| serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// O GraphQL responde 200 com `errors` no corpo, no topo ou dentro da mutation.
pub fn gql_ok(out: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(out)
        .map_err(|_| out.chars().take(200).collect::<String>().replace('\n', " "))?;
    let top = v["errors"]
        .as_array()
        .and_then(|e| e.first())
        .map(|e| e["message"].clone());
    let inner = v["data"]
        .as_object()
        .into_iter()
        .flat_map(|d| d.values())
        .find_map(|m| m["errors"].as_array().and_then(|e| e.first()).cloned());
    match top.or(inner) {
        Some(e) => Err(e.as_str().map_or_else(|| e.to_string(), String::from)),
        None => Ok(v),
    }
}

pub fn me() -> Option<String> {
    read("user", false).ok()?["username"]
        .as_str()
        .map(String::from)
}

pub fn repos_root() -> PathBuf {
    std::env::var_os("MAESTRO_REPOS").map_or_else(
        || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("repos-ctbz"),
        PathBuf::from,
    )
}

pub fn seen_file() -> PathBuf {
    std::env::var_os("MAESTRO_SEEN").map_or_else(
        || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".mr-review-seen"),
        PathBuf::from,
    )
}

/// As três abas da inbox: para revisar, meus e o grupo.
pub fn inbox(me: &str, group: &str) -> [Result<Vec<Row>, String>; 3] {
    let q = "state=opened&per_page=100";
    [
        read(
            &format!("merge_requests?scope=all&{q}&reviewer_username={me}"),
            false,
        )
        .map(|v| rows(&v)),
        read(
            &format!("merge_requests?scope=all&{q}&author_username={me}"),
            false,
        )
        .map(|v| rows(&v)),
        read(&format!("groups/{}/merge_requests?{q}", enc(group)), true).map(|v| {
            rows(&v)
                .into_iter()
                .filter(|r| !r.who.ends_with(&format!("@{me}")))
                .collect()
        }),
    ]
}

pub fn group() -> String {
    std::env::var("MAESTRO_MR_GROUP").unwrap_or_else(|_| "ctbz/experts".into())
}

pub fn ci_of(detail: &Value) -> Status {
    match detail["head_pipeline"]["status"].as_str().unwrap_or("") {
        "success" | "manual" | "skipped" => Status::Ok,
        "failed" | "canceled" => Status::Fail,
        "" => Status::None,
        _ => Status::Running,
    }
}

/// O MR aberto na tela: detalhe, diff por arquivo e as discussões.
#[derive(Debug, Clone)]
pub struct Opened {
    pub detail: Value,
    /// (new_path, old_path, diff da API)
    pub raw: Vec<(String, String, String)>,
    pub files: Vec<crate::diff::FileDiff>,
    pub discussions: Value,
    /// `/approvals`: `user_has_approved`, `user_can_approve`, `approved_by`
    pub approvals: Value,
    /// arquivos que o GitLab marca como `generated_file`
    pub generated: Vec<String>,
}

pub fn open(path: &str, iid: u64) -> Result<Opened, String> {
    let base = format!("projects/{}/merge_requests/{iid}", enc(path));
    let detail = read(&base, false)?;
    let diffs = read(&format!("{base}/diffs?per_page=100"), true)?;
    let discussions = read(&format!("{base}/discussions?per_page=100"), true)?;
    let approvals = read(&format!("{base}/approvals"), false).unwrap_or_default();
    let mut generated = Vec::new();
    let mut raw = Vec::new();
    let mut patch = String::new();
    for d in diffs.as_array().into_iter().flatten() {
        let (np, op) = (
            d["new_path"].as_str().unwrap_or_default(),
            d["old_path"].as_str().unwrap_or_default(),
        );
        let body = d["diff"].as_str().unwrap_or_default();
        if d["generated_file"] == true {
            generated.push(np.to_string());
        }
        raw.push((np.to_string(), op.to_string(), body.to_string()));
        let new = if d["new_file"] == true {
            "new file mode 100644\n"
        } else {
            ""
        };
        patch.push_str(&format!(
            "diff --git a/{op} b/{np}\n{new}--- a/{op}\n+++ b/{np}\n{body}"
        ));
        if !body.ends_with('\n') {
            patch.push('\n');
        }
    }
    Ok(Opened {
        files: crate::diff::parse(&patch),
        detail,
        raw,
        discussions,
        approvals,
        generated,
    })
}

pub fn approve(path: &str, iid: u64, sha: &str) -> Result<(), String> {
    let base = format!("projects/{}/merge_requests/{iid}", enc(path));
    write("POST", &format!("{base}/approve"), &json!({ "sha": sha })).map(drop)
}

pub fn unapprove(path: &str, iid: u64) -> Result<(), String> {
    let base = format!("projects/{}/merge_requests/{iid}", enc(path));
    write("POST", &format!("{base}/unapprove"), &json!({})).map(drop)
}

/// As opções do diálogo de merge; os valores iniciais vêm do MR.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeOpts {
    pub squash: bool,
    pub remove_branch: bool,
    pub when_pipeline_succeeds: bool,
}

impl MergeOpts {
    pub fn from_detail(d: &Value) -> Self {
        Self {
            squash: d["squash"] == true || d["squash_on_merge"] == true,
            remove_branch: d["force_remove_source_branch"] == true,
            when_pipeline_succeeds: false,
        }
    }
}

/// `sha` trava o merge no commit que você revisou: se entrou outro, o GitLab recusa.
pub fn merge(path: &str, iid: u64, sha: &str, o: &MergeOpts) -> Result<(), String> {
    let base = format!("projects/{}/merge_requests/{iid}", enc(path));
    let body = json!({
        "sha": sha,
        "squash": o.squash,
        "should_remove_source_branch": o.remove_branch,
        "merge_when_pipeline_succeeds": o.when_pipeline_succeeds,
    });
    write("PUT", &format!("{base}/merge"), &body).map(drop)
}

pub fn cancel_auto_merge(path: &str, iid: u64) -> Result<(), String> {
    let base = format!("projects/{}/merge_requests/{iid}", enc(path));
    write(
        "POST",
        &format!("{base}/cancel_merge_when_pipeline_succeeds"),
        &json!({}),
    )
    .map(drop)
}

/// Por que o Merge está apagado (`detailed_merge_status` do GitLab); `None` quando dá para mergear.
pub fn merge_blocker(detail: &Value) -> Option<&'static str> {
    if detail["state"] != "opened" {
        return Some("o MR não está aberto");
    }
    Some(
        match detail["detailed_merge_status"].as_str().unwrap_or_default() {
            "mergeable" | "" => return None,
            "not_approved" => "precisa de aprovação",
            "draft_status" => "o MR é rascunho",
            "discussions_not_resolved" => "há discussões não resolvidas",
            // pipeline rodando: dá para agendar ("merge quando o pipeline passar")
            "ci_still_running" => return None,
            "ci_must_pass" => "o pipeline precisa passar",
            "conflict" | "need_rebase" => "conflito com a branch de destino",
            "blocked_status" => "bloqueado por outro MR",
            "checking" | "unchecked" | "preparing" | "approvals_syncing" => {
                "o GitLab ainda está checando"
            }
            _ => "o GitLab não permite o merge agora",
        },
    )
}

/// A worktree persistente do MR (`~/.cache/maestro/wt/<path>/<iid>`), no sha da cabeça.
pub fn worktree(path: &str, iid: u64, target: &str) -> Result<PathBuf, String> {
    let git = |dir: &Path, args: &[&str]| -> Result<String, String> {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if o.status.success() {
            Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
        }
    };
    let repos = repos_root();
    let probe = repos.join(path.rsplit('/').next().unwrap_or(path));
    let origin = git(&probe, &["remote", "get-url", "origin"]).ok();
    let dir = repo_dir(&repos, path, origin.as_deref());
    if !dir.join(".git").exists() {
        let url = format!("git@gitlab.com:{path}.git");
        let o = Command::new("git")
            .args(["clone", "-q", "--filter=blob:none", &url])
            .arg(&dir)
            .output()
            .map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).trim().into());
        }
    }
    let specs = refspecs(iid, target);
    let mut fetch = vec!["fetch", "-q", "origin"];
    fetch.extend(specs.iter().map(String::as_str));
    git(&dir, &fetch)?;
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let wt = home
        .join(".cache/maestro/wt")
        .join(path)
        .join(iid.to_string());
    let mr_ref = format!("refs/mr/{iid}");
    if !(wt.join(".git").exists() && git(&wt, &["checkout", "-q", "--detach", &mr_ref]).is_ok()) {
        // .git apontando para um clone que sumiu: refaz a worktree
        let _ = std::fs::remove_dir_all(&wt);
        let _ = git(&dir, &["worktree", "prune"]);
        std::fs::create_dir_all(wt.parent().unwrap()).map_err(|e| e.to_string())?;
        git(
            &dir,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &wt.to_string_lossy(),
                &mr_ref,
            ],
        )?;
    }
    Ok(wt)
}

/// Um rascunho de comentário num MR (o `S.drafts` com `ctx='mr'` do protótipo).
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub id: usize,
    pub file: String,
    pub line: Option<u32>,
    /// comentário numa linha removida (só existe do lado velho)
    pub old: Option<u32>,
    pub body: String,
    pub by: String,
    pub sev: Option<String>,
    pub label: Option<String>,
    /// o head do MR quando o rascunho nasceu: se mudou, a linha pode ter andado
    pub head: String,
    /// já criado como draft note no GitLab (não repostar numa nova tentativa)
    pub gitlab_id: Option<u64>,
    pub error: Option<String>,
    pub general: bool,
    /// resposta a esta discussão: sai na review como resposta na thread
    pub reply_to: Option<String>,
}

/// Publica como uma review só: cada rascunho vira draft note e um `bulk_publish` solta tudo junto.
/// Se algum não entrar, nada é publicado; os que entraram guardam o id e não são reenviados na próxima tentativa.
pub fn publish(path: &str, iid: u64, opened: &Opened, drafts: &mut [Draft]) -> Result<(), String> {
    let base = format!("projects/{}/merge_requests/{iid}", enc(path));
    let head_now = read(&base, false)?["diff_refs"]["head_sha"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let refs = &opened.detail["diff_refs"];
    let mut failed = 0;
    for d in drafts.iter_mut().filter(|d| d.gitlab_id.is_none()) {
        let body = match (&d.label, &d.sev) {
            (Some(l), Some(s)) if !l.is_empty() => format!("**{l}** ({s}): {}", d.body),
            _ => d.body.clone(),
        };
        let inline = !d.general && (d.line.is_some() || d.old.is_some()) && d.reply_to.is_none();
        if inline && d.head != head_now {
            d.error = Some("O MR recebeu um commit novo depois deste rascunho; a linha pode ter mudado de lugar.".into());
            failed += 1;
            continue;
        }
        let note = if let Some(id) = &d.reply_to {
            json!({"note": body, "in_reply_to_discussion_id": id})
        } else if inline {
            let raw = opened.raw.iter().find(|r| r.0 == d.file);
            let (old, diff) = raw.map_or((d.file.as_str(), ""), |r| (r.1.as_str(), r.2.as_str()));
            let pos = match (d.line, d.old) {
                (Some(n), _) => position(refs, &d.file, old, n, diff),
                (None, o) => position_old(refs, &d.file, old, o.unwrap_or_default()),
            };
            json!({"note": body, "position": pos})
        } else {
            json!({"note": general_body(&body, &d.file, d.line)})
        };
        match write("POST", &format!("{base}/draft_notes"), &note) {
            Ok(v) => {
                d.gitlab_id = v["id"].as_u64();
                d.error = None;
            }
            Err(e) => {
                d.error = Some(format!("GitLab: {e}"));
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(format!(
            "{failed} comentário{} não foi aceito pelo GitLab",
            if failed > 1 { "s" } else { "" }
        ));
    }
    write(
        "POST",
        &format!("{base}/draft_notes/bulk_publish"),
        &json!({}),
    )?;
    Ok(())
}

/// O que o revisor manda para a tela enquanto roda.
#[derive(Debug)]
pub enum Run {
    Ok(String),
    Tool(String),
    Pid(u32),
    Done(Report, f64),
    Failed(String),
}

/// O `review()` do script: worktree, prompt com dedup, `claude -p` em stream-json. Roda numa thread.
#[allow(clippy::too_many_arguments)]
pub fn review(
    path: &str,
    iid: u64,
    title: &str,
    target: &str,
    desc: &str,
    notes: &Value,
    since: Option<String>,
    send: impl Fn(Run),
) {
    use std::io::{BufRead, Write};
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // um revisor por vez: a gaveta, o vigia e a fila automática dividem worktree e teto de gasto
    let _one = ONE.try_lock().or_else(|_| {
        send(Run::Ok("esperando a outra revisão terminar".into()));
        ONE.lock()
    });
    let wt = match worktree(path, iid, target) {
        Ok(w) => w,
        Err(e) => return send(Run::Failed(format!("worktree: {e}"))),
    };
    send(Run::Ok(format!("worktree refs/mr/{iid} pronta")));
    let n = notes
        .as_array()
        .map_or(0, |a| a.iter().filter(|x| x["system"] != true).count());
    send(Run::Ok(format!(
        "{n} comentário{} existente{} carregado{} para dedup",
        if n == 1 { "" } else { "s" },
        if n == 1 { "" } else { "s" },
        if n == 1 { "" } else { "s" }
    )));
    let rubric = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".claude/mr-review-rubric.md");
    let db = crate::ai_review::db(&crate::ai_review::Config::load());
    if let Some(d) = &db {
        send(Run::Ok(format!(
            "banco só leitura: {}",
            d.connections.join(", ")
        )));
    }
    let child = Command::new("claude")
        .args(claude_args(Some(&rubric), db.as_ref()))
        .current_dir(&wt)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return send(Run::Failed(format!("claude: {e}"))),
    };
    send(Run::Pid(child.id()));
    send(Run::Ok("claude -p · stream-json · json-schema".into()));
    let p = prompt(
        iid,
        title,
        target,
        desc,
        &notes_text(notes),
        since.as_deref(),
    );
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(p.as_bytes());
    }
    let mut done = false;
    if let Some(out) = child.stdout.take() {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            match parse_event(&line) {
                Some(Step::Tool(t)) => send(Run::Tool(t)),
                Some(Step::Done(f, c)) => {
                    done = true;
                    send(Run::Done(f, c));
                }
                Some(Step::Failed(e)) => {
                    done = true;
                    send(Run::Failed(e));
                }
                None => {}
            }
        }
    }
    let _ = child.wait();
    if !done {
        send(Run::Failed("o Claude parou sem devolver os achados".into()));
    }
}

/// Grava `path!iid@sha` no mesmo arquivo do `~/.mr-review-seen`.
/// Arquivos marcados "Visto": `path!iid|arquivo|hash do diff`. Diff com outro hash = mudou depois.
pub fn viewed_file() -> PathBuf {
    std::env::var_os("MAESTRO_VIEWED").map_or_else(
        || {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share/maestro/mr-viewed")
        },
        PathBuf::from,
    )
}

pub fn viewed(state: &str, key: &str) -> HashMap<String, u64> {
    state
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '|');
            (it.next()? == key).then_some(())?;
            Some((it.next()?.to_string(), it.next()?.parse().ok()?))
        })
        .collect()
}

/// Troca a linha do arquivo; `None` desmarca.
pub fn set_viewed(state: &str, key: &str, file: &str, hash: Option<u64>) -> String {
    let prefix = format!("{key}|{file}|");
    let mut out: String = state
        .lines()
        .filter(|l| !l.starts_with(&prefix))
        .map(|l| format!("{l}\n"))
        .collect();
    if let Some(h) = hash {
        out.push_str(&format!("{prefix}{h}\n"));
    }
    out
}

/// FNV-1a: estável entre builds (o `DefaultHasher` não promete isso).
pub fn diff_hash(diff: &str) -> u64 {
    diff.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

pub fn mark_seen(path: &str, iid: u64, sha: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(seen_file())
    {
        let _ = writeln!(f, "{path}!{iid}@{sha}");
    }
}

/// Rascunhos seus no GitLab neste MR que não vieram do Maestro: o `bulk_publish` levaria junto.
pub fn foreign_drafts(path: &str, iid: u64, ours: &[u64]) -> usize {
    read(
        &format!("projects/{}/merge_requests/{iid}/draft_notes", enc(path)),
        false,
    )
    .ok()
    .and_then(|v| v.as_array().cloned())
    .unwrap_or_default()
    .iter()
    .filter(|d| d["id"].as_u64().is_some_and(|i| !ours.contains(&i)))
    .count()
}

/// Desistir: apaga do GitLab os draft notes que o Maestro já criou.
pub fn discard_remote(path: &str, iid: u64, ids: &[u64]) {
    for id in ids {
        let _ = write(
            "DELETE",
            &format!(
                "projects/{}/merge_requests/{iid}/draft_notes/{id}",
                enc(path)
            ),
            &Value::Null,
        );
    }
}

#[cfg(test)]
mod viewed_tests {
    use super::*;

    #[test]
    fn visto_cai_quando_o_diff_muda() {
        let k = "g/p!7";
        let st = set_viewed("", k, "a.rs", Some(diff_hash("+x")));
        let st = set_viewed(&st, "g/p!8", "a.rs", Some(1));
        let v = viewed(&st, k);
        assert_eq!(v.get("a.rs"), Some(&diff_hash("+x")));
        assert_ne!(diff_hash("+x"), diff_hash("+y"));
        let st = set_viewed(&st, k, "a.rs", None);
        assert!(viewed(&st, k).is_empty() && viewed(&st, "g/p!8").len() == 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "@@ -10,4 +10,5 @@ fn a()\n ctx10\n-old11\n+new11\n+new12\n ctx13\n\\ No newline at end of file\n";

    #[test]
    fn armadilhas_do_script() {
        // 1: linha de contexto ganha old_line; adicionada não
        assert_eq!(old_line(DIFF, 10), Some(10));
        assert_eq!(old_line(DIFF, 13), Some(12));
        assert_eq!(old_line(DIFF, 11), None);
        assert!(in_diff(DIFF, 12) && in_diff(DIFF, 13) && !in_diff(DIFF, 40));
        // 2
        let a = write_args("POST", "projects/x/draft_notes");
        assert!(
            a.windows(2)
                .any(|w| w == ["-H", "Content-Type: application/json"])
        );
        // 3: glab sai 0 num 400; sem .id não é sucesso
        assert!(created_id(r#"{"message":"400 line_code can't be blank"}"#).is_err());
        assert_eq!(created_id(r#"{"id": 9}"#).as_deref(), Ok("9"));
        // 4
        assert_eq!(
            refspecs(7, "main")[0],
            "+refs/merge-requests/7/head:refs/mr/7"
        );
        // 5
        let r = Path::new("/r");
        assert_eq!(
            repo_dir(
                r,
                "ctbz/a/zendesk-agents",
                Some("git@gitlab.com:ctbz/a/zendesk-agents.git")
            ),
            r.join("zendesk-agents")
        );
        assert_eq!(
            repo_dir(
                r,
                "ctbz/b/zendesk-agents",
                Some("git@gitlab.com:ctbz/a/zendesk-agents.git")
            ),
            r.join("ctbz_b_zendesk-agents")
        );
        // 6
        assert!(
            pipeline_ok("success")
                && pipeline_ok("none")
                && !pipeline_ok("failed")
                && !pipeline_ok("running")
        );
        // 7: bots e Duo entram no dedup
        let notes = json!([{"system": true, "body": "x"}, {"system": false, "author": {"username": "GitLabDuo"}, "body": "resumo"},
            {"system": false, "author": {"username": "ana"}, "body": "linha", "position": {"new_path": "a.ts", "new_line": 3}}]);
        let t = notes_text(&notes);
        assert!(
            t.contains("@GitLabDuo\nresumo")
                && t.contains("@ana em a.ts:3")
                && !t.contains("\nx\n")
        );
        assert!(prompt(1, "t", "main", "d", &t, None).contains("@GitLabDuo"));
        // 8
        let mut f = vec![
            Finding {
                file: "a.ts".into(),
                line: Some(12),
                severity: "major".into(),
                label: "issue".into(),
                body: "b".into(),
                outside: false,
                ..Default::default()
            },
            Finding {
                file: "a.ts".into(),
                line: Some(40),
                severity: "nit".into(),
                label: "nitpick".into(),
                body: "b".into(),
                outside: false,
                ..Default::default()
            },
        ];
        mark_outside(&mut f, &[("a.ts".into(), DIFF.into())]);
        assert!(!f[0].outside && f[1].outside);
        assert_eq!(general_body("b", "a.ts", Some(40)), "b\n\n`a.ts:40`");
        // 9
        assert!(can_publish("opened") && !can_publish("merged") && !can_publish("closed"));
        // 10
        let st = "g/p!3@aaa\ng/p!3@bbb\n";
        assert!(seen(st, "g/p", 3, "bbb") && !seen(st, "g/p", 3, "ccc"));
        assert_eq!(last_seen(st, "g/p", 3).as_deref(), Some("bbb"));
        assert_eq!(
            join_pages(br#"[{"d":"a[0][1]"}][{"d":"b"}]"#).unwrap(),
            json!([{"d": "a[0][1]"}, {"d": "b"}])
        );
        // 11
        let args = claude_args(None, None);
        assert!(
            args.windows(2)
                .any(|w| w == ["--tools", "Read,Grep,Glob,Bash,Skill,Agent"])
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("mcp__") || a == "--mcp-config")
        );
        assert!(args.last().unwrap().contains("Nenhum banco disponível"));
        let db = crate::ai_review::Db {
            mcp: "{}".into(),
            settings: "{\"hooks\":{}}".into(),
            connections: vec!["PRD - x".into()],
        };
        let with_db = claude_args(None, Some(&db));
        assert!(with_db.contains(&"mcp__dbx__dbx_execute_query".to_string()));
        assert!(!with_db.contains(&"mcp__dbx__dbx_execute_batch".to_string()));
        assert!(
            with_db
                .windows(2)
                .any(|w| w[0] == "--settings" && w[1].contains("hooks"))
        );
        assert!(with_db.last().unwrap().contains("PRD - x"));
        assert!(
            args.contains(&"Bash(git blame:*)".to_string())
                && !args.iter().any(|a| a == "Edit" || a == "Write")
                && args.windows(2).any(|w| w == ["--setting-sources", "user"])
                && args.contains(&"--strict-mcp-config".to_string())
        );
        let pos = position(
            &json!({"base_sha": "b", "start_sha": "s", "head_sha": "h"}),
            "a.ts",
            "a.ts",
            13,
            DIFF,
        );
        assert_eq!(
            (pos["new_line"].as_u64(), pos["old_line"].as_u64()),
            (Some(13), Some(12))
        );
    }

    /// O `glab` falso grava cada escrita (argumentos e corpo); um corpo com "FALHA" volta 400 sem `.id`.
    #[test]
    fn publica_como_uma_review() {
        let dir = std::env::temp_dir().join(format!("maestro-glab-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("log");
        let script = format!(
            "#!/bin/sh\nbody=$(cat)\necho \"$* | $body\" >> '{}'\ncase \"$*\" in\n  *-X*) case \"$body\" in *FALHA*) echo '{{\"message\":\"400\"}}';; *) echo '{{\"id\": 5}}';; esac;;\n  *) echo '{{\"diff_refs\":{{\"head_sha\":\"h\"}}}}';;\nesac\n",
            log.display()
        );
        std::fs::write(dir.join("glab"), script).unwrap();
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(dir.join("glab"))
            .status()
            .unwrap();
        let path = format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        // SAFETY: nenhum outro teste chama o glab
        unsafe { std::env::set_var("PATH", path) };
        let opened = Opened {
            detail: json!({"diff_refs": {"base_sha": "b", "start_sha": "s", "head_sha": "h"}}),
            raw: vec![("a.ts".into(), "a.ts".into(), DIFF.into())],
            files: Vec::new(),
            discussions: Value::Null,
            approvals: Value::Null,
            generated: Vec::new(),
        };
        let d = |id, line, body: &str| Draft {
            id,
            file: "a.ts".into(),
            line,
            old: None,
            body: body.into(),
            by: "você".into(),
            sev: None,
            label: None,
            head: "h".into(),
            gitlab_id: None,
            error: None,
            general: false,
            reply_to: None,
        };
        let mut drafts = vec![d(1, Some(13), "contexto"), d(2, Some(40), "FALHA")];
        assert!(publish("g/p", 3, &opened, &mut drafts).is_err());
        assert_eq!(drafts[0].gitlab_id, Some(5));
        assert!(drafts[1].error.is_some());
        let first = std::fs::read_to_string(&log).unwrap();
        assert!(first.contains(
            "-H Content-Type: application/json projects/g%2Fp/merge_requests/3/draft_notes"
        ));
        assert!(first.contains("\"old_line\":12") && !first.contains("bulk_publish"));
        // nova tentativa: o que já entrou não é reenviado, e aí sai a review inteira
        drafts[1].body = "ok agora".into();
        std::fs::remove_file(&log).unwrap();
        assert!(publish("g/p", 3, &opened, &mut drafts).is_ok());
        let second = std::fs::read_to_string(&log).unwrap();
        assert_eq!(second.matches("/draft_notes --input").count(), 1);
        assert!(second.contains("draft_notes/bulk_publish"));
        // commit novo no MR depois do rascunho: não posta na linha errada
        let mut stale = vec![Draft {
            head: "velho".into(),
            ..d(3, Some(13), "x")
        }];
        assert!(publish("g/p", 3, &opened, &mut stale).is_err());
        assert!(stale[0].error.as_deref().unwrap().contains("commit novo"));
        // resposta numa thread: vai como draft note da discussão, mesmo com commit novo
        std::fs::remove_file(&log).unwrap();
        let mut reply = vec![Draft {
            head: "velho".into(),
            reply_to: Some("abc123".into()),
            ..d(4, Some(13), "valeu, ajustei")
        }];
        assert!(publish("g/p", 3, &opened, &mut reply).is_ok());
        let third = std::fs::read_to_string(&log).unwrap();
        assert!(third.contains("\"in_reply_to_discussion_id\":\"abc123\""));
        assert!(!third.contains("position"));
        // comentário numa linha removida: só old_line na position
        std::fs::remove_file(&log).unwrap();
        let mut gone = vec![Draft {
            line: None,
            old: Some(7),
            ..d(5, None, "por que saiu?")
        }];
        assert!(publish("g/p", 3, &opened, &mut gone).is_ok());
        let removed = std::fs::read_to_string(&log).unwrap();
        assert!(removed.contains("\"old_line\":7") && !removed.contains("new_line"));
        // aprovar e mergear, travados no sha revisado
        std::fs::remove_file(&log).unwrap();
        approve("g/p", 3, "h").unwrap();
        unapprove("g/p", 3).unwrap();
        let o =
            MergeOpts::from_detail(&json!({"squash": true, "force_remove_source_branch": true}));
        merge("g/p", 3, "h", &o).unwrap();
        let fourth = std::fs::read_to_string(&log).unwrap();
        assert!(
            fourth.contains("-X POST")
                && fourth.contains("merge_requests/3/approve")
                && fourth.contains("\"sha\":\"h\"")
        );
        assert!(fourth.contains("merge_requests/3/unapprove"));
        assert!(fourth.contains("-X PUT") && fourth.contains("merge_requests/3/merge"));
        assert!(
            fourth.contains("\"squash\":true")
                && fourth.contains("\"should_remove_source_branch\":true")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn motivo_do_merge_apagado() {
        let st = |s: &str| json!({"state": "opened", "detailed_merge_status": s});
        assert_eq!(merge_blocker(&st("mergeable")), None);
        assert_eq!(
            merge_blocker(&st("not_approved")),
            Some("precisa de aprovação")
        );
        assert_eq!(merge_blocker(&st("ci_still_running")), None);
        assert_eq!(
            merge_blocker(&st("ci_must_pass")),
            Some("o pipeline precisa passar")
        );
        assert_eq!(
            merge_blocker(&json!({"state": "merged"})),
            Some("o MR não está aberto")
        );
    }

    #[test]
    fn progresso_e_resultado_do_stream() {
        let steps: Vec<Step> = include_str!("fixtures/review-stream.jsonl")
            .lines()
            .filter_map(parse_event)
            .collect();
        assert_eq!(steps[0], Step::Tool("Read src/a.ts".into()));
        let Some(Step::Done(r, cost)) = steps.last() else {
            panic!("sem resultado")
        };
        let f = &r.findings;
        assert_eq!((r.verdict.as_str(), f[0].post), ("needs_human", true));
        assert_eq!(
            (f.len(), f[0].file.as_str(), f[0].line),
            (1, "a.ts", Some(1))
        );
        assert!(*cost > 0.);
    }

    #[test]
    fn relatorio_completo() {
        let r = parse_report(&json!({"verdict":"approve","confidence":0.9,"summary":"ok",
            "checks":[{"dimension":"sec","status":"pass","note":""}],
            "claims":[{"claim":"c","status":"confirmed","source":"PRD - x","query":"select 1","result":"1"}],
            "findings":[{"file":"a","line":null,"severity":"nit","category":"read","label":"nitpick","body":"b","post":false}]}));
        assert_eq!(
            (
                r.verdict.as_str(),
                r.confidence,
                r.checks[0].status.as_str()
            ),
            ("approve", 0.9, "pass")
        );
        assert_eq!(
            (
                r.claims[0].query.as_str(),
                r.findings[0].post,
                r.findings[0].line
            ),
            ("select 1", false, None)
        );
        assert!(SCHEMA.contains("needs_human") && serde_json::from_str::<Value>(SCHEMA).is_ok());
    }

    #[test]
    fn reviewed_by_sha_or_gitlab_state() {
        let r = rows(&json!([{"references":{"full":"g/p!7"},"iid":7,"sha":"abc"}]))[0].clone();
        let revs = json!([{"user":{"username":"eu"},"state":"approved"},{"user":{"username":"x"},"state":"unreviewed"}]);
        assert_eq!(my_review_state(&revs, "eu").as_deref(), Some("approved"));
        assert!(reviewed("", &r, Some("approved")));
        assert!(!reviewed("", &r, Some("unapproved")));
        assert!(reviewed("g/p!7@abc\n", &r, None));
        assert!(!reviewed("g/p!7@old\n", &r, Some("unreviewed")));
    }
}
