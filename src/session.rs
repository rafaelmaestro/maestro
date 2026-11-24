use std::{
    collections::{BTreeSet, HashMap},
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use serde_json::Value;

/// Sub-task proposta pelo plano (o bloco ```subtasks do ExitPlanMode).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Proposal {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub sp: f64,
}

/// Custo e contexto de uma sessão do Claude Code, lidos do JSONL que ele mesmo grava.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub cost: f64,
    pub today: f64,
    pub ctx: u8,
}

#[derive(Default)]
struct Tracked {
    offset: u64,
    cost: f64,
    cost_before_today: f64,
    last_day: String,
    tokens: u64,
    one_million: bool,
    /// arquivos que o Claude editou (Edit/Write/MultiEdit/NotebookEdit), caminho absoluto
    edited: BTreeSet<String>,
    /// pastas em que um Bash escreveu (`cd outro-repo && sed -i …`, `git commit`, `glab mr create`)
    bash_dirs: BTreeSet<String>,
    /// o último plano com sub-tasks e quantos já vieram
    plan: Option<(u64, Vec<Proposal>)>,
}

/// Lê só o que foi acrescentado desde a última vez em cada arquivo.
#[derive(Default)]
pub struct Sessions {
    files: HashMap<PathBuf, Tracked>,
    by_pane: HashMap<String, PathBuf>,
    /// pasta → raiz do repo git dela (None: fora de repo)
    roots: HashMap<PathBuf, Option<PathBuf>>,
}

/// `~/.claude/projects/<cwd com tudo que não é letra ou dígito trocado por ->`
pub fn project_dir(home: &Path, cwd: &str) -> PathBuf {
    let name: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(name)
}

/// A sessão do pane: o JSONL do cwd cujo título (`agentName`/`aiTitle`) é o título do terminal;
/// sem título igual, a mais recente do cwd.
pub fn transcript(home: &Path, cwd: &str, title: &str) -> Option<PathBuf> {
    let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(project_dir(home, cwd))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by_key(|a| std::cmp::Reverse(a.0));
    let needles = [
        format!("\"agentName\":\"{title}\""),
        format!("\"aiTitle\":\"{title}\""),
    ];
    files
        .iter()
        .find(|(_, p)| {
            std::fs::read_to_string(p).is_ok_and(|t| needles.iter().any(|n| t.contains(n)))
        })
        .or(files.first())
        .map(|(_, p)| p.clone())
}

impl Sessions {
    fn find(&mut self, home: &Path, pane: &str, cwd: &str, title: &str) -> Option<PathBuf> {
        if let Some(p) = self.by_pane.get(pane) {
            return Some(p.clone());
        }
        let found = transcript(home, cwd, title)?;
        self.by_pane.insert(pane.into(), found.clone());
        Some(found)
    }

    /// Os repos em que o Claude do pane editou arquivo, na ordem do caminho (lido no último `usage`).
    pub fn repos(&mut self, pane: &str) -> Vec<PathBuf> {
        let Some(t) = self.by_pane.get(pane).and_then(|p| self.files.get(p)) else {
            return Vec::new();
        };
        let dirs: BTreeSet<PathBuf> = t
            .edited
            .iter()
            .filter_map(|f| Path::new(f).parent().map(Path::to_path_buf))
            .chain(t.bash_dirs.iter().map(PathBuf::from))
            .collect();
        let mut out: Vec<PathBuf> = Vec::new();
        for d in dirs {
            let root = self.roots.entry(d.clone()).or_insert_with(|| git_root(&d));
            if let Some(r) = root
                && !out.contains(r)
            {
                out.push(r.clone());
            }
        }
        out
    }

    pub fn proposals(&self, pane: &str) -> Option<(u64, Vec<Proposal>)> {
        self.by_pane
            .get(pane)
            .and_then(|p| self.files.get(p))?
            .plan
            .clone()
    }

    pub fn usage(&mut self, home: &Path, pane: &str, cwd: &str, title: &str) -> Option<Usage> {
        let path = self.find(home, pane, cwd, title)?;
        let today = today();
        let t = self.files.entry(path.clone()).or_default();
        let mut file = File::open(&path).ok()?;
        file.seek(SeekFrom::Start(t.offset)).ok()?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        // só linhas completas: o Claude pode estar no meio de uma escrita
        while reader.read_line(&mut line).ok()? > 0 && line.ends_with('\n') {
            t.offset += line.len() as u64;
            read_line(t, &line);
            line.clear();
        }
        let window = if t.one_million || t.tokens > 200_000 {
            1_000_000
        } else {
            200_000
        };
        let today_cost = if t.last_day == today {
            t.cost - t.cost_before_today
        } else {
            0.
        };
        Some(Usage {
            cost: t.cost,
            today: today_cost.max(0.),
            ctx: (t.tokens * 100 / window).min(100) as u8,
        })
    }
}

fn git_root(dir: &Path) -> Option<PathBuf> {
    // a pasta pode ter sido apagada depois: sobe até achar uma que existe
    let dir = dir.ancestors().find(|d| d.is_dir())?;
    let o = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

/// As pastas em que um comando Bash escreveu: segue os `cd` e marca o trecho que muda arquivo ou o git.
/// ponytail: heurística por palavra-chave; um `>` de comparação num script pode marcar a pasta atual a mais
fn bash_writes(cmd: &str, home: &str) -> Vec<String> {
    let resolve = |d: &str, cur: Option<&str>| -> Option<String> {
        let d = d.trim_matches(|c| c == '"' || c == '\'');
        let d = match d.strip_prefix('~').or_else(|| d.strip_prefix("$HOME")) {
            Some(rest) => format!("{home}{rest}"),
            None => d.to_string(),
        };
        if d.starts_with('/') {
            Some(d)
        } else {
            cur.map(|c| format!("{c}/{d}"))
        }
    };
    let mut cur: Option<String> = None;
    let mut out: Vec<String> = Vec::new();
    for seg in cmd
        .split(['\n', ';'])
        .flat_map(|s| s.split("&&"))
        .flat_map(|s| s.split("||"))
    {
        let seg = seg.trim().trim_start_matches(['(', '{']).trim();
        let w: Vec<&str> = seg.split_whitespace().collect();
        if w.first() == Some(&"cd") {
            if let Some(d) = w.get(1) {
                cur = resolve(d, cur.as_deref());
            }
            continue;
        }
        let dir = match w.as_slice() {
            ["git", "-C", d, ..] => resolve(d, cur.as_deref()),
            _ => cur.clone(),
        };
        if let Some(d) = dir.filter(|_| writes(seg))
            && !out.contains(&d)
        {
            out.push(d);
        }
    }
    out
}

fn writes(seg: &str) -> bool {
    const W: &[&str] = &[
        "sed -i",
        "tee ",
        "cp ",
        "mv ",
        "rm ",
        "mkdir ",
        "git add",
        "git commit",
        "git checkout -b",
        "git switch -c",
        "git push",
        "git apply",
        "git cherry-pick",
        "git merge",
        "git rebase",
        "git reset",
        "git restore",
        "git stash",
        "glab mr create",
        "--write",
        "--fix",
        "pnpm install",
        "pnpm add",
        "npm install",
        "npm i ",
        "yarn add",
        "yarn install",
    ];
    let padded = format!(" {seg} ");
    let git_c = seg.starts_with("git -C ");
    W.iter().any(|w| {
        padded.contains(&format!(" {w}"))
            || git_c && w.starts_with("git ") && padded.contains(&format!(" {}", &w[4..]))
    }) || seg.contains("open(") && (seg.contains("'w'") || seg.contains("\"w\""))
        || seg.match_indices('>').any(|(i, _)| {
            let before = seg[..i].chars().last();
            let rest = seg[i + 1..].trim_start_matches('>').trim_start();
            !matches!(before, Some('=' | '-' | '>'))
                && !rest.is_empty()
                && !rest.starts_with('&')
                && !rest.starts_with("/dev/")
        })
}

fn read_line(t: &mut Tracked, line: &str) {
    let cost = line.contains("\"type\":\"cost-state\"");
    let assistant = line.contains("\"type\":\"assistant\"");
    if let Some(i) = line.find("\"timestamp\":\"") {
        let day = &line[i + 13..(i + 23).min(line.len())];
        if day != t.last_day {
            t.cost_before_today = t.cost;
            t.last_day = day.into();
        }
    }
    if !cost && !assistant {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if cost {
        t.cost = v["totalCostUSD"].as_f64().unwrap_or(t.cost);
        t.one_million = v["modelUsage"]
            .as_object()
            .is_some_and(|m| m.keys().any(|k| k.ends_with("[1m]")));
    } else {
        let u = &v["message"]["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let tokens =
            n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        if tokens > 0 {
            t.tokens = tokens;
        }
        for c in v["message"]["content"].as_array().into_iter().flatten() {
            let edit = matches!(
                c["name"].as_str(),
                Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
            );
            let path = c["input"]["file_path"]
                .as_str()
                .or(c["input"]["notebook_path"].as_str());
            if let (true, Some(p)) = (c["type"] == "tool_use" && edit, path) {
                t.edited.insert(p.into());
            }
            if c["type"] == "tool_use"
                && c["name"] == "Bash"
                && let Some(cmd) = c["input"]["command"].as_str()
            {
                let home = std::env::var("HOME").unwrap_or_default();
                t.bash_dirs.extend(bash_writes(cmd, &home));
            }
            // o plano vem no ExitPlanMode; o "Pedir ao Claude para propor" pode voltar em texto
            let plan = match c["name"].as_str() {
                Some("ExitPlanMode") => c["input"]["plan"].as_str(),
                _ => c["text"].as_str().filter(|_| c["type"] == "text"),
            };
            if let Some(subs) = plan.and_then(subtasks) {
                let n = t.plan.as_ref().map_or(0, |p| p.0) + 1;
                t.plan = Some((n, subs));
            }
        }
    }
}

/// O último bloco ```subtasks do plano.
fn subtasks(plan: &str) -> Option<Vec<Proposal>> {
    let start = plan.rfind("```subtasks")? + "```subtasks".len();
    let body = &plan[start..];
    let body = &body[..body.find("```").unwrap_or(body.len())];
    serde_json::from_str(body.trim()).ok()
}

/// Data UTC de hoje como `AAAA-MM-DD` (os timestamps do JSONL são UTC).
fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    civil(days)
}

/// Dias desde 1970-01-01 → data (algoritmo de Howard Hinnant).
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_que_escreve_em_outro_repo() {
        let h = "/h";
        let w = |c: &str| bash_writes(c, h);
        assert_eq!(
            w("cd ~/r/zendesk && sed -i 's/a/b/' package.json && grep -n x package.json"),
            ["/h/r/zendesk"]
        );
        assert_eq!(
            w(
                "cd ~/r/zendesk && mkdir -p src/x && cat > src/x/a.ts <<'EOF'\nconst f = () => 1;\nEOF"
            ),
            ["/h/r/zendesk"]
        );
        assert_eq!(
            w("set -e\ncd ~/r/roleset && git checkout -q -b feat/x && git commit -q -m y"),
            ["/h/r/roleset"]
        );
        assert_eq!(
            w("cd ~/r/roleset && glab mr create --draft --title t"),
            ["/h/r/roleset"]
        );
        assert!(w("cd ~/r/kong && grep -rln zendesk . | head; sed -n 1,5p a.yaml").is_empty());
        assert!(w("cd ~/r/pipe && git status -sb | head -2 && cat a.yml 2>/dev/null").is_empty());
        assert!(w("cd ~/r/z 2>/dev/null || { ls ~/r | grep -i z; exit; }").is_empty());
        assert!(w("sed -i s/a/b/ relativo-sem-cd.txt").is_empty());
        assert_eq!(w("cd /a && cd b && echo x > f"), ["/a/b"]);
        assert_eq!(w("git -C /a/b commit -m x"), ["/a/b"]);
        assert_eq!(
            w("cd ~/r/z && python3 - <<'EOF'\nopen(p,'w').write(s)\nEOF"),
            ["/h/r/z"]
        );
        assert!(w("cd /a && git log -1 --format=%h 2>&1").is_empty());
    }

    #[test]
    fn datas() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_741), "2026-10-15");
    }

    #[test]
    fn le_custo_contexto_e_o_de_hoje() {
        let home = std::env::temp_dir().join(format!("maestro-sess-{}", std::process::id()));
        let dir = project_dir(&home, "/repo/x.y");
        std::fs::create_dir_all(&dir).unwrap();
        let today = today();
        let lines = [
            r#"{"type":"agent-name","agentName":"plano"}"#.to_string(),
            r#"{"type":"user","timestamp":"2020-01-01T10:00:00Z"}"#.into(),
            r#"{"type":"cost-state","totalCostUSD":10.0,"modelUsage":{"claude-x":{}}}"#.into(),
            format!(r#"{{"type":"user","timestamp":"{today}T10:00:00Z"}}"#),
            r#"{"type":"assistant","message":{"usage":{"input_tokens":2,"cache_read_input_tokens":299998,"cache_creation_input_tokens":0}}}"#.into(),
            r#"{"type":"cost-state","totalCostUSD":12.5,"modelUsage":{"claude-x[1m]":{}}}"#.into(),
        ];
        let (a, b) = (home.join("a"), home.join("b/src"));
        for r in [&a, &home.join("b")] {
            std::fs::create_dir_all(r).unwrap();
            std::process::Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(r)
                .status()
                .unwrap();
        }
        std::fs::create_dir_all(&b).unwrap();
        let edit = |tool: &str, p: &Path| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{tool}","input":{{"file_path":"{}"}}}}]}}}}"#,
                p.display()
            )
        };
        let mut lines = lines.to_vec();
        lines.push(edit("Edit", &b.join("x.ts")));
        lines.push(edit("Write", &a.join("novo.ts")));
        lines.push(edit("Read", &home.join("fora/y.ts")));
        lines.push(edit("Edit", &b.join("z.ts")));
        let plan = |p: &str| {
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"ExitPlanMode","input":{"plan":p}}]}})
                .to_string()
        };
        lines.push(plan("só o plano, sem sub-tasks"));
        lines.push(plan(
            "velho\n```subtasks\n[{\"type\":\"Subtarefa\",\"title\":\"[x] y\",\"sp\":1}]\n```",
        ));
        lines.push(
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"```subtasks\n[]\n```"}]}})
                .to_string(),
        );
        lines.push(plan(
            "Plano\n```subtasks\n[{\"type\":\"Desenvolvimento\",\"title\":\"[bots-api] paginar\",\"sp\":2},\
             {\"type\":\"Qualidade e Testes\",\"title\":\"e2e\",\"sp\":1}]\n```\nfim",
        ));
        std::fs::write(dir.join("a.jsonl"), lines.join("\n") + "\n").unwrap();
        let mut s = Sessions::default();
        let u = s.usage(&home, "p1", "/repo/x.y", "plano").unwrap();
        let (n, subs) = s.proposals("p1").unwrap();
        assert_eq!(n, 3);
        assert_eq!(
            subs,
            vec![
                Proposal {
                    kind: "Desenvolvimento".into(),
                    title: "[bots-api] paginar".into(),
                    sp: 2.
                },
                Proposal {
                    kind: "Qualidade e Testes".into(),
                    title: "e2e".into(),
                    sp: 1.
                }
            ]
        );
        let canon = |p: PathBuf| p.canonicalize().unwrap();
        assert_eq!(
            s.repos("p1").into_iter().map(canon).collect::<Vec<_>>(),
            vec![canon(a.clone()), canon(home.join("b"))]
        );
        assert_eq!(
            u,
            Usage {
                cost: 12.5,
                today: 2.5,
                ctx: 30
            }
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
