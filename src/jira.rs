use std::process::Command;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Debug, PartialEq, Deserialize)]
pub struct Issue {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub criteria: String,
}

/// `PROJ-123`
pub fn is_key(k: &str) -> bool {
    let Some((proj, n)) = k.split_once('-') else {
        return false;
    };
    proj.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && proj
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

/// A chave de `PROJ-123`, `proj-123` ou de um link (`…/browse/PROJ-123`, `…?selectedIssue=PROJ-123`).
pub fn key_from(input: &str) -> Option<String> {
    let s = input.trim();
    let tail = ["/browse/", "selectedIssue="]
        .iter()
        .find_map(|m| s.split_once(m).map(|(_, t)| t))
        .unwrap_or(s);
    let key = tail
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .next()?
        .to_uppercase();
    is_key(&key).then_some(key)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Card {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub squad: String,
    pub title: String,
    #[serde(default)]
    pub desc: String,
    /// `https://<site>.atlassian.net`, do getAccessibleAtlassianResources
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub subs: Vec<Sub>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Sub {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub sp: Option<f64>,
    pub status: String,
    /// os status de destino que a sub-task alcança agora
    #[serde(default)]
    pub transitions: Vec<String>,
}

#[derive(Deserialize)]
struct Created {
    key: String,
}

const CLOUD: &str = "Chame getAccessibleAtlassianResources para obter o cloudId (e a url do site)";
const ERR: &str =
    "se alguma ferramenta der erro, responda SOMENTE {\"error\":\"<mensagem da ferramenta>\"}";

/// Um `claude -p` curto com o MCP do Atlassian do próprio Claude Code; devolve o JSON da resposta.
/// ponytail: haiku custa ~US$ 0,01 e ~7 s por chamada; trocar pela API do Jira se ficar frequente
fn run<T: DeserializeOwned>(prompt: String, tools: &[&str]) -> Result<T, String> {
    let tools: Vec<String> = ["getAccessibleAtlassianResources"]
        .iter()
        .chain(tools)
        .map(|t| format!("mcp__claude_ai_Atlassian_MCP__{t}"))
        .collect();
    let out = Command::new("claude")
        .arg("-p")
        .arg(prompt)
        .args(["--model", "haiku", "--output-format", "json"])
        .arg(format!("--allowedTools={}", tools.join(",")))
        .current_dir(std::env::temp_dir())
        .output()
        .map_err(|e| format!("claude: {e}"))?;
    let reply: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| "resposta inválida do claude".to_string())?;
    parse(reply["result"].as_str().unwrap_or_default())
}

pub fn fetch(key: &str) -> Result<Issue, String> {
    run(
        format!(
            "{CLOUD} e depois getJiraIssue com issueIdOrKey {key}. \
             Responda SOMENTE um JSON {{\"type\":\"\",\"title\":\"\",\"criteria\":\"\"}} com o tipo, o título e os \
             critérios de aceite em uma frase; {ERR}."
        ),
        &["getJiraIssue"],
    )
}

/// O card com as sub-tasks, os SP (`customfield_10004`), a squad (`customfield_18419`) e as transições de cada uma.
pub fn card(key: &str) -> Result<Card, String> {
    run(
        format!(
            "{CLOUD}. Chame getJiraIssue com issueIdOrKey {key} (campos summary, description, issuetype, status, \
             subtasks e customfield_18419, que é a squad). Para cada sub-task, getJiraIssue dela (summary, issuetype, \
             status e customfield_10004, que são os story points) e executeRead com name listJiraIssueTransitions e \
             inputs {{\"issueIdOrKey\":\"<key da sub-task>\"}}. Responda SOMENTE um JSON \
             {{\"key\":\"{key}\",\"type\":\"\",\"status\":\"\",\"squad\":\"\",\"title\":\"\",\"desc\":\"\",\
             \"site\":\"https://<site>.atlassian.net\",\"subs\":[{{\"key\":\"\",\"type\":\"\",\"title\":\"\",\"sp\":2,\
             \"status\":\"\",\"transitions\":[\"<nome do status de destino de cada transição>\"]}}]}} com a descrição \
             em texto puro e sp null quando não houver; {ERR}."
        ),
        &["getJiraIssue", "executeRead"],
    )
}

pub fn save(key: &str, title: &str, desc: &str) -> Result<(), String> {
    let fields = serde_json::json!({ "summary": title, "description": desc });
    run::<Value>(
        format!(
            "{CLOUD} e depois editJiraIssue com issueIdOrKey {key} e fields {fields} (descrição em texto puro). \
             Responda SOMENTE {{\"ok\":true}}; {ERR}."
        ),
        &["editJiraIssue"],
    )
    .map(|_| ())
}

/// Leva a issue à transição cujo status de destino é `to`.
pub fn transition(key: &str, to: &str) -> Result<(), String> {
    let to = serde_json::to_string(to).unwrap_or_default();
    run::<Value>(
        format!(
            "{CLOUD}. Chame executeRead com name listJiraIssueTransitions e inputs {{\"issueIdOrKey\":\"{key}\"}}, \
             escolha a transição cujo status de destino (to.name) é {to} e chame transitionJiraIssue com issueIdOrKey \
             {key} e o id dela. Responda SOMENTE {{\"ok\":true}}; se nenhuma transição levar a esse status, \
             {{\"error\":\"sem transição para {}\"}}; {ERR}.",
            to.trim_matches('"')
        ),
        &["executeRead", "transitionJiraIssue"],
    )
    .map(|_| ())
}

/// Cria o card no projeto, atribuído a você; devolve a key.
pub fn create(project: &str, kind: &str, title: &str) -> Result<String, String> {
    let title = serde_json::to_string(title).unwrap_or_default();
    run::<Created>(
        format!(
            "{CLOUD} e atlassianUserInfo para o seu accountId. Chame createJiraIssue com projectKey {project}, \
             issueTypeName {kind}, summary {title} e assignee_account_id com o seu accountId. \
             Responda SOMENTE {{\"key\":\"<key criada>\"}}; {ERR}."
        ),
        &["atlassianUserInfo", "createJiraIssue"],
    )
    .map(|c| c.key)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub statuses: Vec<String>,
    /// todos os status são de conclusão: o board não carrega os cards dela
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Linked {
    pub key: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrcCard {
    pub key: String,
    pub title: String,
    pub status: String,
    pub labels: Vec<String>,
    pub acr: Option<Linked>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub site: String,
    pub cols: Vec<Column>,
    pub cards: Vec<PrcCard>,
}

/// As colunas e os cards abertos do board. Os cards saem do `tool_result` do MCP, não da resposta do modelo:
/// com dezenas de cards, redigitar arrisca inventar ou perder card.
/// ponytail: até 100 cards (uma página do JQL); paginar se o board passar disso
pub fn board(id: u64, acr_link: Option<&str>) -> Result<Board, String> {
    let tools = [
        "getAccessibleAtlassianResources",
        "executeRead",
        "searchJiraIssuesUsingJql",
    ]
    .map(|t| format!("mcp__claude_ai_Atlassian_MCP__{t}"))
    .join(",");
    let prompt = format!(
        "{CLOUD}. Chame executeRead com name getJiraBoardConfig e inputs {{\"boardId\":{id}}}. Com o id do filtro \
         dessa config, chame searchJiraIssuesUsingJql com jql 'filter = <id> AND statusCategory != Done ORDER BY \
         updated DESC', maxResults 100 e fields [\"summary\",\"status\",\"labels\",\"issuelinks\"]. Não resuma; \
         responda só OK."
    );
    let out = Command::new("claude")
        .arg("-p")
        .arg(prompt)
        .args([
            "--model",
            "haiku",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .arg(format!("--allowedTools={tools}"))
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("claude: {e}"))?;
    parse_board(&String::from_utf8_lossy(&out.stdout), acr_link)
}

/// `acr_link`: o tipo de vínculo PRC↔ACR (da config local); sem ele, vale qualquer vínculo com outro projeto.
pub fn parse_board(stream: &str, acr_link: Option<&str>) -> Result<Board, String> {
    let mut names = std::collections::HashMap::new();
    let (mut site, mut cfg, mut issues) = (None, None, None);
    for line in stream.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        for c in v["message"]["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("tool_use") => {
                    names.insert(
                        c["id"].as_str().unwrap_or_default().to_string(),
                        c["name"].as_str().unwrap_or_default().to_string(),
                    );
                }
                Some("tool_result") => {
                    let name = names
                        .get(c["tool_use_id"].as_str().unwrap_or_default())
                        .cloned()
                        .unwrap_or_default();
                    let text = match &c["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(xs) => xs.iter().filter_map(|x| x["text"].as_str()).collect(),
                        _ => String::new(),
                    };
                    if c["is_error"].as_bool() == Some(true) {
                        return Err(text);
                    }
                    let Ok(body) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if name.ends_with("getAccessibleAtlassianResources") {
                        site = body["data"]["resources"][0]["url"]
                            .as_str()
                            .map(String::from);
                    } else if name.ends_with("executeRead") {
                        cfg = Some(body["data"]["columnConfig"]["columns"].clone());
                    } else if name.ends_with("searchJiraIssuesUsingJql") {
                        issues = Some(body["issues"]["nodes"].clone());
                    }
                }
                _ => {}
            }
        }
        if v["type"] == "result" && v["is_error"].as_bool() == Some(true) {
            return Err(v["result"]
                .as_str()
                .unwrap_or("o claude falhou")
                .to_string());
        }
    }
    let cols = cfg.ok_or("o Jira não devolveu a config do board")?;
    let issues = issues.ok_or("o Jira não devolveu os cards do board")?;
    let cols = cols
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let sts = c["statuses"].as_array()?;
            (!sts.is_empty()).then(|| Column {
                name: c["name"].as_str().unwrap_or_default().to_string(),
                statuses: sts
                    .iter()
                    .filter_map(|s| s["name"].as_str().map(String::from))
                    .collect(),
                done: sts.iter().all(|s| s["category"] == "done"),
            })
        })
        .collect();
    let cards = issues
        .as_array()
        .into_iter()
        .flatten()
        .map(|i| {
            let key = i["key"].as_str().unwrap_or_default().to_string();
            let f = &i["fields"];
            let proj = key.split('-').next().unwrap_or_default();
            let acr = f["issuelinks"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|l| {
                    let o = if l["outwardIssue"].is_object() {
                        &l["outwardIssue"]
                    } else {
                        &l["inwardIssue"]
                    };
                    let k = o["key"].as_str()?;
                    let ok = match acr_link {
                        Some(t) => l["type"]["name"] == t,
                        None => k.split('-').next() != Some(proj),
                    };
                    ok.then(|| Linked {
                        key: k.to_string(),
                        status: o["fields"]["status"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    })
                });
            PrcCard {
                key,
                title: f["summary"].as_str().unwrap_or_default().trim().to_string(),
                status: f["status"]["name"].as_str().unwrap_or_default().to_string(),
                labels: f["labels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|l| l.as_str().map(String::from))
                    .collect(),
                acr,
            }
        })
        .collect();
    Ok(Board {
        site: site.unwrap_or_default(),
        cols,
        cards,
    })
}

fn parse<T: DeserializeOwned>(result: &str) -> Result<T, String> {
    let (a, b) = (result.find('{'), result.rfind('}'));
    let json = match (a, b) {
        (Some(a), Some(b)) if b > a => &result[a..=b],
        _ => return Err("o Claude não devolveu o JSON".into()),
    };
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chave_e_resposta() {
        assert!(is_key("PRD-171500"));
        assert!(!is_key("prd-1") && !is_key("PRD") && !is_key("PRD-x"));
        assert_eq!(key_from(" prd-172747 ").as_deref(), Some("PRD-172747"));
        assert_eq!(
            key_from("https://x.atlassian.net/browse/PRD-172747?focused=1").as_deref(),
            Some("PRD-172747")
        );
        assert_eq!(
            key_from("https://x.atlassian.net/jira/software/projects/PRD/boards/7?selectedIssue=PRD-9&a=b")
                .as_deref(),
            Some("PRD-9")
        );
        assert_eq!(key_from("https://x.atlassian.net/jira/your-work"), None);
        let ok = "```json\n{\"type\":\"Story\",\"title\":\"Paginação\",\"criteria\":\"page e size\"}\n```";
        assert_eq!(parse::<Issue>(ok).unwrap().title, "Paginação");
        assert_eq!(
            parse::<Issue>("{\"error\":\"Issue \\\"PRD-1\\\" not found\"}").unwrap_err(),
            "Issue \"PRD-1\" not found"
        );
    }

    #[test]
    fn board_sai_do_tool_result() {
        let fx = include_str!("fixtures/prc-board.jsonl");
        let b = parse_board(fx, None).unwrap();
        assert_eq!(b.site, "https://x.atlassian.net");
        let cols: Vec<_> = b
            .cols
            .iter()
            .map(|c| (c.name.as_str(), c.statuses.len(), c.done))
            .collect();
        assert_eq!(
            cols,
            [
                ("triagem", 1, false),
                ("em andamento", 1, false),
                ("com dev", 2, false),
                ("resolvido", 1, true)
            ]
        );
        let keys: Vec<_> = b.cards.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["PRC-1", "PRC-2", "PRC-3"]);
        assert_eq!(b.cards[0].labels, ["falha_envio_email", "escalado"]);
        assert_eq!(b.cards[0].acr, None);
        assert_eq!(
            b.cards[1].acr,
            Some(Linked {
                key: "ACR-9".into(),
                status: "Backlog".into()
            })
        );
        let typed = parse_board(fx, Some("Problem/Incident")).unwrap();
        assert_eq!(
            typed.cards[1].acr.as_ref().map(|a| a.key.as_str()),
            Some("ACR-9")
        );
        assert_eq!(parse_board(fx, Some("Blocks")).unwrap().cards[1].acr, None);
        let err = "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"id\":\"a\",\"name\":\"executeRead\"}]}}\n\
                   {\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"a\",\"is_error\":true,\"content\":\"401 sem acesso\"}]}}";
        assert_eq!(parse_board(err, None).unwrap_err(), "401 sem acesso");
        assert!(parse_board("", None).is_err());
    }

    #[test]
    fn card_key_e_ok() {
        let reply = r#"Pronto:
{"key":"PRD-1","type":"Story","status":"Em andamento","squad":"Squad Bots","title":"Paginação","desc":"page e size",
"site":"https://x.atlassian.net","subs":[{"key":"PRD-2","type":"Desenvolvimento","title":"[bots-api] paginar","sp":2,
"status":"Building","transitions":["Code Review","Pull Request"]},{"key":"PRD-3","type":"Qualidade e Testes","title":"e2e","sp":null,"status":"Tarefas pendentes"}]}"#;
        let c = parse::<Card>(reply).unwrap();
        assert_eq!((c.squad.as_str(), c.subs.len()), ("Squad Bots", 2));
        assert_eq!(c.subs[0].sp, Some(2.));
        assert_eq!(c.subs[0].transitions, ["Code Review", "Pull Request"]);
        assert_eq!((c.subs[1].sp, c.subs[1].transitions.len()), (None, 0));
        assert_eq!(
            parse::<Created>("{\"key\":\"PRD-9\"}").unwrap().key,
            "PRD-9"
        );
        assert!(parse::<Value>("```json\n{\"ok\":true}\n```").is_ok());
        assert_eq!(
            parse::<Value>("{\"error\":\"sem transição para Pull Request\"}").unwrap_err(),
            "sem transição para Pull Request"
        );
        assert!(parse::<Value>("não consegui").is_err());
    }
}
