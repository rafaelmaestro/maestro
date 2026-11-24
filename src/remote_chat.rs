//! A conversa que o celular mostra: lida do JSONL que o Claude Code grava, não do terminal.

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use serde_json::{Value, json};

const TAIL: u64 = 768 * 1024;
const MAX_ITEMS: usize = 80;
const CUT: usize = 160;

fn cut(s: &str) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    match line.char_indices().nth(CUT) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// Só o fim do arquivo: o transcript de uma sessão longa passa de dezenas de MB.
pub fn tail(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    Ok(if len > TAIL {
        text.split_once('\n')
            .map(|(_, r)| r.to_string())
            .unwrap_or_default()
    } else {
        text
    })
}

fn arg(name: &str, input: &Value, cwd: &str) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    let a = match name {
        "Bash" => s("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let p = s("file_path");
            p.strip_prefix(cwd)
                .map(|r| r.trim_start_matches('/').to_string())
                .unwrap_or(p)
        }
        "Grep" | "Glob" => s("pattern"),
        "Task" | "Agent" => s("description"),
        "WebFetch" => s("url"),
        _ => input
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string)))
            .unwrap_or_default(),
    };
    cut(&a)
}

fn result_text(c: &Value) -> String {
    match c {
        Value::String(s) => cut(s),
        Value::Array(a) => cut(&a
            .iter()
            .filter_map(|x| x["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")),
        _ => String::new(),
    }
}

/// O que não é conversa: lembretes do sistema, comandos locais, notificações de tarefa.
fn noise(t: &str) -> bool {
    let t = t.trim_start();
    t.is_empty() || t.starts_with('<') || t.starts_with("Caveat:")
}

/// `{items, pending}`: mensagens (`u` sua, `c` do Claude, `tools` agrupadas) e a última ferramenta sem resultado.
pub fn parse(text: &str, cwd: &str) -> Value {
    let mut items: Vec<Value> = Vec::new();
    let mut pending: Vec<(String, Value)> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["isSidechain"] == true || v["isMeta"] == true {
            continue;
        }
        let content = &v["message"]["content"];
        match v["type"].as_str() {
            Some("user") => {
                let parts: Vec<Value> = match content {
                    Value::String(s) => vec![json!({ "type": "text", "text": s })],
                    Value::Array(a) => a.clone(),
                    _ => continue,
                };
                for p in parts {
                    match p["type"].as_str() {
                        Some("text") => {
                            let t = p["text"].as_str().unwrap_or("");
                            if !noise(t) {
                                items.push(json!({ "k": "u", "t": t.trim() }));
                            }
                        }
                        Some("tool_result") => {
                            let id = p["tool_use_id"].as_str().unwrap_or("");
                            pending.retain(|(pid, _)| pid != id);
                            let r = result_text(&p["content"]);
                            let err = p["is_error"] == true;
                            for it in items.iter_mut().rev() {
                                let Some(l) = it["l"].as_array_mut() else {
                                    continue;
                                };
                                if let Some(t) = l.iter_mut().find(|t| t["id"] == id) {
                                    t["r"] = json!(r);
                                    t["err"] = json!(err);
                                    break;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                for p in content.as_array().into_iter().flatten() {
                    match p["type"].as_str() {
                        Some("text") => {
                            let t = p["text"].as_str().unwrap_or("").trim();
                            if !t.is_empty() {
                                items.push(json!({ "k": "c", "t": t }));
                            }
                        }
                        Some("tool_use") => {
                            let name = p["name"].as_str().unwrap_or("");
                            let id = p["id"].as_str().unwrap_or("").to_string();
                            let tool = json!({ "id": id, "n": name, "a": arg(name, &p["input"], cwd), "r": "" });
                            pending.push((
                                id,
                                json!({
                                    "n": name,
                                    "a": arg(name, &p["input"], cwd),
                                    "desc": cut(p["input"]["description"].as_str().unwrap_or("")),
                                }),
                            ));
                            match items.last_mut().and_then(|l| l["l"].as_array_mut()) {
                                Some(l) => l.push(tool),
                                None => items.push(json!({ "k": "tools", "l": [tool] })),
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let skip = items.len().saturating_sub(MAX_ITEMS);
    json!({
        "items": items.into_iter().skip(skip).collect::<Vec<_>>(),
        "pending": pending.pop().map(|(_, p)| p),
    })
}

fn label(raw: &str) -> String {
    let l = raw
        .trim()
        .trim_end_matches("(esc)")
        .trim_end_matches("(shift+tab)")
        .trim();
    if l == "Yes" {
        return "Sim".into();
    }
    if let Some(r) = l.strip_prefix("Yes, and don't ask again") {
        // o caminho do projeto no fim só ocupa a tela do celular
        let r = r.split(" in /").next().unwrap_or(r);
        return format!(
            "Sim, e não perguntar de novo{}",
            r.replacen(" for ", " para ", 1)
        );
    }
    if l.starts_with("No, and tell Claude") {
        return "Não, e dizer o que fazer".into();
    }
    if l == "No" {
        return "Não".into();
    }
    l.to_string()
}

/// As opções do menu que o Claude mostra na tela (`❯ 1. Yes`, `  2. …`): o último bloco numerado a partir do 1.
pub fn menu(screen: &str) -> Vec<Value> {
    let mut found: Vec<(u32, String)> = Vec::new();
    for line in screen.lines() {
        let t = line
            .trim_start_matches(|c: char| c.is_whitespace() || c == '❯' || c == '│' || c == '>');
        let Some((n, rest)) = t.split_once(". ") else {
            continue;
        };
        let Ok(n) = n.trim().parse::<u32>() else {
            continue;
        };
        if !(1..=9).contains(&n) || rest.trim().is_empty() {
            continue;
        }
        if n == 1 {
            found.clear();
        }
        if found.len() as u32 + 1 == n {
            found.push((n, rest.trim_end_matches('│').trim().to_string()));
        }
    }
    found
        .into_iter()
        .map(|(n, l)| json!({ "key": n.to_string(), "label": label(&l) }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSONL: &str = r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"}}
{"type":"user","message":{"role":"user","content":"deixa o timeout do slack configurável"}}
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"..."},{"type":"text","text":"Vou ler o client."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/w/fix/src/slack.ts"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"export const client = ...\nmais"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/w/fix/src/slack.ts"}}]}}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"Updated"}]}]}}
{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagente"}]}}
{"type":"user","isMeta":true,"message":{"role":"user","content":"meta"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Agora os testes."},{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"npm test -- slack","description":"Roda os testes do client"}}]}}
lixo que não é json"#;

    #[test]
    fn monta_a_conversa_do_transcript() {
        let v = parse(JSONL, "/w/fix");
        let items = v["items"].as_array().unwrap();
        let kinds: Vec<&str> = items.iter().map(|i| i["k"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["u", "c", "tools", "c", "tools"]);
        assert_eq!(items[0]["t"], "deixa o timeout do slack configurável");
        let tools = items[2]["l"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["a"], "src/slack.ts");
        assert_eq!(tools[0]["r"], "export const client = ...");
        assert_eq!(tools[1]["r"], "Updated");
        assert_eq!(v["pending"]["n"], "Bash");
        assert_eq!(v["pending"]["a"], "npm test -- slack");
        assert_eq!(v["pending"]["desc"], "Roda os testes do client");
    }

    #[test]
    fn sem_pendente_quando_tudo_tem_resultado() {
        let done = JSONL.lines().take(6).collect::<Vec<_>>().join("\n");
        assert!(parse(&done, "/w/fix")["pending"].is_null());
    }

    #[test]
    fn le_so_o_fim_do_arquivo_grande() {
        let dir = std::env::temp_dir().join(format!("maestro-chat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.jsonl");
        let line = r#"{"type":"user","message":{"content":"x"}}"#;
        let big = format!("{}\n", line).repeat((TAIL as usize / line.len()) * 2);
        std::fs::write(&p, &big).unwrap();
        let t = tail(&p).unwrap();
        assert!(t.len() as u64 <= TAIL);
        assert!(t.lines().all(|l| l == line));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn le_o_menu_de_permissao() {
        let screen = "Bash command\n\n  npm test -- slack\n  Roda os testes\n\nDo you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again for npm test commands in /w/fix\n  3. No, and tell Claude what to do differently (esc)\n";
        let m = menu(screen);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], json!({ "key": "1", "label": "Sim" }));
        assert_eq!(
            m[1]["label"],
            "Sim, e não perguntar de novo para npm test commands"
        );
        assert_eq!(
            m[2],
            json!({ "key": "3", "label": "Não, e dizer o que fazer" })
        );
    }

    #[test]
    fn menu_pega_o_ultimo_bloco_e_ignora_listas_soltas() {
        let screen = "Passos:\n1. ler\n2. testar\n\n│ Ready to code?\n│ ❯ 1. Yes, and auto-accept edits │\n│   2. Yes, and manually approve edits │\n│   3. No, keep planning │\n";
        let m = menu(screen);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["label"], "Yes, and auto-accept edits");
        assert_eq!(m[2]["label"], "No, keep planning");
        assert!(menu("sem menu aqui").is_empty());
    }
}
