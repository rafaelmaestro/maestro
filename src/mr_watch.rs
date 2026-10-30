//! O vigia dos seus MRs: pipe que falha, thread nova e a revisão depois do verde vão ao Claude da feature.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    mr::{self, Status},
    mrs::{Finding, Row},
};

/// Falhas seguidas do mesmo job antes de parar de mandar ao Claude.
const TRIES: u32 = 3;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// a primeira leitura só grava a base: o que já existia não dispara
    init: bool,
    mrs: BTreeMap<String, Watch>,
    /// MR fora de feature → cwd do Claude solto que o abriu
    #[serde(default)]
    pub panes: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    /// o último pipeline já tratado (mandado ao Claude ou desistido)
    pipe: u64,
    /// job → vezes que foi mandado ao Claude sem o pipe ficar verde
    tries: BTreeMap<String, u32>,
    pub gave_up: bool,
    /// maior id de nota já mandada ao Claude
    last_note: u64,
    pub reviewed: bool,
    /// threads já mandadas ao agente (ids das discussões)
    pub threads: std::collections::BTreeSet<String>,
    pub found: Option<usize>,
    /// o que o vigia fez, para a aba MR (os últimos)
    #[serde(default)]
    pub log: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub id: String,
    /// id da primeira nota: o mesmo do comentário da aba MR
    pub note: u64,
    pub at: String,
    /// (quem, texto) da thread inteira, para o agente ter o contexto
    pub notes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    Pipe {
        pipeline: u64,
        job: String,
        url: String,
    },
    GaveUp {
        pipeline: u64,
        job: String,
    },
    Threads {
        last: u64,
        list: Vec<Thread>,
    },
    Review,
    Findings(Vec<Finding>),
}

fn file() -> PathBuf {
    std::env::var_os("MAESTRO_MR_WATCH").map_or_else(
        || {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share/maestro/mr-watch.json")
        },
        PathBuf::from,
    )
}

fn author(n: &Value) -> &str {
    n["author"]["username"].as_str().unwrap_or_default()
}

impl State {
    pub fn load() -> Self {
        std::fs::read_to_string(file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let f = file();
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, serde_json::to_string(self).unwrap_or_default());
    }

    pub fn get(&self, key: &str) -> Option<&Watch> {
        self.mrs.get(key)
    }

    /// Fim de uma volta do polling: daqui em diante, MR novo entra para ser vigiado do zero.
    pub fn ready(&mut self) {
        self.init = true;
    }

    /// O que mandar ao Claude agora. Nada muda além da base do MR visto pela primeira vez:
    /// o que foi mandado só vira "tratado" no `commit`, depois que o Claude recebeu.
    pub fn events(
        &mut self,
        key: &str,
        detail: &Value,
        jobs: &Value,
        discussions: &Value,
        me: &str,
    ) -> Vec<Ev> {
        let ci = crate::mrs::ci_of(detail);
        let pipeline = detail["head_pipeline"]["id"].as_u64().unwrap_or_default();
        let open: Vec<&Value> = discussions
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                let n = &d["notes"][0];
                let bot_note = mr::is_bot(author(n)) && n["resolvable"] != true;
                n["system"] != true
                    && !bot_note
                    && (n["resolvable"] != true || n["resolved"] != true)
            })
            .collect();
        let max_note = open
            .iter()
            .flat_map(|d| d["notes"].as_array().into_iter().flatten())
            .filter_map(|n| n["id"].as_u64())
            .max()
            .unwrap_or_default();
        let init = self.init;
        let w = self.mrs.entry(key.to_string()).or_insert_with(|| {
            if init {
                Watch::default()
            } else {
                Watch {
                    pipe: pipeline,
                    last_note: max_note,
                    reviewed: true,
                    ..Watch::default()
                }
            }
        });
        if ci == Status::Ok {
            w.tries.clear();
            w.gave_up = false;
        }
        let mut out = Vec::new();
        if ci == Status::Fail
            && pipeline != w.pipe
            && let Some((job, url)) = mr::failed_job(jobs)
        {
            out.push(if w.tries.get(&job).copied().unwrap_or_default() < TRIES {
                Ev::Pipe { pipeline, job, url }
            } else {
                Ev::GaveUp { pipeline, job }
            });
        }
        let list: Vec<Thread> =
            open.iter()
                .filter(|d| {
                    d["notes"].as_array().into_iter().flatten().any(|n| {
                        n["id"].as_u64().is_some_and(|id| id > w.last_note) && author(n) != me
                    })
                })
                .map(|d| {
                    let n = &d["notes"][0];
                    Thread {
                        id: d["id"].as_str().unwrap_or_default().into(),
                        note: n["id"].as_u64().unwrap_or_default(),
                        at: match n["position"]["new_path"].as_str() {
                            Some(p) => match n["position"]["new_line"].as_u64() {
                                Some(l) => format!("{p}:{l}"),
                                None => p.into(),
                            },
                            None => "geral".into(),
                        },
                        notes: d["notes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|n| {
                                (
                                    format!("@{}", author(n)),
                                    n["body"].as_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect(),
                    }
                })
                .collect();
        if !list.is_empty() {
            out.push(Ev::Threads {
                last: max_note,
                list,
            });
        }
        if ci == Status::Ok && !w.reviewed {
            out.push(Ev::Review);
        }
        out
    }

    /// O Claude recebeu (ou o Maestro desistiu): não manda de novo.
    pub fn commit(&mut self, key: &str, ev: &Ev) {
        let w = self.mrs.entry(key.to_string()).or_default();
        let line = match ev {
            Ev::Pipe { pipeline, job, .. } => {
                w.pipe = *pipeline;
                *w.tries.entry(job.clone()).or_default() += 1;
                format!("pipe falhou em {job} → enviado ao agente")
            }
            Ev::GaveUp { pipeline, job } => {
                w.pipe = *pipeline;
                w.gave_up = true;
                format!("{job} falhou {TRIES} vezes seguidas → parei de mandar")
            }
            Ev::Threads { last, list } => {
                w.last_note = w.last_note.max(*last);
                w.threads.extend(list.iter().map(|t| t.id.clone()));
                let who: Vec<&str> = list
                    .iter()
                    .filter_map(|t| t.notes.last().map(|n| n.0.as_str()))
                    .collect();
                format!("comentário novo de {} → enviado ao agente", who.join(", "))
            }
            Ev::Review => {
                w.reviewed = true;
                "pipe verde → revisor rodando no MR".into()
            }
            Ev::Findings(f) => {
                w.found = Some(f.len());
                if f.is_empty() {
                    "revisor: nenhum achado".into()
                } else {
                    format!("revisor: {} achado(s) → enviados ao agente", f.len())
                }
            }
        };
        w.log.push(line);
        let n = w.log.len().saturating_sub(6);
        w.log.drain(..n);
    }

    /// `https://gitlab.com/g/p/-/merge_requests/7` → `g/p!7`, a chave do estado.
    pub fn key_of(web_url: &str, iid: u64) -> Option<String> {
        let path = web_url
            .split_once("://")?
            .1
            .split_once('/')?
            .1
            .split_once("/-/merge_requests/")?
            .0;
        Some(format!("{path}!{iid}"))
    }

    /// MR que saiu da lista de abertos (mergeado ou fechado) não precisa mais de estado.
    pub fn keep(&mut self, open: &[String]) {
        self.mrs.retain(|k, _| open.contains(k));
        self.panes.retain(|k, _| open.contains(k));
    }
}

/// Detalhe, jobs (só com o pipe vermelho) e discussões do MR. Bloqueia: chamar fora da thread da UI.
pub fn fetch(row: &Row) -> Result<(Value, Value, Value), String> {
    let base = format!(
        "projects/{}/merge_requests/{}",
        crate::mrs::enc(&row.path),
        row.iid
    );
    let detail = crate::mrs::read(&base, false)?;
    let jobs = match detail["head_pipeline"]["id"].as_u64() {
        Some(p) if crate::mrs::ci_of(&detail) == Status::Fail => crate::mrs::read(
            &format!(
                "projects/{}/pipelines/{p}/jobs?per_page=100",
                crate::mrs::enc(&row.path)
            ),
            false,
        )?,
        _ => Value::Null,
    };
    let discussions = crate::mrs::read(&format!("{base}/discussions?per_page=100"), true)?;
    Ok((detail, jobs, discussions))
}

/// O revisor de MR (o mesmo da gaveta "Revisão Claude") rodando no seu próprio MR. Bloqueia por minutos.
/// Tira o MR do draft (o `Draft:` do título), para os revisores de IA rodarem e o time ser avisado.
pub fn undraft(row: &Row) -> Result<bool, String> {
    let title = undrafted(&row.title);
    if title == row.title {
        return Ok(false);
    }
    let at = format!(
        "projects/{}/merge_requests/{}",
        crate::mrs::enc(&row.path),
        row.iid
    );
    crate::mrs::write("PUT", &at, &serde_json::json!({ "title": title })).map(|_| true)
}

fn undrafted(title: &str) -> String {
    let t = title.trim_start();
    ["Draft:", "[Draft]", "(Draft)"]
        .iter()
        .find_map(|p| t.strip_prefix(p))
        .map_or_else(|| title.to_string(), |r| r.trim_start().to_string())
}

pub fn review(row: &Row) -> Result<(Vec<Finding>, f64), String> {
    let base = format!(
        "projects/{}/merge_requests/{}",
        crate::mrs::enc(&row.path),
        row.iid
    );
    let detail = crate::mrs::read(&base, false)?;
    let notes =
        crate::mrs::read(&format!("{base}/notes?per_page=100"), true).unwrap_or(Value::Null);
    let out = std::sync::Mutex::new(Err("o Claude parou sem devolver os achados".to_string()));
    crate::mrs::review(
        &row.path,
        row.iid,
        &row.title,
        &row.target,
        detail["description"].as_str().unwrap_or_default(),
        &notes,
        None,
        |m| match m {
            crate::mrs::Run::Done(f, c) => *out.lock().unwrap() = Ok((f, c)),
            crate::mrs::Run::Failed(e) => *out.lock().unwrap() = Err(e),
            _ => {}
        },
    );
    out.into_inner().unwrap()
}

/// O nome do agente de um MR: o mesmo a cada evento, para o Claude reaproveitar o que já está rodando.
pub fn agent(row: &Row) -> String {
    format!("mr-{}-{}", row.proj(), row.iid)
}

/// O texto que vai ao Claude da feature. `None`: não é com ele (revisão roda no Maestro, desistência só avisa).
pub fn prompt(row: &Row, ev: &Ev) -> Option<String> {
    let task = match ev {
        Ev::Pipe { job, url, .. } => format!(
            "O pipeline falhou no job {job} ({url}). Leia o log (glab api projects/:id/jobs/<id>/trace), ache a causa raiz e corrija. Se for infra/flaky, rode o job de novo (glab ci retry) em vez de mexer no código."
        ),
        Ev::Threads { list, .. } => {
            let threads: Vec<String> = list
                .iter()
                .map(|t| {
                    let notes: Vec<String> =
                        t.notes.iter().map(|(w, b)| format!("  {w}: {b}")).collect();
                    format!("THREAD_ID {} em {}\n{}", t.id, t.at, notes.join("\n"))
                })
                .collect();
            format!(
                "Threads com comentário novo. Para cada uma: se procede, corrija e responda o que mudou; se não procede, responda o porquê; se já está resolvido, diga onde. Responda na thread: glab api -X POST \"projects/{}/merge_requests/{}/discussions/<THREAD_ID>/notes\" -f body=\"...\". Não resolva a thread.\n\n{}",
                crate::mrs::enc(&row.path),
                row.iid,
                threads.join("\n\n")
            )
        }
        Ev::Findings(f) => {
            let list: Vec<String> = f
                .iter()
                .map(|x| {
                    let at = x.line.map_or(x.file.clone(), |l| format!("{}:{l}", x.file));
                    format!("- [{}] {at}: {} {}", x.severity, x.label, x.body)
                })
                .collect();
            format!(
                "O revisor de MR do Maestro achou isto no próprio MR. Corrija o que procede; o que não procede, ignore e diga por quê na sua resposta final (não comente no MR).\n\n{}",
                list.join("\n")
            )
        }
        Ev::Review | Ev::GaveUp { .. } => return None,
    };
    Some(format!(
        "Vigia do MR !{iid} ({url}, {src} → {target}). Delegue a um subagente por MR: veja em ListAgents se já existe um `{name}`; se existir, mande a tarefa com SendMessage para ele; senão crie com Agent usando a description `{name}`. Agentes de MRs da mesma worktree trabalham um de cada vez; se mexer numa fatia de baixo (`<branch>-n`), rebaseie e re-pushe as de cima. Sem pedir confirmação: corrija, commite (Conventional Commits) e dê push para {src} (fatia sem branch local: `git push origin <sha>:refs/heads/{src}`, como no passo 4 do /mr). Tarefa:\n\n{task}",
        iid = row.iid,
        url = row.web_url,
        src = row.source,
        target = row.target,
        name = agent(row),
    ))
}

/// A worktree que é dona do MR: a da branch, ou a da feature fatiada pelo `/mr` (`<branch>-n`).
pub fn owns(branch: &str, source: &str) -> bool {
    source == branch
        || source
            .strip_prefix(branch)
            .and_then(|r| r.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// O Claude solto (fora de feature) que abriu cada MR: o transcript onde um `glab mr create` devolveu o link.
/// Lê `~/.claude/projects/<cwd>`, porque o Claude em tela cheia não deixa o link no scrollback.
pub fn openers(rows: &[Row], cwds: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let projects =
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".claude/projects");
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for cwd in cwds {
        let dir: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let files = std::fs::read_dir(projects.join(dir))
            .into_iter()
            .flatten()
            .flatten();
        for f in files.filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")) {
            let Ok(t) = std::fs::read_to_string(f.path()) else {
                continue;
            };
            for r in rows {
                if !out.iter().any(|(k, _)| *k == r.key()) && created(&t, &r.web_url) {
                    out.push((r.key(), cwd.clone()));
                }
            }
        }
    }
    out
}

/// O transcript tem um `glab mr create` cujo resultado traz o link (citar o link não basta: colar o MR noutra sessão não a faz dona).
fn created(transcript: &str, url: &str) -> bool {
    if !transcript.contains("mr create") || !cites(transcript, url) {
        return false;
    }
    let items = |l: &str| -> Vec<Value> {
        serde_json::from_str::<Value>(l)
            .ok()
            .and_then(|v| v["message"]["content"].as_array().cloned())
            .unwrap_or_default()
    };
    let calls: Vec<String> = transcript
        .lines()
        .filter(|l| l.contains("mr create"))
        .flat_map(items)
        .filter(|c| {
            c["input"]["command"]
                .as_str()
                .is_some_and(|c| c.contains("glab mr create"))
        })
        .filter_map(|c| c["id"].as_str().map(String::from))
        .collect();
    transcript
        .lines()
        .filter(|l| cites(l, url))
        .flat_map(items)
        .any(|c| {
            c["tool_use_id"]
                .as_str()
                .is_some_and(|id| calls.iter().any(|x| x == id))
                && cites(&c["content"].to_string(), url)
        })
}

fn cites(text: &str, url: &str) -> bool {
    text.match_indices(url)
        .any(|(i, _)| !text[i + url.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

/// (worktree do MR, worktree do Claude da feature) de cada MR seu que é de uma feature do Maestro.
pub fn owners(rows: &[Row]) -> Vec<(Row, PathBuf, PathBuf)> {
    let invs = crate::feature::inventories();
    let mut project: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
    let mut proj = |p: &Path| {
        project
            .entry(p.to_path_buf())
            .or_insert_with(|| crate::audit::project_of(p))
            .clone()
    };
    let mut out = Vec::new();
    for r in rows {
        let hit = invs.iter().find_map(|inv| {
            let w = inv.worktrees.iter().find(|w| {
                owns(&w.branch, &r.source) && proj(&w.repo).as_deref() == Some(r.path.as_str())
            })?;
            Some((w.path.clone(), inv.worktrees.first()?.path.clone()))
        });
        if let Some((root, owner)) = hit {
            out.push((r.clone(), root, owner));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn detail(status: &str, pipeline: u64) -> Value {
        json!({ "head_pipeline": { "id": pipeline, "status": status } })
    }

    fn jobs() -> Value {
        json!([{ "name": "test:unit", "status": "failed", "web_url": "https://gitlab.com/g/p/-/jobs/9" }])
    }

    fn disc(id: &str, notes: &[(u64, &str)]) -> Value {
        json!({ "id": id, "notes": notes.iter().map(|(i, w)| json!({
            "id": i, "author": { "username": w }, "body": "b", "resolvable": true, "resolved": false
        })).collect::<Vec<_>>() })
    }

    fn fresh() -> State {
        State {
            init: true,
            ..State::default()
        }
    }

    #[test]
    fn primeira_leitura_so_grava_a_base() {
        let mut s = State::default();
        let d = json!([disc("a", &[(5, "ana")])]);
        assert!(
            s.events("g/p!1", &detail("failed", 7), &jobs(), &d, "eu")
                .is_empty()
        );
        s.ready();
        assert!(
            s.events("g/p!1", &detail("failed", 7), &jobs(), &d, "eu")
                .is_empty()
        );
        assert!(
            s.events("g/p!1", &detail("success", 7), &jobs(), &d, "eu")
                .is_empty()
        );
    }

    #[test]
    fn nota_nova_de_outro_vai_uma_vez_e_a_minha_nao() {
        let mut s = fresh();
        let d = json!([disc("a", &[(5, "ana")]), disc("b", &[(6, "eu")])]);
        let ev = s.events("g/p!1", &detail("running", 7), &Value::Null, &d, "eu");
        let [Ev::Threads { list, .. }] = ev.as_slice() else {
            panic!("{ev:?}")
        };
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
        s.commit("g/p!1", &ev[0]);
        assert!(
            s.events("g/p!1", &detail("running", 7), &Value::Null, &d, "eu")
                .is_empty()
        );
        // a resposta do Claude (como eu) não dispara; a réplica da ana dispara
        let d = json!([disc("a", &[(5, "ana"), (8, "eu")])]);
        assert!(
            s.events("g/p!1", &detail("running", 7), &Value::Null, &d, "eu")
                .is_empty()
        );
        let d = json!([disc("a", &[(5, "ana"), (8, "eu"), (9, "ana")])]);
        assert_eq!(
            s.events("g/p!1", &detail("running", 7), &Value::Null, &d, "eu")
                .len(),
            1
        );
    }

    #[test]
    fn thread_resolvida_nao_dispara() {
        let mut s = fresh();
        let d = json!([{ "id": "a", "notes": [{ "id": 5, "author": { "username": "ana" }, "resolvable": true, "resolved": true }] }]);
        assert!(
            s.events("g/p!1", &detail("running", 7), &Value::Null, &d, "eu")
                .is_empty()
        );
    }

    #[test]
    fn pipe_falho_vai_por_pipeline_novo_e_desiste_na_quarta() {
        let mut s = fresh();
        let none = json!([]);
        for p in 1..=TRIES as u64 {
            let ev = s.events("g/p!1", &detail("failed", p), &jobs(), &none, "eu");
            assert!(
                matches!(&ev[..], [Ev::Pipe { job, .. }] if job == "test:unit"),
                "{ev:?}"
            );
            s.commit("g/p!1", &ev[0]);
            assert!(
                s.events("g/p!1", &detail("failed", p), &jobs(), &none, "eu")
                    .is_empty()
            );
        }
        let ev = s.events("g/p!1", &detail("failed", 9), &jobs(), &none, "eu");
        assert!(matches!(&ev[..], [Ev::GaveUp { .. }]), "{ev:?}");
        s.commit("g/p!1", &ev[0]);
        assert!(s.get("g/p!1").unwrap().gave_up);
        // ficou verde: zera, e a revisão entra
        let ev = s.events("g/p!1", &detail("success", 10), &jobs(), &none, "eu");
        assert_eq!(ev, vec![Ev::Review]);
        assert!(!s.get("g/p!1").unwrap().gave_up);
    }

    #[test]
    fn revisao_so_depois_do_verde_e_uma_vez() {
        let mut s = fresh();
        let none = json!([]);
        assert!(
            s.events("g/p!1", &detail("running", 1), &Value::Null, &none, "eu")
                .is_empty()
        );
        let ev = s.events("g/p!1", &detail("success", 1), &Value::Null, &none, "eu");
        assert_eq!(ev, vec![Ev::Review]);
        s.commit("g/p!1", &Ev::Review);
        assert!(
            s.events("g/p!1", &detail("success", 2), &Value::Null, &none, "eu")
                .is_empty()
        );
    }

    #[test]
    fn chave_pelo_link_e_log_curto() {
        assert_eq!(
            State::key_of("https://gitlab.com/g/sub/p/-/merge_requests/7", 7).as_deref(),
            Some("g/sub/p!7")
        );
        let mut s = fresh();
        for p in 0..8 {
            s.commit(
                "k",
                &Ev::Pipe {
                    pipeline: p,
                    job: "j".into(),
                    url: String::new(),
                },
            );
        }
        assert_eq!(s.get("k").unwrap().log.len(), 6);
    }

    #[test]
    fn dono_e_quem_rodou_o_mr_create_nao_quem_citou() {
        let u = "https://gitlab.com/g/p/-/merge_requests/31";
        let call = json!({"message": {"content": [{"type": "tool_use", "id": "t1", "input": {"command": "git push && glab mr create --draft --yes"}}]}});
        let res = json!({"message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": format!("{u}\n")}]}});
        let cited = json!({"message": {"content": [{"type": "text", "text": format!("olha o {u} (glab mr create)")}]}});
        assert!(created(&format!("{call}\n{res}"), u));
        assert!(!created(&cited.to_string(), u));
        assert!(!created(&format!("{call}\n{cited}"), u));
    }

    #[test]
    fn tira_so_o_prefixo_de_draft() {
        assert_eq!(undrafted("Draft: feat/x"), "feat/x");
        assert_eq!(undrafted("[Draft] feat/x"), "feat/x");
        assert_eq!(undrafted("feat/x"), "feat/x");
        assert_eq!(undrafted("Drafts de nota"), "Drafts de nota");
    }

    #[test]
    fn link_do_mr_nao_casa_com_iid_maior() {
        let u = "https://gitlab.com/g/p/-/merge_requests/31";
        assert!(cites("abri https://gitlab.com/g/p/-/merge_requests/31)", u));
        assert!(!cites("https://gitlab.com/g/p/-/merge_requests/316", u));
    }

    #[test]
    fn dona_do_mr_inclui_as_fatias() {
        assert!(owns("feat/x", "feat/x"));
        assert!(owns("feat/x", "feat/x-1"));
        assert!(owns("feat/x", "feat/x-12"));
        assert!(!owns("feat/x", "feat/x-y"));
        assert!(!owns("feat/x", "feat/xy"));
        assert!(!owns("feat/x", "feat/x-"));
    }

    #[test]
    fn prompt_nomeia_o_agente_e_a_thread() {
        let row = Row {
            path: "g/bots-api".into(),
            iid: 7,
            title: "t".into(),
            who: String::new(),
            sha: String::new(),
            source: "feat/x-1".into(),
            target: "main".into(),
            web_url: "https://gitlab.com/g/bots-api/-/merge_requests/7".into(),
            ci: None,
            notes: 0,
            updated: String::new(),
        };
        let ev = Ev::Threads {
            last: 5,
            list: vec![Thread {
                id: "abc".into(),
                note: 5,
                at: "src/a.ts:3".into(),
                notes: vec![("@ana".into(), "e o env?".into())],
            }],
        };
        let p = prompt(&row, &ev).unwrap();
        assert!(p.contains("`mr-bots-api-7`"));
        assert!(p.contains("Sem pedir confirmação"));
        assert!(p.contains("projects/g%2Fbots-api/merge_requests/7/discussions/<THREAD_ID>/notes"));
        assert!(p.contains("THREAD_ID abc em src/a.ts:3\n  @ana: e o env?"));
        assert_eq!(prompt(&row, &Ev::Review), None);
    }
}
