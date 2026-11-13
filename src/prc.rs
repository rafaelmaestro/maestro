//! PRCs: o board de chamados do Jira e a base de receitas no vault (`prcs/`).
//! O board e a empresa ficam na config local, fora do repo.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::jira::{Board, Column, PrcCard};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Config {
    pub board: u64,
    #[serde(default = "default_repos")]
    pub repos: String,
    /// o tipo de vínculo PRC↔ACR no Jira (ex.: `Problem/Incident`)
    #[serde(default)]
    pub acr_link: Option<String>,
}

fn default_repos() -> String {
    "~/repos".into()
}

pub const EXAMPLE: &str = "{\n  \"board\": 123,\n  \"repos\": \"~/repos\"\n}";

pub fn config_path() -> PathBuf {
    std::env::var_os("MAESTRO_PRC").map_or_else(
        || {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".config/maestro/prc.json")
        },
        PathBuf::from,
    )
}

/// `Ok(None)`: sem config (a tela mostra o exemplo); `Err`: config inválida.
pub fn load() -> Result<Option<Config>, String> {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("{}: {e}", config_path().display())),
        Err(_) => Ok(None),
    }
}

/// Os cards de cada coluna, na ordem do board. Card com status fora das colunas não aparece.
pub fn columns(b: &Board) -> Vec<(&Column, Vec<&PrcCard>)> {
    b.cols
        .iter()
        .map(|c| {
            let xs = if c.done {
                Vec::new()
            } else {
                b.cards
                    .iter()
                    .filter(|x| c.statuses.contains(&x.status))
                    .collect()
            };
            (c, xs)
        })
        .collect()
}

/// O status de destino ao soltar um card na coluna: o primeiro que ela mapeia.
pub fn target(c: &Column) -> Option<&str> {
    c.statuses.first().map(String::as_str)
}

/// As receitas de `prcs/` que batem com as labels do card.
pub fn recipes(labels: &[String], vault: &Path) -> Vec<String> {
    labels
        .iter()
        .filter(|l| vault.join("prcs").join(format!("{l}.md")).is_file())
        .cloned()
        .collect()
}

/// As notas de `prcs/` (sem `_acr.md`), para a IA procurar quando nenhuma label bate.
pub fn notes(vault: &Path) -> Vec<String> {
    let mut xs: Vec<String> = std::fs::read_dir(vault.join("prcs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let stem = n.strip_suffix(".md")?;
            (!stem.starts_with('_')).then(|| stem.to_string())
        })
        .collect();
    xs.sort();
    xs
}

/// `~` no começo vira o HOME (a config vem escrita à mão).
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}

/// A pasta da sessão de um card: cwd do Claude, contexto, settings e o `result.json`.
/// Fica fora da pasta de repos para não herdar o `.claude/settings.local.json` dela.
pub fn session_dir(key: &str) -> PathBuf {
    std::env::var_os("MAESTRO_PRC_SESSIONS")
        .map_or_else(
            || {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join(".local/share/maestro/prcs")
            },
            PathBuf::from,
        )
        .join(key)
}

pub fn label(key: &str) -> String {
    format!("{key} · análise")
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Acr {
    pub causa: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub loc: String,
}

/// O `result.json` que o Claude grava ao fim da análise e atualiza a cada passo da contingência.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Analysis {
    pub diag: String,
    #[serde(default)]
    pub ev: Vec<String>,
    #[serde(default)]
    pub cont: Option<Vec<String>>,
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub acr: Option<Acr>,
    #[serde(default)]
    pub no_acr: Option<String>,
}

/// `None`: ainda analisando; `Some(Err)`: o arquivo existe, mas não está no formato.
pub fn result(dir: &Path) -> Option<Result<Analysis, String>> {
    let s = std::fs::read_to_string(dir.join("result.json")).ok()?;
    Some(serde_json::from_str(&s).map_err(|e| format!("result.json fora do formato: {e}")))
}

const READS: [&str; 4] = [
    "getAccessibleAtlassianResources",
    "getJiraIssue",
    "searchJiraIssuesUsingJql",
    "listJiraIssueComments",
];

/// Shell só de leitura: o hook libera sem pedir quando todas as partes do comando começam por um destes.
/// Regra de prefixo no settings não serve: `cat a > b` passaria por `Bash(cat:*)`.
const BASH_READ: &[&str] = &[
    "git status",
    "git log",
    "git show",
    "git diff",
    "git blame",
    "git ls-files",
    "git grep",
    "git rev-parse",
    "rg",
    "grep",
    "cat",
    "head",
    "tail",
    "ls",
    "wc",
    "jq",
    "tree",
    "stat",
    "file",
    "pwd",
    "which",
    "glab mr view",
    "glab mr list",
    "glab mr diff",
    "glab issue view",
    "glab ci view",
];

/// Sem os settings de usuário nem os da pasta de repos, todo Bash e todo MCP pedem aprovação;
/// aqui só se libera leitura e o resultado, e se nega editar os repos.
pub fn settings_json(session: &Path, repos: &Path) -> String {
    let abs = |p: &Path| format!("/{}/**", p.display());
    let mut allow = vec![
        "Read".to_string(),
        "Grep".into(),
        "Glob".into(),
        format!("Edit({})", abs(session)),
    ];
    allow.extend(READS.map(|t| format!("mcp__claude_ai_Atlassian_MCP__{t}")));
    let deny = vec![
        format!("Edit({})", abs(repos)),
        format!("NotebookEdit({})", abs(repos)),
        "mcp__ripwire__replace_symbol_body".into(),
        "mcp__ripwire__insert_after_symbol".into(),
        "mcp__ripwire__insert_before_symbol".into(),
    ];
    let exe = std::env::current_exe().unwrap_or_default();
    serde_json::json!({
        "permissions": { "allow": allow, "deny": deny },
        "hooks": { "PreToolUse": [ {
            "matcher": "Bash|mcp__dbx__.*",
            "hooks": [ { "type": "command", "command": format!("{} prc-guard", crate::feature::sh_quote(&exe.to_string_lossy())) } ]
        } ] }
    })
    .to_string()
}

/// O hook PreToolUse do dbx na sessão de PRC: consulta só de leitura roda sem pedir;
/// escrita fica com o Claude Code, que pede aprovação (a contingência pode precisar escrever).
pub fn guard(stdin: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stdin).ok()?;
    let tool = v["tool_name"].as_str()?;
    if tool == "Bash" {
        return read_only_bash(v["tool_input"]["command"].as_str()?).then(|| "shell".into());
    }
    if tool == "mcp__dbx__dbx_execute_query" {
        crate::ai_review::read_only_sql(v["tool_input"]["sql"].as_str()?)
            .is_none()
            .then(|| "SELECT".into())
    } else {
        const READ: [&str; 7] = [
            "dbx_list_connections",
            "dbx_list_databases",
            "dbx_list_tables",
            "dbx_describe_table",
            "dbx_get_schema_context",
            "dbx_list_routines",
            "dbx_get_routine_source",
        ];
        READ.iter()
            .any(|r| tool == format!("mcp__dbx__{r}"))
            .then(|| "leitura de schema".into())
    }
}

/// `true` só se cada parte (`;`, `&&`, `||`, `|`, linhas) é `cd` ou começa por um comando de leitura,
/// sem redirecionamento nem substituição. Na dúvida, `false`: o Claude Code pergunta.
pub fn read_only_bash(cmd: &str) -> bool {
    let cmd = &unquote(cmd);
    const BAD: [&str; 8] = [">", "`", "$(", "<(", "--output", "--pre", "--exec", "\\"];
    if BAD.iter().any(|b| cmd.contains(b)) {
        return false;
    }
    let parts = cmd
        .split(['\n', ';', '|'])
        .flat_map(|p| p.split("&&"))
        .map(str::trim)
        .filter(|p| !p.is_empty());
    let mut any = false;
    for p in parts {
        any = true;
        let mut w: Vec<&str> = p.split_whitespace().collect();
        if w.first() == Some(&"git") && w.get(1) == Some(&"-C") && w.len() > 3 {
            w.drain(1..3);
        }
        let line = w.join(" ");
        let ok = w.first() == Some(&"cd")
            || BASH_READ
                .iter()
                .chain(crate::audit::PROD_READ)
                .any(|c| line == *c || line.starts_with(&format!("{c} ")));
        if !ok {
            return false;
        }
    }
    any
}

/// O conteúdo entre aspas vira `Q`: `>` e `|` dentro de um filtro ou regex não são redirecionamento nem pipe.
/// Aspas sem fechar deixam o resto como está, e aí a checagem tende a recusar.
fn unquote(cmd: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                out.push('Q');
            }
            None => out.push(c),
        }
    }
    out
}

pub fn guard_main(stdin: &str) {
    if let Some(r) = guard(stdin) {
        println!(
            "{}",
            serde_json::json!({ "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": format!("só leitura ({r})")
            } })
        );
    }
}

/// Os MCPs de usuário (`mcpServers` do `~/.claude.json`) somem com `--setting-sources local`.
pub fn mcp_json(claude_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(claude_json).unwrap_or_default();
    serde_json::json!({ "mcpServers": v.get("mcpServers").cloned().unwrap_or_else(|| serde_json::json!({})) })
        .to_string()
}

pub const EXEC_MSG: &str = "Execute a contingência proposta, um passo por vez.";
pub const ACR_MSG: &str = "Crie a ACR seguindo a seção \"Criar a ACR\".";

pub fn context(card: &PrcCard, session: &Path, repos: &Path, vault: &Path) -> String {
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap_or_default();
    let recipes = recipes(&card.labels, vault);
    let notes_txt = if recipes.is_empty() {
        let all = notes(vault);
        format!(
            "Nenhuma nota bate com as labels do card ({}). Notas existentes em {}: {}. Leia as que parecerem do mesmo tipo.",
            card.labels.join(", "),
            vault.join("prcs").display(),
            if all.is_empty() {
                "nenhuma".into()
            } else {
                all.join(", ")
            }
        )
    } else {
        recipes
            .iter()
            .map(|n| {
                format!(
                    "### prcs/{n}.md\n\n{}",
                    read(vault.join("prcs").join(format!("{n}.md")))
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let acr = read(vault.join("prcs/_acr.md"));
    let acr = if acr.trim().is_empty() {
        "Sem receita (prcs/_acr.md não existe): entregue o texto da ACR pronto para colar e não crie card.".to_string()
    } else {
        acr
    };
    format!(
        "# Análise do {key}\n\n\
         Você analisa o chamado {key} (\"{title}\") do board de PRCs. Responda em português.\n\n\
         ## Regras\n\n\
         - Até o resultado, só leia: o card e os comentários no Jira, as receitas abaixo, os repos em {repos} e produção só leitura (SELECT, logs). Nada que escreva.\n\
         - Nunca edite arquivo de repo. Correção de código é caminho da ACR.\n\
         - Separe a contingência (destravar o cliente sem mudar código, em passos executáveis) da solução definitiva (a causa raiz no código, que pede ACR).\n\
         - Nunca grave dado de cliente (nome, CNPJ, e-mail, telefone) fora do Jira.\n\n\
         ## Resultado\n\n\
         Ao fim da análise, grave {result} exatamente neste formato e mostre o mesmo resumo no terminal:\n\n\
         {{\"diag\":\"diagnóstico em uma ou duas frases\",\"ev\":[\"evidência: arquivo:linha, log ou consulta\"],\"cont\":[\"passo\"] ou null,\"done\":0,\"acr\":{{\"causa\":\"\",\"repo\":\"\",\"loc\":\"arquivo:linha\"}} ou null,\"no_acr\":\"por que não precisa de ACR, quando acr for null\"}}\n\n\
         Depois espere instruções.\n\n\
         ## Executar a contingência\n\n\
         Quando pedirem: um passo por vez, na ordem de \"cont\". Depois de cada passo concluído, atualize \"done\" no result.json.\n\n\
         ## Criar a ACR\n\n\
         Quando pedirem, use a causa raiz já analisada, sem investigar de novo, mostre o card antes de criar e siga esta receita:\n\n{acr}\n\n\
         ## Fechar\n\n\
         No fim da análise e no fim da contingência, proponha criar ou atualizar a nota da base em {prcs}/<label>.md no modelo abaixo, no mínimo com uma linha no Histórico.\n\n\
         ## Receitas que batem com o card\n\n{notes_txt}\n\n\
         ## Modelo de nota\n\n{template}\n",
        key = card.key,
        title = card.title,
        repos = repos.display(),
        result = session.join("result.json").display(),
        prcs = vault.join("prcs").display(),
        template = read(vault.join("templates/prc.md")),
    )
}

/// O comando digitado no pane da sessão (cwd = pasta da sessão).
pub fn command(card: &PrcCard, session: &Path, repos: &Path, vault: &Path) -> String {
    let q = |p: &Path| crate::feature::sh_quote(&p.to_string_lossy());
    format!(
        "claude --permission-mode default --setting-sources local --settings {} --mcp-config {} --add-dir {} --add-dir {} --append-system-prompt \"$(cat {})\" {}",
        q(&session.join("settings.json")),
        q(&session.join("mcp.json")),
        q(repos),
        q(vault),
        q(&session.join("ctx.md")),
        crate::feature::sh_quote(&format!("Analise o {}: {}", card.key, card.title)),
    )
}

/// Grava contexto, settings e mcp.json da sessão e devolve o comando.
pub fn prepare(
    card: &PrcCard,
    session: &Path,
    repos: &Path,
    vault: &Path,
) -> std::io::Result<String> {
    std::fs::create_dir_all(session)?;
    let _ = std::fs::remove_file(session.join("result.json"));
    std::fs::write(session.join("ctx.md"), context(card, session, repos, vault))?;
    std::fs::write(session.join("settings.json"), settings_json(session, repos))?;
    let claude = std::fs::read_to_string(
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".claude.json"),
    )
    .unwrap_or_default();
    std::fs::write(session.join("mcp.json"), mcp_json(&claude))?;
    Ok(command(card, session, repos, vault))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jira::parse_board;

    #[test]
    fn colunas_e_destino() {
        let b = parse_board(include_str!("fixtures/prc-board.jsonl"), None).unwrap();
        let cols: Vec<_> = columns(&b)
            .iter()
            .map(|(c, xs)| {
                (
                    c.name.clone(),
                    xs.iter().map(|x| x.key.clone()).collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            cols,
            [
                ("triagem".to_string(), vec!["PRC-1".to_string()]),
                ("em andamento".into(), vec![]),
                ("com dev".into(), vec!["PRC-2".into()]),
                ("resolvido".into(), vec![]),
            ]
        );
        assert_eq!(target(&b.cols[2]), Some("Aguardando Dev"));
        assert_eq!(
            serde_json::from_str::<Config>("{\"board\":7}").unwrap(),
            Config {
                board: 7,
                repos: "~/repos".into(),
                acr_link: None
            }
        );
    }

    fn card() -> PrcCard {
        PrcCard {
            key: "PRC-1".into(),
            title: "E-mail do cliente's não chega".into(),
            status: "Backlog".into(),
            labels: vec!["falha_envio_email".into(), "escalado".into()],
            acr: None,
        }
    }

    #[test]
    fn sessao_isolada() {
        let (s, repos, vault) = (
            Path::new("/h/s/PRC-1"),
            Path::new("/h/repos"),
            Path::new("/h/vault"),
        );
        let cmd = command(&card(), s, repos, vault);
        for part in [
            "--permission-mode default",
            "--setting-sources local",
            "--settings '/h/s/PRC-1/settings.json'",
            "--mcp-config '/h/s/PRC-1/mcp.json'",
            "--add-dir '/h/repos' --add-dir '/h/vault'",
            r#""$(cat '/h/s/PRC-1/ctx.md')""#,
            r#"'Analise o PRC-1: E-mail do cliente'\''s não chega'"#,
        ] {
            assert!(cmd.contains(part), "{part} em {cmd}");
        }
        let v: serde_json::Value = serde_json::from_str(&settings_json(s, repos)).unwrap();
        let allow: Vec<&str> = v["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        let deny: Vec<&str> = v["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x.as_str())
            .collect();
        assert!(allow.contains(&"Edit(//h/s/PRC-1/**)"));
        assert!(allow.contains(&"mcp__claude_ai_Atlassian_MCP__getJiraIssue"));
        assert!(
            !allow
                .iter()
                .any(|a| a.contains("dbx") || a.starts_with("Bash"))
        );
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Bash|mcp__dbx__.*");
        assert!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with(" prc-guard")
        );
        let q = |t: &str, sql: &str| {
            guard(&serde_json::json!({"tool_name": t, "tool_input": {"sql": sql}}).to_string())
        };
        assert!(
            q(
                "mcp__dbx__dbx_execute_query",
                "select * from t where id = 1"
            )
            .is_some()
        );
        assert_eq!(q("mcp__dbx__dbx_execute_query", "update t set a = 1"), None);
        assert_eq!(
            q("mcp__dbx__dbx_execute_query", "select 1; delete from t"),
            None
        );
        assert!(q("mcp__dbx__dbx_describe_table", "").is_some());
        assert_eq!(q("mcp__dbx__dbx_execute_batch", "select 1"), None);
        for ok in [
            "git -C /r/app log -1 --oneline",
            "cd /r/app && git log -3",
            "rg -l export src | head -3",
            "gcloud logging read 'severity>=ERROR AND timestamp>=\"2026-11-01\"' --limit 5",
            "rg -n 'foo|bar' src",
            "kubectl logs pod-1 --tail 50",
        ] {
            assert!(read_only_bash(ok), "{ok}");
        }
        for no in [
            "cat a > b",
            "cat 'a' >b",
            "rg 'x' \"y\" | sh",
            "git log -1; touch x",
            "rg x | xargs rm",
            "rg --pre sh x",
            "git diff --output=/tmp/x",
            "echo $(rm -rf x)",
            "glab api -X POST x",
            "kubectl get secrets -o yaml",
            "git -C /r push",
            "",
        ] {
            assert!(!read_only_bash(no), "{no}");
        }
        let b = |c: &str| {
            guard(
                &serde_json::json!({"tool_name": "Bash", "tool_input": {"command": c}}).to_string(),
            )
        };
        assert!(b("git log -1").is_some() && b("rm -rf /").is_none());
        assert!(
            deny.contains(&"Edit(//h/repos/**)") && deny.contains(&"NotebookEdit(//h/repos/**)")
        );
        assert!(deny.contains(&"mcp__ripwire__replace_symbol_body"));
        let m: serde_json::Value = serde_json::from_str(&mcp_json(
            r#"{"mcpServers":{"dbx":{"command":"x"}},"projects":{}}"#,
        ))
        .unwrap();
        assert_eq!(m, serde_json::json!({"mcpServers":{"dbx":{"command":"x"}}}));
        assert_eq!(mcp_json("lixo"), r#"{"mcpServers":{}}"#);
    }

    #[test]
    fn resultado_da_analise() {
        let dir = std::env::temp_dir().join(format!("maestro-prc-r-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(result(&dir).is_none());
        std::fs::write(
            dir.join("result.json"),
            r#"{"diag":"d","cont":["a","b"],"done":1,"acr":null,"no_acr":"dado pontual"}"#,
        )
        .unwrap();
        let a = result(&dir).unwrap().unwrap();
        assert_eq!(
            (a.cont.unwrap().len(), a.done, a.acr, a.no_acr.as_deref()),
            (2, 1, None, Some("dado pontual"))
        );
        std::fs::write(
            dir.join("result.json"),
            r#"{"diag":"d","cont":null,"acr":{"causa":"c","repo":"loja-api","loc":"src/a.ts:4"}}"#,
        )
        .unwrap();
        let a = result(&dir).unwrap().unwrap();
        assert_eq!(
            (a.cont, a.acr.map(|x| x.repo)),
            (None, Some("loja-api".to_string()))
        );
        std::fs::write(dir.join("result.json"), "pela metade").unwrap();
        assert!(result(&dir).unwrap().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn contexto_com_e_sem_receita() {
        let v = std::env::temp_dir().join(format!("maestro-prc-c-{}", std::process::id()));
        std::fs::create_dir_all(v.join("prcs")).unwrap();
        std::fs::create_dir_all(v.join("templates")).unwrap();
        std::fs::write(v.join("templates/prc.md"), "## Sintoma").unwrap();
        let (s, repos) = (Path::new("/h/s/PRC-1"), Path::new("/h/repos"));
        let sem = context(&card(), s, repos, &v);
        assert!(sem.contains("Nenhuma nota bate") && sem.contains("Sem receita (prcs/_acr.md"));
        assert!(sem.contains("/h/s/PRC-1/result.json") && sem.contains("## Sintoma"));
        std::fs::write(v.join("prcs/falha_envio_email.md"), "tirar da supressão").unwrap();
        std::fs::write(v.join("prcs/_acr.md"), "tipo X, vínculo Y").unwrap();
        let com = context(&card(), s, repos, &v);
        assert!(
            com.contains("### prcs/falha_envio_email.md") && com.contains("tirar da supressão")
        );
        assert!(com.contains("tipo X, vínculo Y") && !com.contains("Sem receita"));
        std::fs::remove_dir_all(&v).unwrap();
    }

    #[test]
    fn receitas_do_vault() {
        let dir = std::env::temp_dir().join(format!("maestro-prc-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("prcs")).unwrap();
        for n in ["falha_envio_email", "alteracao_email", "_acr"] {
            std::fs::write(dir.join("prcs").join(format!("{n}.md")), "# x").unwrap();
        }
        let labels = ["escalado".to_string(), "falha_envio_email".to_string()];
        assert_eq!(recipes(&labels, &dir), ["falha_envio_email"]);
        assert_eq!(notes(&dir), ["alteracao_email", "falha_envio_email"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
