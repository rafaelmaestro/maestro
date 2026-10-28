use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// `~/vault` (ou `MAESTRO_VAULT`, para testar numa cópia).
pub fn dir() -> PathBuf {
    std::env::var_os("MAESTRO_VAULT").map_or_else(
        || PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("vault"),
        PathBuf::from,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// `repos/bots-api.md`
    pub key: String,
    /// `repos`, `decisions`, `daily` ou `.` (raiz)
    pub dir: String,
    pub name: String,
    pub text: String,
}

impl Note {
    pub fn links(&self) -> Vec<String> {
        links(&self.text)
    }
}

/// Os `[[nome]]` do texto, sem repetir (`[[nome|apelido]]` e `[[nome#seção]]` contam como `nome`).
pub fn links(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("[[") {
        rest = &rest[i + 2..];
        let Some(j) = rest.find("]]") else { break };
        let name = rest[..j].split(['|', '#']).next().unwrap_or("").trim();
        if !name.is_empty() && seen.insert(name.to_string()) {
            out.push(name.to_string());
        }
        rest = &rest[j + 2..];
    }
    out
}

/// As notas do vault, menos `templates/` e as pastas escondidas.
pub fn load(root: &Path) -> Vec<Note> {
    let mut notes = Vec::new();
    let mut walk = vec![root.to_path_buf()];
    while let Some(d) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || (d == root && name == "templates") {
                continue;
            }
            if p.is_dir() {
                walk.push(p);
            } else if let Some(stem) = name.strip_suffix(".md") {
                let key = p
                    .strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned();
                let dir = key
                    .rsplit_once('/')
                    .map_or(".".into(), |(d, _)| d.to_string());
                notes.push(Note {
                    key,
                    dir,
                    name: stem.into(),
                    text: std::fs::read_to_string(&p).unwrap_or_default(),
                });
            }
        }
    }
    notes.sort_by(|a, b| a.key.cmp(&b.key));
    notes
}

pub fn find<'a>(notes: &'a [Note], name: &str) -> Option<&'a Note> {
    notes.iter().find(|n| n.name == name)
}

/// "Mencionado em": quem linka esta nota.
pub fn backlinks<'a>(notes: &'a [Note], name: &str) -> Vec<&'a Note> {
    notes
        .iter()
        .filter(|n| n.name != name && n.links().iter().any(|l| l == name))
        .collect()
}

pub fn today() -> String {
    std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// O que a feature recebe no `--append-system-prompt`: `repos/<repo>.md` e as notas que ela linka.
pub fn context(notes: &[Note], repo: &str) -> Option<String> {
    let note = notes.iter().find(|n| n.dir == "repos" && n.name == repo)?;
    let mut out = format!(
        "Memória de trabalho (~/vault, contrato em ~/vault/CLAUDE.md). Nota do repo, {}:\n\n{}",
        note.key, note.text
    );
    for l in note.links() {
        if let Some(n) = find(notes, &l) {
            out.push_str(&format!("\n\n--- {} ---\n{}", n.key, n.text));
        }
    }
    Some(out)
}

/// A linha do Histórico de `repos/<repo>.md`, no formato que já está lá.
pub fn history_line(date: &str, iid: u64, branch: &str, title: &str, open: bool) -> String {
    format!(
        "- {date}: MR !{iid} `{branch}` — {}{}",
        title.trim().trim_end_matches('.'),
        if open { " (aberto)" } else { "" }
    )
}

/// Põe (ou atualiza) a linha do MR na seção Histórico; `None` se já estava assim.
pub fn with_history(text: &str, iid: u64, line: &str) -> Option<String> {
    let tag = format!("MR !{iid} `");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    if let Some(i) = lines.iter().position(|l| l.contains(&tag)) {
        // aberto → mergeado: tira só o "(aberto)", a data e o texto continuam os do dia em que abriu
        let merged = !line.ends_with("(aberto)");
        if merged && lines[i].ends_with(" (aberto)") {
            lines[i] = lines[i].trim_end_matches(" (aberto)").to_string();
        } else {
            return None;
        }
    } else {
        match lines.iter().position(|l| l.trim() == "## Histórico") {
            Some(h) => {
                let end = lines[h + 1..]
                    .iter()
                    .position(|l| l.starts_with("## "))
                    .map_or(lines.len(), |e| h + 1 + e);
                let mut at = end;
                while at > h + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, line.to_string());
            }
            None => {
                lines.push(String::new());
                lines.push("## Histórico".into());
                lines.push(String::new());
                lines.push(line.to_string());
            }
        }
    }
    Some(lines.join("\n") + "\n")
}

/// `- [ ] … branch …` → `- [x]` na daily: só a tarefa que fala desta feature (branch, `!iid` ou chave do Jira).
pub fn with_done(text: &str, keys: &[String]) -> Option<(String, String)> {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let i = lines.iter().position(|l| {
        l.trim_start().starts_with("- [ ]") && keys.iter().any(|k| !k.is_empty() && l.contains(k))
    })?;
    lines[i] = lines[i].replacen("- [ ]", "- [x]", 1);
    let task = lines[i]
        .trim_start()
        .trim_start_matches("- [x]")
        .trim()
        .to_string();
    Some((lines.join("\n") + "\n", task))
}

/// Uma escrita do Maestro no vault, com o que havia antes (o desfazer da Atividade).
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    pub at: SystemTime,
    pub desc: String,
    pub path: PathBuf,
    pub before: Option<String>,
}

/// O que o próprio Maestro acabou de gravar: o observador não conta como "fora do Maestro".
static OURS: std::sync::Mutex<Vec<(PathBuf, Option<String>)>> = std::sync::Mutex::new(Vec::new());

fn ours(path: &Path, text: Option<&str>) {
    if let Ok(mut o) = OURS.lock() {
        o.push((path.to_path_buf(), text.map(String::from)));
    }
}

pub fn write(rel: &str, text: &str, desc: String) -> Result<Write, String> {
    let path = dir().join(rel);
    let before = std::fs::read_to_string(&path).ok();
    ours(&path, Some(text));
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    log(&desc);
    Ok(Write {
        at: SystemTime::now(),
        desc,
        path,
        before,
    })
}

/// O salvar automático das Notas: sem linha na Atividade a cada pausa na digitação.
pub fn save(rel: &str, text: &str) -> Result<(), String> {
    let path = dir().join(rel);
    ours(&path, Some(text));
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// Uma edição que o salvar automático já gravou: só a linha na Atividade, com o texto de antes.
pub fn edited(rel: &str, before: Option<String>, desc: String) -> Write {
    log(&desc);
    Write {
        at: SystemTime::now(),
        desc,
        path: dir().join(rel),
        before,
    }
}

pub fn undo(w: &Write) -> Result<(), String> {
    ours(&w.path, w.before.as_deref());
    let r = match &w.before {
        Some(t) => std::fs::write(&w.path, t),
        None => std::fs::remove_file(&w.path),
    };
    log(&format!("desfeito: {}", w.desc));
    r.map_err(|e| e.to_string())
}

/// Um log por vault: o de uma cópia de teste (`MAESTRO_VAULT`) nunca cai na Atividade do vault real.
fn log_file() -> PathBuf {
    let name = match std::env::var_os("MAESTRO_VAULT") {
        Some(v) => format!(
            "vault-activity-{}.log",
            crate::feature::slug(&v.to_string_lossy())
        ),
        None => "vault-activity.log".into(),
    };
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".local/share/maestro")
        .join(name)
}

fn log(desc: &str) {
    use std::io::Write;
    if cfg!(test) {
        return;
    }
    let t = std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file())
    {
        let _ = writeln!(f, "{t}\t{desc}");
    }
}

/// Notas criadas, mudadas ou apagadas por outro (o Claude seguindo o contrato, você no Obsidian):
/// cada uma vai para a Atividade com o texto de antes, para o Desfazer.
pub fn external(old: &[Note], new: &[Note]) -> Vec<Write> {
    let root = dir();
    let mut taken = OURS
        .lock()
        .map(|mut o| std::mem::take(&mut *o))
        .unwrap_or_default();
    let mut mine = |path: &Path, text: Option<&str>| {
        let i = taken
            .iter()
            .position(|(p, t)| p == path && t.as_deref() == text);
        i.map(|i| taken.remove(i)).is_some()
    };
    let mut out = Vec::new();
    for n in new {
        let before = old.iter().find(|o| o.key == n.key);
        if before.is_some_and(|b| b.text == n.text) {
            continue;
        }
        let path = root.join(&n.key);
        if mine(&path, Some(&n.text)) {
            continue;
        }
        let what = if before.is_some() {
            "alterada"
        } else {
            "criada"
        };
        out.push(Write {
            at: SystemTime::now(),
            desc: format!("{} {what} (fora do Maestro)", n.key),
            path,
            before: before.map(|b| b.text.clone()),
        });
    }
    for o in old.iter().filter(|o| !new.iter().any(|n| n.key == o.key)) {
        let path = root.join(&o.key);
        if mine(&path, None) {
            continue;
        }
        out.push(Write {
            at: SystemTime::now(),
            desc: format!("{} apagada (fora do Maestro)", o.key),
            path,
            before: Some(o.text.clone()),
        });
    }
    // o que não casou ainda pode estar a caminho do disco: volta para a próxima leitura
    if let Ok(mut o) = OURS.lock() {
        o.extend(taken);
    }
    for w in &out {
        log(&w.desc);
    }
    out
}

/// A Atividade do vault: (quando, o quê), da mais nova para a mais antiga.
pub fn activity() -> Vec<(String, String)> {
    std::fs::read_to_string(log_file())
        .unwrap_or_default()
        .lines()
        .rev()
        .filter_map(|l| l.split_once('\t'))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// MR aberto ou mergeado → Histórico de `repos/<repo>.md` (criada do template se faltar) e o `[x]` na daily.
pub fn on_mr(
    repo: &str,
    iid: u64,
    branch: &str,
    title: &str,
    merged: bool,
) -> Vec<Result<Write, String>> {
    let date = today();
    let rel = format!("repos/{repo}.md");
    let text = std::fs::read_to_string(dir().join(&rel)).unwrap_or_else(|_| {
        std::fs::read_to_string(dir().join("templates/repo.md"))
            .unwrap_or_else(|_| "# {{title}}\n\n## Histórico\n".into())
            .replace("{{title}}", repo)
    });
    let mut out = Vec::new();
    let line = history_line(&date, iid, branch, title, !merged);
    if let Some(t) = with_history(&text, iid, &line) {
        let what = if merged { "mergeado" } else { "aberto" };
        out.push(write(&rel, &t, format!("{rel} · Histórico: !{iid} {what}")));
    }
    if merged {
        let daily = format!("daily/{date}.md");
        let key = jira_key(branch);
        let keys = [branch.to_string(), format!("!{iid}"), key];
        if let Some((t, task)) = std::fs::read_to_string(dir().join(&daily))
            .ok()
            .and_then(|d| with_done(&d, &keys))
        {
            out.push(write(&daily, &t, format!("{daily} · [x] {task}")));
        }
    }
    out
}

/// `fix/PRD-171448-reroute` → `PRD-171448`
fn jira_key(branch: &str) -> String {
    branch
        .split(['/', '-', '_'])
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| {
            w[0].len() >= 2
                && w[0].chars().all(|c| c.is_ascii_uppercase())
                && !w[1].is_empty()
                && w[1].chars().all(|c| c.is_ascii_digit())
        })
        .map(|w| format!("{}-{}", w[0], w[1]))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{history_line, jira_key, links, with_done, with_history};

    #[test]
    fn links_e_historico() {
        assert_eq!(
            links("a [[x]] b [[y|apelido]] [[x]] [[z#seção]] [[ ]]"),
            vec!["x", "y", "z"]
        );
        let note =
            "# bots-api\n\n## Histórico\n\n- 2026-10-02: MR !143 `fix/a` — antes.\n\n## Outra\n";
        let open = history_line("2026-10-15", 150, "feat/b", "nova rota.", true);
        assert_eq!(open, "- 2026-10-15: MR !150 `feat/b` — nova rota (aberto)");
        let t = with_history(note, 150, &open).unwrap();
        assert!(
            t.contains("antes.\n- 2026-10-15: MR !150 `feat/b` — nova rota (aberto)\n\n## Outra")
        );
        assert_eq!(with_history(&t, 150, &open), None);
        let merged = history_line("2026-10-20", 150, "feat/b", "nova rota", false);
        let t2 = with_history(&t, 150, &merged).unwrap();
        assert!(t2.contains("- 2026-10-15: MR !150 `feat/b` — nova rota\n"));
        assert_eq!(with_history(&t2, 150, &merged), None);
        let sem = with_history("# x\n", 1, "- l").unwrap();
        assert!(sem.ends_with("## Histórico\n\n- l\n"));
    }

    #[test]
    fn mudanca_fora_do_maestro() {
        use super::{Note, dir, external, ours};
        let n = |k: &str, t: &str| Note {
            key: k.into(),
            dir: "decisions".into(),
            name: k.into(),
            text: t.into(),
        };
        let old = vec![n("decisions/a.md", "1"), n("decisions/b.md", "x")];
        let new = vec![n("decisions/a.md", "2"), n("decisions/c.md", "novo")];
        ours(&dir().join("decisions/a.md"), Some("2"));
        let w = external(&old, &new);
        let d: Vec<&str> = w.iter().map(|w| w.desc.as_str()).collect();
        assert_eq!(
            d,
            vec![
                "decisions/c.md criada (fora do Maestro)",
                "decisions/b.md apagada (fora do Maestro)"
            ]
        );
        assert_eq!(w[1].before.as_deref(), Some("x"));
    }

    #[test]
    fn daily_marca_so_a_tarefa_da_feature() {
        let d = "# d\n\n## Fila\n- [ ] outra coisa\n- [ ] revisar PRD-171448 no bots\n";
        let keys = vec![
            "fix/PRD-171448-x".to_string(),
            "!146".into(),
            jira_key("fix/PRD-171448-x"),
        ];
        let (t, task) = with_done(d, &keys).unwrap();
        assert!(t.contains("- [ ] outra coisa\n- [x] revisar PRD-171448"));
        assert_eq!(task, "revisar PRD-171448 no bots");
        assert_eq!(with_done("- [ ] nada a ver\n", &keys), None);
        assert_eq!(jira_key("feat/x"), "");
    }
}
