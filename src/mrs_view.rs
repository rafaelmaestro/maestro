use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use futures::StreamExt;
use gpui_kit::{prelude::FluentBuilder, *};
use serde_json::Value;

use crate::{
    diff::{self, FileDiff, Kind},
    editor,
    form::{FieldEvent, TextField, button, checkbox},
    mr::Status,
    mr_notify::{self, Notice},
    mrs::{self, Draft, Finding, MergeOpts, Opened, Row, Run},
    theme::{self, Theme},
    tour::{self, Anchors},
};

pub enum Event {
    Toast(String),
    /// confirmação antes de publicar: (título, texto)
    AskPublish(String, String),
    /// descarte com "Desfazer" no toast
    Discarded(String),
    /// o diálogo de merge: título, `fonte → destino · CI`, opções do MR e se só dá para agendar
    AskMerge(String, String, MergeOpts, bool),
    /// MR novo ou resposta: toast com "Abrir" (e notify-send com a janela fora de foco)
    Notify(String, String),
    /// "?" num bloco: a gaveta "Perguntar ao Claude"
    AskClaude(crate::ask::Ctx),
    /// seus MRs abertos, a cada volta do polling: o vigia do shell decide o que mandar ao Claude
    Mine(String, Vec<Row>),
}

#[derive(Clone, Copy)]
enum Drag {
    Files,
    Drawer,
}

#[derive(Clone, Copy)]
enum DRow {
    Desc,
    Disc,
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    /// lado a lado: índices da velha e da nova no hunk
    Pair(usize, usize, Option<usize>, Option<usize>),
    /// rascunhos em linhas fora dos hunks, no fim do arquivo
    Orphans(usize),
    Lone,
    Gens,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Review,
    /// os de "Para revisar" que você já revisou (fora da conta do título)
    Reviewed,
    Mine,
    Group,
}

enum Phase {
    Setup,
    Run(Vec<(&'static str, String)>),
    Done(Vec<(Finding, bool)>, f64),
}

/// A gaveta "Revisão Claude" do protótipo.
struct Drawer {
    row: Row,
    phase: Phase,
    incremental: bool,
    pid: Option<u32>,
    /// 6. pipeline vermelho ou rodando: avisa antes de revisar
    ci_warn: Option<&'static str>,
}

/// O modo MRs do protótipo: inbox (`renderInbox`), o MR com threads e rascunhos (`renderMR`) e o revisor.
pub struct MrsView {
    me: Option<String>,
    lists: [Option<Result<Vec<Row>, String>>; 3],
    ci: HashMap<String, Status>,
    /// o seu estado de revisor no GitLab, por MR de "Para revisar"
    states: HashMap<String, String>,
    tab: Tab,
    sel: usize,
    filter: Entity<TextField>,
    seen: String,
    open: Option<(Row, Option<Result<Opened, String>>)>,
    drafts: HashMap<String, Vec<Draft>>,
    next_id: usize,
    /// a linha clicada: (arquivo, linha nova, linha velha); removida só tem a velha
    selected: Option<(String, Option<u32>, Option<u32>)>,
    editing: Option<(usize, Entity<TextField>)>,
    drawer: Option<Drawer>,
    /// o `S.undo` do protótipo: (MR, [(posição, rascunho)]) por descarte
    undo: Vec<(String, Vec<(usize, Draft)>)>,
    posting: bool,
    /// o diff do MR aberto, virtualizado: só as linhas na tela viram elemento
    diff: ListState,
    drows: Vec<DRow>,
    scroll_to: Option<String>,
    /// hash do diff de cada arquivo do MR aberto, para o "Visto"
    hashes: HashMap<String, u64>,
    list_scroll: ScrollHandle,
    focus: FocusHandle,
    files_w: f32,
    drawer_w: f32,
    /// lado a lado (o padrão) ou unificado; `S` alterna
    split: bool,
    collapsed: HashSet<String>,
    /// "Visto" do MR aberto: arquivo → hash do diff quando você marcou
    viewed: HashMap<String, u64>,
    /// o arquivo onde o `n`/`p`/`v` está
    cur_file: Option<String>,
    /// MRs com a descrição recolhida (nesta sessão)
    desc_closed: HashSet<String>,
    hide_gen: bool,
    gen_open: bool,
    /// a thread com a caixa de resposta aberta
    replying: Option<(String, Entity<TextField>)>,
    /// aprovar, merge ou resolver em andamento (o botão fica apagado)
    busy: bool,
    notify: mr_notify::State,
    /// `path!iid` → quem respondeu (ponto laranja e "nova resposta" na inbox)
    replied: HashMap<String, String>,
    /// notas que chegaram por notificação: selo "nova" na thread
    fresh: HashSet<u64>,
    theme: Theme,
    anchors: Anchors,
    _poll: Task<()>,
}

impl EventEmitter<Event> for MrsView {}

fn state_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".config/maestro/state.toml")
}

/// `mr_unified = true` no `state.toml`: a escolha de Lado a lado/Unificado sobrevive ao reinício.
fn unified_pref() -> bool {
    std::fs::read_to_string(state_path())
        .is_ok_and(|s| s.lines().any(|l| l.trim() == "mr_unified = true"))
}

fn save_unified(on: bool) {
    let path = state_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().starts_with("mr_unified"))
        .collect();
    let line = format!("mr_unified = {on}");
    lines.push(&line);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, lines.join("\n") + "\n");
}

fn red() -> Hsla {
    rgb(0xff5555).into()
}
fn yellow() -> Hsla {
    rgb(0xf1fa8c).into()
}

/// Onde um cartão fica no diff.
#[derive(Clone, Copy)]
enum Side {
    Unified,
    Left,
    Right,
    Disc,
}

/// O `.pill` do protótipo; `dashed` é o `.pill.gen`.
fn pill(t: &Theme, text: &str, c: Hsla, dashed: bool) -> Div {
    div()
        .font_family(t.mono.clone())
        .text_size(px(12.))
        .px(px(7.))
        .rounded(px(10.))
        .border_1()
        .when(dashed, |d| d.border_dashed())
        .border_color(c)
        .text_color(c)
        .whitespace_nowrap()
        .child(text.to_string())
}

impl MrsView {
    pub fn new(theme: Theme, anchors: Anchors, cx: &mut Context<Self>) -> Self {
        let t = theme.clone();
        let filter = cx.new(|cx| TextField::new(t, "", "filtrar  ( / )", false, false, cx));
        cx.subscribe(&filter, |this, _, e: &FieldEvent, cx| {
            if matches!(e, FieldEvent::Changed) {
                this.sel = 0;
            }
            cx.notify();
        })
        .detach();
        // ponytail: a inbox relê a cada 60 s, como o desenho pede (R força)
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
            }
        });
        Self {
            me: None,
            lists: [None, None, None],
            ci: HashMap::new(),
            states: HashMap::new(),
            tab: Tab::Review,
            sel: 0,
            filter,
            seen: std::fs::read_to_string(mrs::seen_file()).unwrap_or_default(),
            open: None,
            drafts: HashMap::new(),
            next_id: 0,
            selected: None,
            editing: None,
            drawer: None,
            undo: Vec::new(),
            posting: false,
            diff: ListState::new(0, ListAlignment::Top, px(400.)),
            drows: Vec::new(),
            scroll_to: None,
            hashes: HashMap::new(),
            list_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            files_w: 270.,
            drawer_w: 440.,
            split: !unified_pref(),
            collapsed: HashSet::new(),
            viewed: HashMap::new(),
            cur_file: None,
            desc_closed: HashSet::new(),
            hide_gen: true,
            gen_open: false,
            replying: None,
            busy: false,
            notify: mr_notify::State::load(),
            replied: HashMap::new(),
            fresh: HashSet::new(),
            theme,
            anchors,
            _poll: poll,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Quantos MRs esperam a sua revisão (o número de "MRs" no título).
    pub fn waiting(&self) -> usize {
        match &self.lists[0] {
            Some(Ok(v)) => v.iter().filter(|r| !self.is_reviewed(r)).count(),
            _ => 0,
        }
    }

    fn is_reviewed(&self, r: &Row) -> bool {
        mrs::reviewed(&self.seen, r, self.states.get(&r.key()).map(String::as_str))
    }

    /// As linhas de uma aba, antes do filtro de texto.
    fn tab_rows(&self, tab: Tab) -> Option<Result<Vec<Row>, String>> {
        let i = match tab {
            Tab::Review | Tab::Reviewed => 0,
            Tab::Mine => 1,
            Tab::Group => 2,
        };
        self.lists[i].clone().map(|l| {
            l.map(|v| match tab {
                Tab::Review => v.into_iter().filter(|r| !self.is_reviewed(r)).collect(),
                Tab::Reviewed => v.into_iter().filter(|r| self.is_reviewed(r)).collect(),
                _ => v,
            })
        })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let me = self.me.clone();
        cx.spawn(async move |this, cx| {
            let (me, lists) = cx
                .background_executor()
                .spawn(async move {
                    let Some(me) = me.or_else(mrs::me) else {
                        let e = Err("glab sem login: rode glab auth login".to_string());
                        return (None, [e.clone(), e.clone(), e]);
                    };
                    let l = mrs::inbox(&me, &mrs::group());
                    (Some(me), l)
                })
                .await;
            let rows: Vec<Row> = lists.iter().flatten().flatten().cloned().collect();
            let review: Vec<String> = lists[0].iter().flatten().map(Row::key).collect();
            let me_name = me.clone().unwrap_or_default();
            // respostas: nos MRs que você revisa (inclui Revisados) e nos seus
            let mut watch: Vec<Row> = Vec::new();
            for r in lists[0].iter().chain(&lists[1]).flatten() {
                if !watch.iter().any(|w| w.key() == r.key()) {
                    watch.push(r.clone());
                }
            }
            let fresh_review = lists[0].as_ref().ok().cloned();
            let mine = lists[1].as_ref().ok().cloned();
            let _ = this.update(cx, |this, cx| {
                this.me = me;
                this.seen = std::fs::read_to_string(mrs::seen_file()).unwrap_or_default();
                this.lists = lists.map(Some);
                if let Some(v) = fresh_review {
                    let n = this.notify.new_mrs(&v);
                    this.announce(n, cx);
                }
                if let (Some(v), false) = (mine, me_name.is_empty()) {
                    cx.emit(Event::Mine(me_name.clone(), v));
                }
                cx.notify();
            });
            for r in watch {
                let changed = this
                    .update(cx, |this, _| this.notify.changed(&r))
                    .unwrap_or(false);
                if !changed || me_name.is_empty() {
                    continue;
                }
                let (path, iid) = (r.path.clone(), r.iid);
                let discussions = cx
                    .background_executor()
                    .spawn(async move {
                        let base = format!("projects/{}/merge_requests/{iid}", mrs::enc(&path));
                        mrs::read(&format!("{base}/discussions?per_page=100"), true)
                    })
                    .await;
                let Ok(d) = discussions else { continue };
                let me = me_name.clone();
                if this
                    .update(cx, |this, cx| {
                        let n = this.notify.replies(&r, &d, &me);
                        this.announce(n, cx);
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, _| this.notify.save());
            // o selo de CI vem do detalhe de cada MR (a lista não traz o pipeline)
            for r in rows {
                let (path, iid, key) = (r.path.clone(), r.iid, r.key());
                // em "Para revisar", também o seu estado de revisor (a lista não traz)
                let ask = review.contains(&key).then(|| me_name.clone());
                let (ci, state) = cx
                    .background_executor()
                    .spawn(async move {
                        let base = format!("projects/{}/merge_requests/{iid}", mrs::enc(&path));
                        let ci = mrs::read(&base, false).map(|d| mrs::ci_of(&d));
                        let state = ask.and_then(|me| {
                            mrs::read(&format!("{base}/reviewers"), false)
                                .ok()
                                .and_then(|v| mrs::my_review_state(&v, &me))
                        });
                        (ci, state)
                    })
                    .await;
                let alive = this.update(cx, |this, cx| {
                    if let Ok(ci) = ci {
                        this.ci.insert(key.clone(), ci);
                    }
                    if let Some(s) = state {
                        this.states.insert(key, s);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn announce(&mut self, notices: Vec<Notice>, cx: &mut Context<Self>) {
        let proj = |k: &str| {
            let (p, iid) = k.rsplit_once('!').unwrap_or((k, ""));
            format!("{} !{iid}", p.rsplit('/').next().unwrap_or(p))
        };
        for n in notices {
            let (text, key) = match n {
                Notice::NewMr { key, who, .. } => {
                    (format!("{who} pediu sua revisão em {}", proj(&key)), key)
                }
                Notice::Reply { key, who, note, .. } => {
                    self.fresh.insert(note);
                    self.replied.insert(key.clone(), who.clone());
                    (
                        format!("{who} respondeu à sua thread em {}", proj(&key)),
                        key,
                    )
                }
            };
            cx.emit(Event::Notify(text, key));
        }
    }

    /// O "Abrir" da notificação: acha o MR nas listas e abre.
    pub fn open_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let row = self
            .lists
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .find(|r| r.key() == key)
            .cloned();
        if let Some(r) = row {
            self.open_row(r, cx);
        }
    }

    /// Roda uma escrita no GitLab fora da thread de UI e relê o MR no fim.
    fn act(
        &mut self,
        ok: String,
        f: impl FnOnce(&str, u64, &str) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some((row, _)) = self.open.clone() else {
            return;
        };
        let head = self.head();
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (path, iid) = (row.path.clone(), row.iid);
            let r = cx
                .background_executor()
                .spawn(async move { f(&path, iid, &head) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match r {
                    Ok(()) => {
                        cx.emit(Event::Toast(ok));
                        this.open_row(row, cx);
                        this.refresh(cx);
                    }
                    Err(e) => cx.emit(Event::Toast(format!("GitLab: {e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_approve(&mut self, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = &self.open else {
            return;
        };
        let name = format!("{} !{}", row.proj(), row.iid);
        if o.approvals["user_has_approved"] == true {
            self.act(
                format!("Aprovação removida de {name}"),
                |p, i, _| mrs::unapprove(p, i),
                cx,
            );
        } else {
            self.act(format!("Você aprovou {name}"), mrs::approve, cx);
        }
    }

    /// `Merge…`: o diálogo do shell mostra as opções; `Cancelar merge agendado` cancela direto.
    fn ask_merge(&mut self, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = &self.open else {
            return;
        };
        if o.detail["merge_when_pipeline_succeeds"] == true {
            let iid = row.iid;
            return self.act(
                format!("Merge agendado de !{iid} cancelado"),
                |p, i, _| mrs::cancel_auto_merge(p, i),
                cx,
            );
        }
        let d = &o.detail;
        let running = d["detailed_merge_status"] == "ci_still_running";
        let ci = Self::ci_label(self.ci.get(&row.key()).copied(), &self.theme).0;
        let mut line = format!(
            "{} → {} · {ci}",
            d["source_branch"].as_str().unwrap_or(&row.source),
            d["target_branch"].as_str().unwrap_or(&row.target)
        );
        if o.approvals["user_has_approved"] == true {
            line.push_str(" · aprovado por você");
        }
        let mut opts = MergeOpts::from_detail(d);
        opts.when_pipeline_succeeds = running;
        cx.emit(Event::AskMerge(
            format!("Merge de {} !{}?", row.proj(), row.iid),
            line,
            opts,
            running,
        ));
    }

    pub fn merge(&mut self, o: MergeOpts, cx: &mut Context<Self>) {
        let Some((row, _)) = &self.open else { return };
        let (iid, target) = (row.iid, row.target.clone());
        let ok = if o.when_pipeline_succeeds {
            format!("Merge de !{iid} agendado: sai quando o pipeline passar")
        } else {
            format!(
                "!{iid} mergeado em {target}{}{}",
                if o.squash { " (squash)" } else { "" },
                if o.remove_branch {
                    " · branch de origem apagada"
                } else {
                    ""
                }
            )
        };
        self.act(ok, move |p, i, h| mrs::merge(p, i, h, &o), cx);
    }

    fn resolve(&mut self, id: String, resolved: bool, cx: &mut Context<Self>) {
        let ok = if resolved {
            "Thread reaberta"
        } else {
            "Thread resolvida"
        };
        self.act(
            ok.into(),
            move |p, iid, _| {
                let ep = format!(
                    "projects/{}/merge_requests/{iid}/discussions/{id}",
                    mrs::enc(p)
                );
                mrs::write("PUT", &ep, &serde_json::json!({ "resolved": !resolved })).map(drop)
            },
            cx,
        );
    }

    fn start_reply(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.theme.clone();
        let f = cx.new(|cx| {
            TextField::new(
                t,
                "",
                "Responder… (vira rascunho e sai junto com a review)",
                true,
                false,
                cx,
            )
        });
        let did = id.clone();
        cx.subscribe_in(
            &f,
            window,
            move |this, _, e: &FieldEvent, window, cx| match e {
                FieldEvent::Submit => {
                    window.focus(&this.focus, cx);
                    this.add_reply(&did, cx)
                }
                FieldEvent::Cancel => {
                    window.focus(&this.focus, cx);
                    this.replying = None;
                    cx.notify();
                }
                FieldEvent::Changed => {}
            },
        )
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        self.replying = Some((id, f));
        cx.notify();
    }

    /// O `addReply` do protótipo: a resposta vira rascunho e sai junto com a review.
    fn add_reply(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((_, f)) = self.replying.clone() else {
            return;
        };
        let body = f.read(cx).value.trim().to_string();
        if body.is_empty() {
            return cx.emit(Event::Toast("Escreva a resposta antes".into()));
        }
        self.next_id += 1;
        let (nid, head) = (self.next_id, self.head());
        if let Some(v) = self.mr_drafts() {
            v.push(Draft {
                id: nid,
                file: String::new(),
                line: None,
                old: None,
                body,
                by: "você".into(),
                sev: None,
                label: None,
                head,
                gitlab_id: None,
                error: None,
                general: false,
                reply_to: Some(id.to_string()),
            });
        }
        self.replying = None;
        cx.emit(Event::Toast(
            "Resposta adicionada à review. Sai quando você publicar.".into(),
        ));
        cx.notify();
    }

    fn toggle_split(&mut self, cx: &mut Context<Self>) {
        self.split = !self.split;
        save_unified(!self.split);
        cx.notify();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let q = self.filter.read(cx).value.to_lowercase();
        match &self.tab_rows(self.tab) {
            Some(Ok(v)) => v
                .iter()
                .filter(|m| {
                    q.is_empty()
                        || format!("{} {} {} !{}", m.proj(), m.title, m.who, m.iid)
                            .to_lowercase()
                            .contains(&q)
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `novo`, `revisado` ou `+N commits` (o sha mudou desde a sua última revisão).
    fn pill(&self, r: &Row) -> (&'static str, String) {
        if self.replied.contains_key(&r.key()) {
            return ("reply", "nova resposta".into());
        }
        match self.states.get(&r.key()).map(String::as_str) {
            Some("approved") => return ("", "aprovado".into()),
            Some("requested_changes") => return ("", "pediu mudanças".into()),
            Some("reviewed") => return ("", "revisado".into()),
            _ => {}
        }
        if mrs::seen(&self.seen, &r.path, r.iid, &r.sha) {
            ("", "revisado".into())
        } else if mrs::last_seen(&self.seen, &r.path, r.iid).is_some() {
            ("inc", "commits novos".into())
        } else {
            ("new", "novo".into())
        }
    }

    pub fn open_row(&mut self, r: Row, cx: &mut Context<Self>) {
        let (path, iid) = (r.path.clone(), r.iid);
        self.replied.remove(&r.key());
        // reabrir o mesmo MR (depois de aprovar, resolver…) mantém a tela até o novo chegar
        let same = self.open.as_ref().is_some_and(|(o, _)| o.key() == r.key());
        if !same {
            let state = std::fs::read_to_string(mrs::viewed_file()).unwrap_or_default();
            self.viewed = mrs::viewed(&state, &r.key());
            self.cur_file = None;
            self.open = Some((r, None));
            self.selected = None;
            self.collapsed.clear();
            self.replying = None;
            self.hashes.clear();
            self.diff.reset(0);
        }
        cx.spawn(async move |this, cx| {
            let o = cx
                .background_executor()
                .spawn(async move { mrs::open(&path, iid) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some((row, slot)) = this.open.as_mut()
                    && row.iid == iid
                {
                    if let Ok(o) = &o {
                        this.hashes = o
                            .raw
                            .iter()
                            .map(|(p, _, d)| (p.clone(), mrs::diff_hash(d)))
                            .collect();
                        for (path, h) in &this.hashes {
                            if this.viewed.get(path) == Some(h) {
                                this.collapsed.insert(path.clone());
                            }
                        }
                    }
                    *slot = Some(o);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn file_hash(&self, path: &str) -> u64 {
        self.hashes.get(path).copied().unwrap_or_default()
    }

    fn is_viewed(&self, _: &Opened, path: &str) -> bool {
        self.viewed.get(path) == Some(&self.file_hash(path))
    }

    /// Marcado e depois mudou num commit novo: volta a "não visto", com aviso.
    fn changed_since(&self, _: &Opened, path: &str) -> bool {
        self.viewed
            .get(path)
            .is_some_and(|h| *h != self.file_hash(path))
    }

    fn visible_files(&self, o: &Opened) -> Vec<String> {
        o.files
            .iter()
            .filter(|f| !(self.hide_gen && Self::is_gen(o, &f.path)))
            .map(|f| f.path.clone())
            .collect()
    }

    /// `v` e o checkbox: marca (recolhe) ou desmarca; `next` pula para o próximo não visto.
    fn toggle_viewed(&mut self, path: &str, next: bool, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = &self.open else {
            return;
        };
        let (key, hash) = (row.key(), self.file_hash(path));
        let on = !self.is_viewed(o, path);
        let file = mrs::viewed_file();
        let state = std::fs::read_to_string(&file).unwrap_or_default();
        let _ = std::fs::create_dir_all(file.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(
            &file,
            mrs::set_viewed(&state, &key, path, on.then_some(hash)),
        );
        if on {
            self.viewed.insert(path.to_string(), hash);
            self.collapsed.insert(path.to_string());
        } else {
            self.viewed.remove(path);
            self.collapsed.remove(path);
        }
        if on && next {
            let files = self.visible_files(o);
            match files.iter().find(|f| !self.is_viewed(o, f)) {
                Some(f) => {
                    let f = f.clone();
                    return self.jump(&f, cx);
                }
                None => cx.emit(Event::Toast(format!(
                    "Todos os {} arquivos vistos. Publique a review ou aprove.",
                    files.len()
                ))),
            }
        }
        cx.notify();
    }

    fn jump(&mut self, path: &str, cx: &mut Context<Self>) {
        self.collapsed.remove(path);
        self.cur_file = Some(path.to_string());
        self.scroll_to_file(path, cx);
    }

    fn step_file(&mut self, d: isize, cx: &mut Context<Self>) {
        let Some((_, Some(Ok(o)))) = &self.open else {
            return;
        };
        let files = self.visible_files(o);
        if files.is_empty() {
            return;
        }
        let i = self
            .cur_file
            .as_ref()
            .and_then(|c| files.iter().position(|f| f == c))
            .map_or(if d > 0 { -1 } else { files.len() as isize }, |i| {
                i as isize
            });
        let j = (i + d).clamp(0, files.len() as isize - 1) as usize;
        let f = files[j].clone();
        self.jump(&f, cx);
    }

    fn open_gitlab(&self, r: &Row) {
        crate::spawn_reaped(std::process::Command::new("xdg-open").arg(&r.web_url));
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some((r, _)) = self.open.clone() {
            cx.emit(Event::Toast(format!(
                "Atualizando {} !{}…",
                r.proj(),
                r.iid
            )));
            self.open_row(r, cx);
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.editing = None;
        cx.notify();
    }

    fn head(&self) -> String {
        match &self.open {
            Some((_, Some(Ok(o)))) => o.detail["diff_refs"]["head_sha"]
                .as_str()
                .unwrap_or_default()
                .into(),
            _ => String::new(),
        }
    }

    fn mr_drafts(&mut self) -> Option<&mut Vec<Draft>> {
        let k = self.open.as_ref()?.0.key();
        Some(self.drafts.entry(k).or_default())
    }

    fn new_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((file, line, old)) = self.selected.clone() else {
            return cx.emit(Event::Toast("Clique numa linha do diff primeiro".into()));
        };
        self.next_id += 1;
        let id = self.next_id;
        let head = self.head();
        if let Some(d) = self.mr_drafts() {
            d.push(Draft {
                id,
                file,
                line,
                old: if line.is_none() { old } else { None },
                body: String::new(),
                by: "você".into(),
                sev: None,
                label: None,
                head,
                gitlab_id: None,
                error: None,
                general: false,
                reply_to: None,
            });
        }
        self.edit(id, String::new(), window, cx);
    }

    fn edit(&mut self, id: usize, body: String, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.theme.clone();
        let f =
            cx.new(|cx| TextField::new(t, &body, "Comentário para o autor do MR", true, false, cx));
        cx.subscribe_in(&f, window, move |this, f, e: &FieldEvent, window, cx| {
            let text = f.read(cx).value.trim().to_string();
            match e {
                FieldEvent::Submit if text.is_empty() => {
                    return cx.emit(Event::Toast("Escreva algo antes de salvar".into()));
                }
                FieldEvent::Submit => {
                    if let Some(d) = this
                        .mr_drafts()
                        .and_then(|v| v.iter_mut().find(|d| d.id == id))
                    {
                        d.body = text;
                    }
                }
                FieldEvent::Cancel => {
                    if let Some(v) = this.mr_drafts() {
                        v.retain(|d| d.id != id || !d.body.is_empty());
                    }
                }
                FieldEvent::Changed => return,
            }
            window.focus(&this.focus, cx);
            this.editing = None;
            cx.notify();
        })
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        self.editing = Some((id, f));
        cx.notify();
    }

    fn pending(&self) -> Vec<&Draft> {
        let Some((row, _)) = &self.open else {
            return Vec::new();
        };
        self.drafts
            .get(&row.key())
            .map(|v| v.iter().filter(|d| !d.body.is_empty()).collect())
            .unwrap_or_default()
    }

    /// O `askPublish` do protótipo: confirma com a contagem e quem recebe a notificação.
    fn ask_publish(&mut self, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = &self.open else {
            return;
        };
        let n = self.pending().len();
        if n == 0 {
            return cx.emit(Event::Toast("Nenhum rascunho para publicar".into()));
        }
        let state = o.detail["state"].as_str().unwrap_or_default();
        if !mrs::can_publish(state) {
            return cx.emit(Event::Toast(format!(
                "O MR está {state}: os rascunhos ficam só aqui (ninguém lê comentário em MR fechado)"
            )));
        }
        let (path, iid) = (row.path.clone(), row.iid);
        let ours: Vec<u64> = self.pending().iter().filter_map(|d| d.gitlab_id).collect();
        let foreign = mrs::foreign_drafts(&path, iid, &ours);
        let extra = if foreign > 0 {
            format!(
                " Atenção: você tem {foreign} rascunho{} seu{} neste MR feito{} fora do Maestro; o GitLab publica junto.",
                if foreign > 1 { "s" } else { "" },
                if foreign > 1 { "s" } else { "" },
                if foreign > 1 { "s" } else { "" }
            )
        } else {
            String::new()
        };
        cx.emit(Event::AskPublish(
            format!(
                "Publicar {n} comentário{} em {} !{iid}?",
                if n > 1 { "s" } else { "" },
                row.proj()
            ),
            format!(
                "Sai como uma review só: {} recebe 1 notificação.{extra}",
                row.who
            ),
        ));
    }

    pub fn publish(&mut self, cx: &mut Context<Self>) {
        let Some((row, Some(Ok(o)))) = self.open.clone() else {
            return;
        };
        let key = row.key();
        let mut drafts: Vec<Draft> = self.drafts.get(&key).cloned().unwrap_or_default();
        drafts.retain(|d| !d.body.is_empty());
        self.posting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (r, drafts) = cx
                .background_executor()
                .spawn(async move {
                    let r = mrs::publish(&row.path, row.iid, &o, &mut drafts);
                    (r, drafts)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.posting = false;
                match r {
                    Ok(()) => {
                        this.drafts.remove(&key);
                        if let Some((row, _)) = &this.open {
                            mrs::mark_seen(&row.path, row.iid, &row.sha);
                        }
                        cx.emit(Event::Toast("Review publicada".into()));
                    }
                    Err(e) => {
                        this.drafts.insert(key, drafts);
                        cx.emit(Event::Toast(format!(
                            "Review não publicada: {e}. Veja em vermelho."
                        )));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn discard(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some((row, _)) = &self.open else { return };
        let key = row.key();
        let Some(v) = self.mr_drafts() else { return };
        let Some(i) = v.iter().position(|d| d.id == id) else {
            return;
        };
        let d = v.remove(i);
        self.undo.push((key, vec![(i, d)]));
        cx.emit(Event::Discarded("Comentário descartado".into()));
        cx.notify();
    }

    /// O `undo()` do protótipo: devolve o último descarte no lugar.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let Some((key, mut g)) = self.undo.pop() else {
            cx.emit(Event::Toast("Nada para desfazer".into()));
            return;
        };
        g.sort_by_key(|x| x.0);
        let v = self.drafts.entry(key).or_default();
        for (i, mut d) in g {
            // a draft note remota já foi apagada: volta como rascunho local
            d.gitlab_id = None;
            v.insert(i.min(v.len()), d);
        }
        cx.emit(Event::Toast("Desfeito".into()));
        cx.notify();
    }

    fn discard_all(&mut self, cx: &mut Context<Self>) {
        let Some((row, _)) = &self.open else { return };
        let (path, iid, key) = (row.path.clone(), row.iid, row.key());
        let ids: Vec<u64> = self.pending().iter().filter_map(|d| d.gitlab_id).collect();
        let gone: Vec<(usize, Draft)> = self
            .mr_drafts()
            .map(|v| v.drain(..).enumerate().collect())
            .unwrap_or_default();
        if gone.is_empty() {
            return;
        }
        let n = gone.len();
        self.undo.push((key, gone));
        cx.emit(Event::Discarded(format!(
            "{n} descartado{}",
            if n > 1 { "s" } else { "" }
        )));
        // o que já tinha ido como draft note sai do GitLab também
        std::thread::spawn(move || mrs::discard_remote(&path, iid, &ids));
        cx.notify();
    }

    pub fn start_review(&mut self, cx: &mut Context<Self>) {
        let row = match &self.open {
            Some((r, _)) => Some(r.clone()),
            None => self.rows(cx).get(self.sel).cloned(),
        };
        let Some(row) = row else { return };
        let inc = mrs::last_seen(&self.seen, &row.path, row.iid).is_some_and(|s| s != row.sha);
        let status = match self.ci.get(&row.key()) {
            Some(Status::Fail) => "failed",
            Some(Status::Running) => "running",
            _ => "success",
        };
        let ci_warn = (!mrs::pipeline_ok(status)).then_some(if status == "failed" {
            "Pipeline falhou"
        } else {
            "Pipeline ainda rodando"
        });
        self.drawer = Some(Drawer {
            row,
            phase: Phase::Setup,
            incremental: inc,
            pid: None,
            ci_warn,
        });
        cx.notify();
    }

    fn run_review(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drawer.as_mut() else {
            return;
        };
        let row = d.row.clone();
        let since = d
            .incremental
            .then(|| mrs::last_seen(&self.seen, &row.path, row.iid))
            .flatten();
        d.phase = Phase::Run(Vec::new());
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Run>();
        std::thread::spawn(move || {
            let base = format!(
                "projects/{}/merge_requests/{}",
                mrs::enc(&row.path),
                row.iid
            );
            let detail = match mrs::read(&base, false) {
                Ok(v) => v,
                Err(e) => return drop(tx.unbounded_send(Run::Failed(e))),
            };
            let notes =
                mrs::read(&format!("{base}/notes?per_page=100"), true).unwrap_or(Value::Null);
            mrs::review(
                &row.path,
                row.iid,
                &row.title,
                &row.target,
                detail["description"].as_str().unwrap_or_default(),
                &notes,
                since,
                |m| drop(tx.unbounded_send(m)),
            );
        });
        cx.spawn(async move |this, cx| {
            while let Some(m) = rx.next().await {
                let stop = this
                    .update(cx, |this, cx| {
                        let raw = match &this.open {
                            Some((_, Some(Ok(o)))) => {
                                o.raw.iter().map(|r| (r.0.clone(), r.2.clone())).collect()
                            }
                            _ => Vec::new(),
                        };
                        let Some(d) = this.drawer.as_mut() else {
                            return true;
                        };
                        match (m, &mut d.phase) {
                            (Run::Pid(p), _) => d.pid = Some(p),
                            (Run::Ok(t), Phase::Run(s)) => {
                                for x in s.iter_mut().filter(|x| x.0 == "run") {
                                    x.0 = "ok";
                                }
                                s.push(("ok", t));
                            }
                            (Run::Tool(t), Phase::Run(s)) => s.push(("sub", t)),
                            (Run::Done(mut f, cost), _) => {
                                d.pid = None;
                                mrs::mark_outside(&mut f, &raw);
                                d.phase = Phase::Done(
                                    f.into_iter()
                                        .map(|x| {
                                            let on = !x.outside;
                                            (x, on)
                                        })
                                        .collect(),
                                    cost,
                                );
                                mrs::mark_seen(&d.row.path, d.row.iid, &d.row.sha);
                            }
                            (Run::Failed(e), _) => {
                                cx.emit(Event::Toast(format!("Revisão falhou: {e}")));
                                this.drawer = None;
                            }
                            _ => {}
                        }
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn close_drawer(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.drawer.as_ref().and_then(|d| d.pid) {
            let _ = std::process::Command::new("kill")
                .arg(p.to_string())
                .status();
        }
        self.drawer = None;
        cx.notify();
    }

    /// O `makeDrafts` do protótipo: os achados marcados viram rascunhos; nada é publicado.
    fn make_drafts(&mut self, cx: &mut Context<Self>) {
        let Some(Drawer {
            row,
            phase: Phase::Done(f, _),
            ..
        }) = self.drawer.take()
        else {
            return;
        };
        let same = self
            .open
            .as_ref()
            .is_some_and(|(r, _)| r.key() == row.key());
        if !same {
            self.open_row(row.clone(), cx);
        }
        let head = self.head();
        let list = self.drafts.entry(row.key()).or_default();
        for (x, on) in f.into_iter().filter(|x| x.1) {
            let _ = on;
            self.next_id += 1;
            list.push(Draft {
                id: self.next_id,
                file: x.file,
                line: x.line,
                old: None,
                body: x.body,
                by: "Claude".into(),
                sev: Some(x.severity),
                label: Some(x.label),
                head: head.clone(),
                gitlab_id: None,
                error: None,
                general: x.outside,
                reply_to: None,
            });
        }
        cx.emit(Event::Toast(
            "Rascunhos criados. Nada foi publicado.".into(),
        ));
        cx.notify();
    }

    /// `set_offset` não limita: o offset vai de 0 a `-max_offset`.
    fn scroll_by(&self, dy: Pixels) {
        self.diff.scroll_by(dy);
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // tecla vinda de um campo (filtro, rascunho, resposta) é texto, não atalho
        if self.editing.is_some() || !self.focus.is_focused(window) {
            if e.keystroke.key == "escape" && self.editing.is_none() {
                self.filter.update(cx, |f, cx| f.set(String::new(), cx));
                window.focus(&self.focus, cx);
            }
            return;
        }
        let k = &e.keystroke;
        let shift_r = k.key == "r" && k.modifiers.shift && !k.modifiers.control;
        match (k.key.as_str(), &self.open) {
            _ if shift_r => self.start_review(cx),
            ("enter", Some(_)) if k.modifiers.control => self.ask_publish(cx),
            ("z", Some(_)) if k.modifiers.control && !k.modifiers.shift => self.undo(cx),
            ("c", Some(_)) if !k.modifiers.modified() => self.new_draft(window, cx),
            ("s", Some(_)) if !k.modifiers.modified() => self.toggle_split(cx),
            ("n", Some(_)) if !k.modifiers.modified() => self.step_file(1, cx),
            ("p", Some(_)) if !k.modifiers.modified() => self.step_file(-1, cx),
            ("v", Some(_)) if !k.modifiers.modified() => {
                let target = match &self.open {
                    Some((_, Some(Ok(o)))) => self.cur_file.clone().or_else(|| {
                        self.visible_files(o)
                            .into_iter()
                            .find(|f| !self.is_viewed(o, f))
                    }),
                    _ => None,
                };
                if let Some(f) = target {
                    self.toggle_viewed(&f, true, cx)
                }
            }
            ("o", Some((r, _))) if !k.modifiers.modified() => self.open_gitlab(r),
            ("r", Some(_)) if !k.modifiers.modified() => self.reload(cx),
            (_, Some(_)) if k.key_char.as_deref() == Some("?") && self.selected.is_some() => {
                let (path, new, old) = self.selected.clone().unwrap_or_default();
                match self.ask_ctx(&path, None, new.or(old).unwrap_or_default()) {
                    Some(c) => cx.emit(Event::AskClaude(c)),
                    None => return,
                }
            }
            ("escape", Some(_)) => self.back(cx),
            ("j" | "down", Some(_)) if !k.modifiers.modified() => self.scroll_by(px(66.)),
            ("k" | "up", Some(_)) if !k.modifiers.modified() => self.scroll_by(px(-66.)),
            ("pagedown" | "space", Some(_)) if !k.modifiers.modified() => {
                self.scroll_by(self.diff.viewport_bounds().size.height - px(44.))
            }
            ("pageup", Some(_)) if !k.modifiers.modified() => {
                self.scroll_by(px(44.) - self.diff.viewport_bounds().size.height)
            }
            ("home", Some(_)) => self.diff.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            }),
            ("end", Some(_)) => self.diff.scroll_to_end(),
            ("j" | "down", None) => {
                self.sel = (self.sel + 1).min(self.rows(cx).len().saturating_sub(1));
                // o filho 0 da lista é a âncora do tour
                self.list_scroll.scroll_to_item(self.sel + 1);
            }
            ("k" | "up", None) => {
                self.sel = self.sel.saturating_sub(1);
                self.list_scroll.scroll_to_item(self.sel + 1);
            }
            ("enter", None) => {
                if let Some(r) = self.rows(cx).get(self.sel).cloned() {
                    self.open_row(r, cx);
                }
            }
            ("/", None) => {
                let f = self.filter.read(cx).focus_handle();
                window.focus(&f, cx);
            }
            ("r", None) if !k.modifiers.modified() => self.refresh(cx),
            ("o", None) if !k.modifiers.modified() => {
                if let Some(r) = self.rows(cx).get(self.sel) {
                    self.open_gitlab(r);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// O trecho do MR aberto para o "Perguntar ao Claude": o bloco `hi` ou o que contém `line`.
    fn ask_ctx(&self, path: &str, hi: Option<usize>, line: u32) -> Option<crate::ask::Ctx> {
        let (row, Some(Ok(o))) = self.open.as_ref()? else {
            return None;
        };
        let f = o.files.iter().find(|f| f.path == path)?;
        let hi = hi.unwrap_or_else(|| crate::ask::hunk_at(f, line));
        let place = format!("worktree do MR {} !{}", row.proj(), row.iid);
        let mut c = crate::ask::Ctx::of_hunk(f, hi, place)?;
        c.mr = Some((row.path.clone(), row.iid, row.target.clone()));
        Some(c)
    }

    fn ci_label(s: Option<Status>, t: &Theme) -> (String, Hsla) {
        match s {
            Some(Status::Ok) => ("✓ CI".into(), t.ok),
            Some(Status::Running) => ("⟳ CI".into(), yellow()),
            Some(Status::Fail) => ("✗ CI".into(), red()),
            Some(Status::None) => ("– sem CI".into(), t.muted),
            None => ("…".into(), t.muted),
        }
    }

    fn inbox(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let rows = self.rows(cx);
        let tab = |id: &'static str, label: &str, which: Tab| {
            let n = match &self.tab_rows(which) {
                Some(Ok(v)) => v.len().to_string(),
                _ => "…".into(),
            };
            div()
                .id(id)
                .flex()
                .gap(px(4.))
                .items_center()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(if self.tab == which { t.fg } else { t.muted })
                .when(self.tab == which, |d| d.bg(t.accent_soft))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = which;
                    this.sel = 0;
                    cx.notify();
                }))
                .child(label.to_string())
                .child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .child(n),
                )
        };
        let group = mrs::group();
        let bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(tab("ib-rev", "Para revisar", Tab::Review))
                    .child(tab("ib-done", "Revisados", Tab::Reviewed))
                    .child(tab("ib-mine", "Meus", Tab::Mine))
                    .child(tab("ib-grp", &group, Tab::Group)),
            )
            .child(div().flex_1())
            .child(div().w(px(200.)).child(self.filter.clone()));
        let q = self.filter.read(cx).value.clone();
        let mut list = div()
            .id("ib-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .relative()
            .child(tour::anchor(&self.anchors, "ilist"));
        match &self.tab_rows(self.tab) {
            None => list = list.child(div().p(px(24.)).text_color(t.muted).child("Lendo os MRs…")),
            Some(Err(e)) => {
                list = list.child(self.empty("Não deu para ler os MRs".into(), e.clone()))
            }
            Some(Ok(_)) if rows.is_empty() && !q.is_empty() => {
                list = list.child(self.empty(
                    format!("Nenhum MR bate com “{q}”"),
                    "Esc limpa o filtro.".into(),
                ))
            }
            Some(Ok(_)) if rows.is_empty() => {
                list = list.child(self.empty(
                    "Nenhum MR aguardando você".into(),
                    "A inbox relê a cada 60 s; R força.".into(),
                ))
            }
            Some(Ok(_)) => {
                for (i, r) in rows.into_iter().enumerate() {
                    let (pk, pt) = self.pill(&r);
                    let pc = match pk {
                        "new" => t.accent,
                        "inc" | "reply" => t.orange,
                        _ => t.muted,
                    };
                    let (ci, cc) = Self::ci_label(self.ci.get(&r.key()).copied(), &t);
                    let dot = match pk {
                        "new" => Some(t.accent),
                        "reply" => Some(t.orange),
                        _ => None,
                    };
                    let r2 = r.clone();
                    list = list.child(
                        div()
                            .id(("ib", i))
                            .flex()
                            .gap(px(12.))
                            .items_center()
                            .px(px(12.))
                            .py(px(8.))
                            .border_b_1()
                            .border_color(t.line)
                            .cursor_pointer()
                            .when(i == self.sel, |d| d.bg(t.accent_soft))
                            .hover(|d| d.bg(t.raise))
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                this.sel = i;
                                if e.click_count() == 2 {
                                    this.open_row(r2.clone(), cx);
                                }
                                cx.notify();
                            }))
                            .child(div().w(px(14.)).flex_none().when_some(dot, |d, c| {
                                d.child(div().size(px(7.)).rounded_full().bg(c))
                            }))
                            .child(
                                div()
                                    .w(px(200.))
                                    .flex_none()
                                    .flex()
                                    .gap(px(4.))
                                    .font_family(t.mono.clone())
                                    .text_size(px(13.))
                                    .truncate()
                                    .child(r.proj().to_string())
                                    .child(div().text_color(t.muted).child(format!("!{}", r.iid))),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(r.title.clone()))
                            .child(
                                div()
                                    .w(px(120.))
                                    .flex_none()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(t.muted)
                                    .child(r.who.clone()),
                            )
                            .child(div().w(px(70.)).flex_none().text_color(cc).child(ci))
                            .child(
                                div().w(px(110.)).flex_none().child(
                                    div()
                                        .font_family(t.mono.clone())
                                        .text_size(px(12.))
                                        .px(px(7.))
                                        .rounded(px(10.))
                                        .border_1()
                                        .border_color(if pk.is_empty() { t.line } else { pc })
                                        .text_color(pc)
                                        .whitespace_nowrap()
                                        .child(pt),
                                ),
                            ),
                    );
                }
            }
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(bar)
            .child(list)
    }

    fn empty(&self, title: String, text: String) -> Div {
        let t = &self.theme;
        div()
            .m_auto()
            .max_w(px(380.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_center()
            .text_color(t.muted)
            .child(div().text_size(px(16.)).text_color(t.fg).child(title))
            .child(text)
    }

    fn code(&self, text: &str) -> StyledText {
        let hl: Vec<_> = diff::highlight(text)
            .into_iter()
            .map(|(r, tok)| (r, editor::style(&self.theme, tok)))
            .collect();
        StyledText::new(text.to_string()).with_highlights(hl)
    }

    /// Onde fica um cartão (thread ou rascunho): na coluna do diff unificado, numa das metades
    /// do lado a lado ou na seção Discussões (o `.thr`, `.thr.l`, `.thr.r` e `.disc .thr` do protótipo).
    fn place(&self, side: Side, card: Div) -> Div {
        let card = card.my(px(6.));
        match side {
            Side::Unified => div().pl(px(94.)).pr(px(12.)).child(card.max_w(px(720.))),
            Side::Disc => div().pl(px(12.)).pr(px(12.)).child(card.max_w(px(720.))),
            Side::Left => div()
                .flex()
                .child(
                    div()
                        .w(relative(0.5))
                        .flex_none()
                        .pl(px(58.))
                        .pr(px(12.))
                        .child(card),
                )
                .child(div().flex_1()),
            Side::Right => div()
                .flex()
                .child(div().w(relative(0.5)).flex_none())
                .child(div().flex_1().min_w_0().pl(px(58.)).pr(px(12.)).child(card)),
        }
    }

    fn thread(&self, d: &Value, i: usize, side: Side, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let notes = d["notes"].as_array().cloned().unwrap_or_default();
        let resolved = notes.first().is_some_and(|n| n["resolved"] == true);
        let resolvable = notes.first().is_some_and(|n| n["resolvable"] == true);
        let id = d["id"].as_str().unwrap_or_default().to_string();
        let who = |n: &Value| format!("@{}", n["author"]["username"].as_str().unwrap_or_default());
        let date = |n: &Value| {
            n["created_at"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(10)
                .collect::<String>()
        };
        let first = notes.first().cloned().unwrap_or_default();
        let pos = &first["position"];
        let removed = pos["new_line"]
            .is_null()
            .then(|| pos["old_line"].as_u64())
            .flatten();
        let replying = self
            .replying
            .as_ref()
            .filter(|(r, _)| *r == id)
            .map(|(_, f)| f.clone());
        let mut bd = div()
            .px(px(10.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(first["body"].as_str().unwrap_or_default().to_string());
        for n in notes.iter().skip(1) {
            let fresh = n["id"].as_u64().is_some_and(|x| self.fresh.contains(&x));
            bd = bd.child(
                div()
                    .border_l_2()
                    .border_color(t.line)
                    .pl(px(8.))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .child(div().font_weight(FontWeight::MEDIUM).child(who(n)))
                            .child(div().text_size(px(12.)).text_color(t.muted).child(date(n)))
                            .when(fresh, |x| x.child(pill(&t, "nova", t.accent, false))),
                    )
                    .child(n["body"].as_str().unwrap_or_default().to_string()),
            );
        }
        let replies: Vec<Draft> = self
            .open
            .as_ref()
            .and_then(|(row, _)| self.drafts.get(&row.key()))
            .map(|v| {
                v.iter()
                    .filter(|d| d.reply_to.as_deref() == Some(id.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for r in &replies {
            bd = bd.child(self.reply_draft(r, cx));
        }
        if let Some(f) = replying.clone() {
            let rid = id.clone();
            bd = bd.child(
                div().flex().flex_col().gap(px(6.)).child(f).child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .items_center()
                        .child(
                            button(&t, ("radd", i), "Adicionar à review", true).on_click(
                                cx.listener(move |this, _, _, cx| this.add_reply(&rid, cx)),
                            ),
                        )
                        .child(
                            button(&t, ("rcn", i), "Cancelar", false).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.replying = None;
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .child("Ctrl+⏎ adiciona · Esc cancela"),
                        ),
                ),
            );
        }
        let rid = id.clone();
        let card = div()
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .bg(t.raise)
            .font_family(t.sans)
            .text_size(px(14.))
            .when(resolved, |d| d.opacity(0.7))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .items_center()
                    .px(px(10.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child(
                        div()
                            .text_color(t.fg)
                            .font_weight(FontWeight::MEDIUM)
                            .child(who(&first)),
                    )
                    .child(date(&first))
                    .when_some(removed, |d, o| {
                        d.child(
                            div()
                                .font_family(t.mono.clone())
                                .child(format!("linha removida {o}")),
                        )
                    })
                    .child(div().flex_1())
                    .when(replying.is_none(), |d| {
                        d.child(button(&t, ("rep", i), "Responder", false).on_click(
                            cx.listener(move |this, _, w, cx| this.start_reply(rid.clone(), w, cx)),
                        ))
                    })
                    .when(resolvable, |d| {
                        let id = id.clone();
                        d.child(
                            button(
                                &t,
                                ("res", i),
                                if resolved { "Reabrir" } else { "✓ Resolver" },
                                false,
                            )
                            .when(self.busy, |b| b.opacity(0.45))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.busy {
                                        this.resolve(id.clone(), resolved, cx)
                                    }
                                },
                            )),
                        )
                    }),
            )
            .child(bd);
        self.place(side, card)
    }

    /// Resposta ainda não publicada, dentro da thread (o `.rp.draftrp` do protótipo).
    fn reply_draft(&self, d: &Draft, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let id = d.id;
        let editing = self
            .editing
            .as_ref()
            .filter(|(i, _)| *i == id)
            .map(|(_, f)| f.clone());
        let body = d.body.clone();
        div()
            .border_l_2()
            .border_dashed()
            .border_color(yellow())
            .bg(Hsla {
                a: 0.07,
                ..yellow()
            })
            .rounded_r(px(4.))
            .px(px(8.))
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(yellow())
                            .child("RASCUNHO · resposta"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("sai junto com a review"),
                    ),
            )
            .map(|x| match editing {
                Some(f) => x.child(f).child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child("Ctrl+⏎ salva · Esc cancela"),
                ),
                None => x.child(d.body.clone()).when(!self.posting, |x| {
                    x.child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                button(&t, ("red", id), "Editar", false).on_click(cx.listener(
                                    move |this, _, w, cx| this.edit(id, body.clone(), w, cx),
                                )),
                            )
                            .child(
                                button(&t, ("rdel", id), "Descartar", false)
                                    .text_color(red())
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.discard(id, cx)),
                                    ),
                            ),
                    )
                }),
            })
            .when_some(d.error.clone(), |x, e| {
                x.child(div().text_size(px(13.)).text_color(red()).child(e))
            })
    }

    fn draft_box(&self, d: &Draft, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let id = d.id;
        let editing = self
            .editing
            .as_ref()
            .filter(|(i, _)| *i == id)
            .map(|(_, f)| f.clone());
        let border = if d.error.is_some() { red() } else { yellow() };
        let sev_color = match d.sev.as_deref() {
            Some("blocker" | "major") => red(),
            Some("minor") => t.orange,
            _ => t.muted,
        };
        div()
            .rounded(px(6.))
            .font_family(t.sans)
            .text_size(px(14.))
            .border_1()
            .when(d.error.is_none(), |x| {
                x.border_dashed().bg(Hsla {
                    a: 0.07,
                    ..yellow()
                })
            })
            .border_color(border)
            .when(self.posting, |x| x.opacity(0.6))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(10.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(yellow())
                            .child("RASCUNHO"),
                    )
                    .child(format!("· {}", d.by))
                    .when_some(d.sev.clone(), |x, s| {
                        x.child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(12.))
                                .px(px(6.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(sev_color)
                                .text_color(sev_color)
                                .child(s),
                        )
                    })
                    .when(d.general || (d.line.is_none() && d.old.is_none()), |x| {
                        x.child("nota geral")
                    })
                    .when_some(d.old.filter(|_| d.line.is_none()), |x, o| {
                        x.child(format!("linha removida {o}"))
                    })
                    .child(div().flex_1())
                    .when(self.posting, |x| x.child("publicando…"))
                    .when(editing.is_none() && !self.posting, |x| {
                        let body = d.body.clone();
                        x.child(button(&t, ("ed", id), "Editar", false).on_click(
                            cx.listener(move |this, _, w, cx| this.edit(id, body.clone(), w, cx)),
                        ))
                        .child(
                            button(&t, ("del", id), "Descartar", false)
                                .text_color(red())
                                .on_click(cx.listener(move |this, _, _, cx| this.discard(id, cx))),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .when_some(editing, |x, f| {
                        x.child(f).child(
                            div()
                                .text_size(px(13.))
                                .text_color(t.muted)
                                .child("Ctrl+⏎ salva · Esc cancela"),
                        )
                    })
                    .when(self.editing.as_ref().is_none_or(|(i, _)| *i != id), |x| {
                        let label = d
                            .label
                            .clone()
                            .filter(|l| !l.is_empty())
                            .map(|l| format!("{l}: "))
                            .unwrap_or_default();
                        x.child(format!("{label}{}", d.body))
                    })
                    .when_some(d.error.clone(), |x, e| {
                        x.child(div().text_size(px(13.)).text_color(red()).child(e))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .child(
                                        button(&t, ("retry", id), "Tentar de novo", false)
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.publish(cx)),
                                            ),
                                    )
                                    .child(
                                        button(&t, ("gen", id), "Publicar como nota geral", false)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(d) = this
                                                    .mr_drafts()
                                                    .and_then(|v| v.iter_mut().find(|d| d.id == id))
                                                {
                                                    d.general = true;
                                                    d.error = None;
                                                }
                                                this.publish(cx);
                                            })),
                                    ),
                            )
                    }),
            )
    }

    /// Markdown leve: título em negrito, `**` some, `-` vira •, bloco de código em mono.
    fn desc_lines(desc: &str, t: &Theme) -> Vec<Div> {
        let mut code = false;
        let mut out = Vec::new();
        for l in desc.lines() {
            if l.trim_start().starts_with("```") {
                code = !code;
                continue;
            }
            let d = div().min_h(px(10.));
            out.push(if code {
                d.font_family(t.mono.clone())
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child(l.to_string())
            } else if let Some(h) = l.trim_start().strip_prefix('#') {
                d.pt(px(6.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(h.trim_start_matches('#').trim().replace("**", ""))
            } else if let Some(b) = l.trim_start().strip_prefix("- ") {
                d.child(format!("• {}", b.replace("**", "")))
            } else {
                d.child(l.replace("**", ""))
            });
        }
        out
    }

    fn description(o: &Opened) -> &str {
        o.detail["description"].as_str().unwrap_or_default().trim()
    }

    fn is_gen(o: &Opened, path: &str) -> bool {
        diff::boilerplate(path, o.generated.iter().any(|g| g == path))
    }

    /// As discussões que não são de sistema, com o índice original (id dos botões).
    fn threads(o: &Opened) -> Vec<(usize, &Value)> {
        o.discussions
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, d)| d["notes"][0]["system"] != true)
            .collect()
    }

    fn scroll_to_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some((_, Some(Ok(o)))) = &self.open
            && self.hide_gen
            && Self::is_gen(o, path)
        {
            self.hide_gen = false;
        }
        self.scroll_to = Some(path.to_string());
        cx.notify();
    }

    /// As linhas da lista; contar no fim preserva o topo da rolagem quando algo abre ou fecha.
    fn sync_rows(&mut self) {
        let mut rows = Vec::new();
        if let Some((_, Some(Ok(o)))) = &self.open {
            if !Self::description(o).is_empty() {
                rows.push(DRow::Desc);
            }
            if Self::threads(o)
                .iter()
                .any(|(_, d)| d["notes"][0]["position"].is_null())
            {
                rows.push(DRow::Disc);
            }
            let mut gens = false;
            for (fi, f) in o.files.iter().enumerate() {
                if Self::is_gen(o, &f.path) {
                    gens = true;
                    if self.hide_gen {
                        continue;
                    }
                }
                rows.push(DRow::File(fi));
                if self.collapsed.contains(&f.path) {
                    continue;
                }
                for (hi, h) in f.hunks.iter().enumerate() {
                    rows.push(DRow::Hunk(fi, hi));
                    if self.split {
                        rows.extend(
                            diff::side_by_side(&h.lines)
                                .into_iter()
                                .map(|(l, r)| DRow::Pair(fi, hi, l, r)),
                        );
                    } else {
                        rows.extend((0..h.lines.len()).map(|li| DRow::Line(fi, hi, li)));
                    }
                }
                rows.push(DRow::Orphans(fi));
            }
            rows.push(DRow::Lone);
            if self.hide_gen && gens {
                rows.push(DRow::Gens);
            }
            if let Some(target) = self.scroll_to.take()
                && let Some(ix) = rows
                    .iter()
                    .position(|r| matches!(r, DRow::File(fi) if o.files[*fi].path == target))
            {
                self.diff.scroll_to(ListOffset {
                    item_ix: ix,
                    offset_in_item: px(0.),
                });
            }
        }
        let (old, new) = (self.diff.item_count(), rows.len());
        if new > old {
            self.diff.splice(old..old, new - old);
        } else if new < old {
            self.diff.splice(new..old, 0);
        }
        self.diff.remeasure_items(0..new);
        self.drows = rows;
    }

    fn toggle_collapsed(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        cx.notify();
    }

    /// Clique numa linha: guarda (arquivo, nova, velha); removida só tem a velha.
    fn select_line(
        &mut self,
        path: &str,
        new: Option<u32>,
        old: Option<u32>,
        draft: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some((
            path.to_string(),
            new,
            if new.is_none() { old } else { None },
        ));
        if draft {
            self.new_draft(window, cx);
        }
        cx.notify();
    }

    fn is_selected(&self, path: &str, l: &diff::Line) -> bool {
        self.selected.as_ref().is_some_and(|(p, n, o)| {
            p == path
                && match l.new {
                    Some(x) => *n == Some(x),
                    None => n.is_none() && *o == l.old,
                }
        })
    }

    /// Os cartões presos a uma linha: threads e rascunhos da nova, ou da velha se ela foi removida.
    fn extras(
        &self,
        f: &FileDiff,
        l: &diff::Line,
        threads: &[(usize, &Value)],
        drafts: &[Draft],
        side: Side,
        cx: &mut Context<Self>,
    ) -> Vec<Div> {
        let mut out = Vec::new();
        let at = |d: &Value| -> (bool, Option<u64>, Option<u64>) {
            let p = &d["notes"][0]["position"];
            let file = p["new_path"] == f.path.as_str() || p["old_path"] == f.path.as_str();
            (file, p["new_line"].as_u64(), p["old_line"].as_u64())
        };
        match (l.new, l.kind) {
            (Some(n), _) => {
                for (ti, d) in threads.iter().filter(|(_, d)| {
                    let (file, new, _) = at(d);
                    file && new == Some(n as u64)
                }) {
                    out.push(self.thread(d, *ti, side, cx));
                }
                for d in drafts
                    .iter()
                    .filter(|d| d.reply_to.is_none() && d.file == f.path && d.line == Some(n))
                {
                    out.push(self.place(side, self.draft_box(d, cx)));
                }
            }
            (None, Kind::Del) => {
                let o = l.old.unwrap_or_default();
                for (ti, d) in threads.iter().filter(|(_, d)| {
                    let (file, new, old) = at(d);
                    file && new.is_none() && old == Some(o as u64)
                }) {
                    out.push(self.thread(d, *ti, side, cx));
                }
                for d in drafts.iter().filter(|d| {
                    d.reply_to.is_none() && d.file == f.path && d.line.is_none() && d.old == Some(o)
                }) {
                    out.push(self.place(side, self.draft_box(d, cx)));
                }
            }
            _ => {}
        }
        out
    }

    /// O "+" que aparece no hover da linha (o `.cm` do protótipo).
    fn comment_btn(
        &self,
        id: usize,
        left: f32,
        (path, new, old): (&str, Option<u32>, Option<u32>),
        sel: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = &self.theme;
        let path = path.to_string();
        div()
            .id(("mcm", id))
            .absolute()
            .left(px(left))
            .top(px(2.))
            .size(px(16.))
            .rounded(px(3.))
            .bg(t.accent)
            .text_color(rgb(0x282a36))
            .font_family(t.sans)
            .text_size(px(13.))
            .flex()
            .items_center()
            .justify_center()
            .invisible()
            .group_hover("ml", |s| s.visible())
            .when(sel, |x| x.visible())
            .on_click(
                cx.listener(move |this, _, w, cx| this.select_line(&path, new, old, true, w, cx)),
            )
            .child("+")
    }

    /// Uma metade do lado a lado: a velha à esquerda, a nova à direita (o `halfRow` do protótipo).
    fn half(
        &self,
        id: usize,
        f: &FileDiff,
        l: Option<&diff::Line>,
        left: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = self.theme.clone();
        let empty = || {
            div()
                .id(("mh", id))
                .w(relative(0.5))
                .flex_none()
                .h(px(20.))
                .bg(rgba(0x6272a414))
        };
        let Some(l) = l else { return empty() };
        if (left && l.kind == Kind::Add) || (!left && l.kind == Kind::Del) {
            return empty();
        }
        let del = left && l.kind == Kind::Del;
        let add = !left && l.kind == Kind::Add;
        let num = if left { l.old } else { l.new };
        let sel = self.is_selected(&f.path, l) && (del || !left);
        let (path, new, old) = (f.path.clone(), l.new, l.old);
        let can_comment = del || !left;
        div()
            .id(("mh", id))
            .group("ml")
            .relative()
            .w(relative(0.5))
            .flex_none()
            .flex()
            .h(px(20.))
            .cursor_pointer()
            .when(!left, |x| x.border_l_1().border_color(t.line))
            .when(del, |x| x.bg(rgba(0xff55551f)))
            .when(add, |x| x.bg(rgba(0x50fa7b1a)))
            .hover(|x| x.bg(rgba(0x44475a73)))
            .when(sel, |x| x.border_l_3().border_color(t.accent))
            .on_click(cx.listener(move |this, _, w, cx| {
                let n = if del { None } else { new };
                this.select_line(&path, n, old, false, w, cx)
            }))
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .pr(px(8.))
                    .text_right()
                    .text_color(t.comment)
                    .child(num.map(|x| x.to_string()).unwrap_or_default()),
            )
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .text_color(t.muted)
                    .child(if del {
                        "−"
                    } else if add {
                        "+"
                    } else {
                        ""
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pr(px(8.))
                    .child(self.code(&l.text)),
            )
            .when(can_comment, |x| {
                let n = if del { None } else { new };
                x.child(self.comment_btn(id, 22., (&f.path, n, old), sel, cx))
            })
    }

    fn desc_block(&self, row: &Row, o: &Opened, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let desc = Self::description(o);
        let open = !self.desc_closed.contains(&row.key());
        let key = row.key();
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(t.line)
            .font_family(t.sans)
            .child(
                div()
                    .id("mr-desc")
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .px(px(12.))
                    .py(px(6.))
                    .bg(t.panel)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.desc_closed.remove(&key) {
                            this.desc_closed.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(16.))
                            .text_center()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(if open { "▾" } else { "▸" }),
                    )
                    .child("Descrição")
                    .child(div().text_color(t.muted).child(row.who.clone())),
            )
            .when(open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .px(px(14.))
                        .py(px(8.))
                        .line_height(px(20.))
                        .text_size(px(13.5))
                        .children(Self::desc_lines(desc, &t)),
                )
            })
    }

    fn disc_block(&self, o: &Opened, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let general: Vec<(usize, &Value)> = Self::threads(o)
            .into_iter()
            .filter(|(_, d)| d["notes"][0]["position"].is_null())
            .collect();
        let n = general.len();
        let mut disc = div()
            .flex()
            .flex_col()
            .pb(px(6.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .px(px(12.))
                    .py(px(6.))
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.line)
                    .font_family(t.sans)
                    .child("Discussões")
                    .child(div().text_color(t.muted).child(format!(
                        "{n} gera{}, fora do código",
                        if n > 1 { "is" } else { "l" }
                    ))),
            );
        for (i, d) in &general {
            disc = disc.child(self.thread(d, *i, Side::Disc, cx));
        }
        disc
    }

    fn lone(&self, row: &Row, cx: &mut Context<Self>) -> Div {
        let lone: Vec<Draft> = self
            .drafts
            .get(&row.key())
            .map(|v| {
                v.iter()
                    .filter(|d| d.line.is_none() && d.old.is_none() && d.reply_to.is_none())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        div().flex().flex_col().children(
            lone.iter()
                .map(|d| self.place(Side::Unified, self.draft_box(d, cx))),
        )
    }

    fn gens_note(&self, o: &Opened, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let gens: Vec<&FileDiff> = o
            .files
            .iter()
            .filter(|f| Self::is_gen(o, &f.path))
            .collect();
        let names: Vec<&str> = gens
            .iter()
            .map(|f| f.path.rsplit('/').next().unwrap_or(&f.path))
            .collect();
        let n = gens.len();
        div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .items_center()
            .px(px(12.))
            .py(px(10.))
            .font_family(t.sans)
            .text_color(t.muted)
            .child(format!(
                "{n} arquivo{s} gerado{s} oculto{s} ({}).",
                names.join(", "),
                s = if n > 1 { "s" } else { "" }
            ))
            .child(
                button(&t, "gen-show", "Mostrar", false).on_click(cx.listener(|this, _, _, cx| {
                    this.hide_gen = false;
                    cx.notify();
                })),
            )
    }

    fn row_ctx<'a>(
        &'a self,
        f: &FileDiff,
        o: &'a Opened,
    ) -> (Vec<(usize, &'a Value)>, &'a [Draft]) {
        let drafts = self
            .open
            .as_ref()
            .and_then(|(row, _)| self.drafts.get(&row.key()))
            .map_or(&[][..], Vec::as_slice);
        let threads = Self::threads(o)
            .into_iter()
            .filter(|(_, d)| {
                let p = &d["notes"][0]["position"];
                p["new_path"] == f.path.as_str() || p["old_path"] == f.path.as_str()
            })
            .collect();
        (threads, drafts)
    }

    fn diff_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let (Some((row, Some(Ok(o)))), Some(&r)) = (&self.open, self.drows.get(ix)) else {
            return div().into_any_element();
        };
        match r {
            DRow::Desc => self.desc_block(row, o, cx),
            DRow::Disc => self.disc_block(o, cx),
            DRow::File(fi) => self.file_head(fi, &o.files[fi], o, cx),
            DRow::Hunk(fi, hi) => self.hunk_head(fi, hi, &o.files[fi], cx),
            DRow::Line(fi, hi, li) => self.line_row(fi, hi, li, &o.files[fi], o, cx),
            DRow::Pair(fi, hi, l, r) => self.pair_row(fi, hi, (l, r), &o.files[fi], o, cx),
            DRow::Orphans(fi) => self.orphans(&o.files[fi], o, cx),
            DRow::Lone => self.lone(row, cx),
            DRow::Gens => self.gens_note(o, cx),
        }
        .w_full()
        .into_any_element()
    }

    fn file_head(&self, fi: usize, f: &FileDiff, o: &Opened, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let (a, r) = f.counts();
        let collapsed = self.collapsed.contains(&f.path);
        let (seen, changed) = (self.is_viewed(o, &f.path), self.changed_since(o, &f.path));
        let vpath = f.path.clone();
        let n_threads = Self::threads(o)
            .iter()
            .filter(|(_, d)| d["notes"][0]["position"]["new_path"] == f.path.as_str())
            .count();
        let path = f.path.clone();
        div().flex().flex_col().child(
            div()
                .id(("fh", fi))
                .flex()
                .gap(px(10.))
                .items_center()
                .px(px(12.))
                .py(px(6.))
                .bg(t.panel)
                .border_t_1()
                .border_b_1()
                .border_color(t.line)
                .font_family(t.sans)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_collapsed(&path, cx)))
                .child(
                    div()
                        .w(px(16.))
                        .flex_none()
                        .text_center()
                        .text_size(px(12.))
                        .font_family(t.mono.clone())
                        .text_color(t.muted)
                        .child(if collapsed { "▸" } else { "▾" }),
                )
                .child(div().font_family(t.mono.clone()).child(f.path.clone()))
                .when(f.new, |x| x.child(pill(&t, "novo", t.accent, false)))
                .when(Self::is_gen(o, &f.path), |x| {
                    x.child(pill(&t, "gerado", t.comment, true))
                })
                .child(div().text_color(t.ok).child(format!("+{a}")))
                .child(div().text_color(red()).child(format!("−{r}")))
                .child(div().flex_1())
                .when(collapsed, |x| {
                    let what = if seen { "visto" } else { "colapsado" };
                    x.child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(if n_threads > 0 {
                                format!("{n_threads} thread(s) · {what}")
                            } else {
                                what.into()
                            }),
                    )
                })
                .when(changed, |x| {
                    x.child(pill(&t, "mudou desde que você viu", t.orange, false))
                })
                .child(
                    div()
                        .id(("mr-vw", fi))
                        .flex()
                        .gap(px(5.))
                        .items_center()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_viewed(&vpath, false, cx)
                        }))
                        .child(checkbox(&t, seen))
                        .child("Visto"),
                ),
        )
    }

    fn hunk_head(&self, fi: usize, hi: usize, f: &FileDiff, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let h = &f.hunks[hi];
        let path = f.path.clone();
        div()
            .flex()
            .items_center()
            .px(px(12.))
            .text_color(t.comment)
            .bg(rgba(0x6272a41f))
            .child(h.header.clone())
            .child(div().flex_1())
            .child(
                div()
                    .id(("mr-ask", fi * 1000 + hi))
                    .font_family(t.mono.clone())
                    .text_size(px(11.5))
                    .px(px(6.))
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(4.))
                    .bg(t.raise)
                    .text_color(t.muted)
                    .cursor_pointer()
                    .hover(|d| d.text_color(t.fg).border_color(t.muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(c) = this.ask_ctx(&path, Some(hi), 0) {
                            cx.emit(Event::AskClaude(c));
                        }
                    }))
                    .child("?"),
            )
    }

    fn line_row(
        &self,
        fi: usize,
        hi: usize,
        li: usize,
        f: &FileDiff,
        o: &Opened,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme.clone();
        let (threads, drafts) = self.row_ctx(f, o);
        let l = &f.hunks[hi].lines[li];
        let sel = self.is_selected(&f.path, l);
        let (path, n, old) = (f.path.clone(), l.new, l.old);
        let bg = match l.kind {
            Kind::Add => Some(Hsla::from(rgba(0x50fa7b1a))),
            Kind::Del => Some(Hsla::from(rgba(0xff55551f))),
            Kind::Context => None,
        };
        let sign = match l.kind {
            Kind::Add => "+",
            Kind::Del => "-",
            Kind::Context => "",
        };
        let num = |v: Option<u32>| {
            div()
                .w(px(40.))
                .flex_none()
                .pr(px(8.))
                .text_right()
                .text_color(t.comment)
                .child(v.map(|x| x.to_string()).unwrap_or_default())
        };
        let id = (fi * 1000 + hi) * 10_000 + li;
        let line = div()
            .id(("ml", id))
            .group("ml")
            .relative()
            .flex()
            .h(px(20.))
            .cursor_pointer()
            .when_some(bg, |x, c| x.bg(c))
            .hover(|x| x.bg(rgba(0x44475a73)))
            .when(sel, |x| x.border_l_3().border_color(t.accent))
            .on_click(
                cx.listener(move |this, _, w, cx| this.select_line(&path, n, old, false, w, cx)),
            )
            .child(num(l.old))
            .child(num(l.new))
            .child(div().w(px(18.)).flex_none().text_color(t.muted).child(sign))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pr(px(8.))
                    .child(self.code(&l.text)),
            )
            .child(self.comment_btn(id, 62., (&f.path, l.new, l.old), sel, cx));
        div().flex().flex_col().child(line).children(self.extras(
            f,
            l,
            &threads,
            drafts,
            Side::Unified,
            cx,
        ))
    }

    fn pair_row(
        &self,
        fi: usize,
        hi: usize,
        (l, r): (Option<usize>, Option<usize>),
        f: &FileDiff,
        o: &Opened,
        cx: &mut Context<Self>,
    ) -> Div {
        let (threads, drafts) = self.row_ctx(f, o);
        let lines = &f.hunks[hi].lines;
        // a nova é única por par; sem nova, a velha é uma removida que nenhum outro par usa
        let id = (fi * 1000 + hi) * 10_000 + r.or(l).unwrap_or_default();
        let (l, r) = (l.map(|i| &lines[i]), r.map(|i| &lines[i]));
        let mut out = div().flex().flex_col().child(
            div()
                .flex()
                .child(self.half(id * 2, f, l, true, cx))
                .child(self.half(id * 2 + 1, f, r, false, cx)),
        );
        if let Some(l) = l.filter(|l| l.kind == Kind::Del) {
            out = out.children(self.extras(f, l, &threads, drafts, Side::Left, cx));
        }
        if let Some(r) = r {
            out = out.children(self.extras(f, r, &threads, drafts, Side::Right, cx));
        }
        out
    }

    fn orphans(&self, f: &FileDiff, o: &Opened, cx: &mut Context<Self>) -> Div {
        let (_, drafts) = self.row_ctx(f, o);
        div().flex().flex_col().children(
            drafts
                .iter()
                .filter(|d| {
                    d.reply_to.is_none()
                        && d.file == f.path
                        && d.line.is_some_and(|l| {
                            !f.hunks
                                .iter()
                                .flat_map(|h| &h.lines)
                                .any(|x| x.new == Some(l))
                        })
                })
                .map(|d| self.place(Side::Unified, self.draft_box(d, cx))),
        )
    }

    /// A coluna "Arquivos do MR" (o `renderMRFiles` do protótipo), com os gerados numa seção recolhível.
    pub fn files_panel(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (row, Some(Ok(o))) = self.open.as_ref()? else {
            return None;
        };
        let t = self.theme.clone();
        let drafts = self.drafts.get(&row.key()).cloned().unwrap_or_default();
        let mut col = div()
            .id("mr-files")
            .w(px(self.files_w))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .min_h(px(30.))
                    .flex()
                    .items_center()
                    .px(px(10.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .border_b_1()
                    .border_color(t.line)
                    .child("ARQUIVOS DO MR")
                    .child(div().flex_1())
                    .child(div().font_family(t.mono.clone()).child(format!(
                        "{}/{} vistos",
                        o.files.iter().filter(|f| self.is_viewed(o, &f.path)).count(),
                        o.files.len()
                    ))),
            );
        let row_of = |i: usize, f: &FileDiff, cx: &mut Context<Self>| {
            let (a, r) = f.counts();
            let n = drafts
                .iter()
                .filter(|d| d.file == f.path && d.reply_to.is_none())
                .count();
            let (dir, name) = f
                .path
                .rsplit_once('/')
                .map_or((String::new(), f.path.clone()), |(d, n)| {
                    (format!("{d}/"), n.into())
                });
            let collapsed = self.collapsed.contains(&f.path);
            let (seen, changed) = (self.is_viewed(o, &f.path), self.changed_since(o, &f.path));
            let (p1, p2) = (f.path.clone(), f.path.clone());
            div()
                .id(("mf", i))
                .when(seen, |d| d.opacity(0.55))
                .flex()
                .gap(px(6.))
                .px(px(12.))
                .py(px(3.))
                .cursor_pointer()
                .hover(|d| d.bg(t.raise))
                .on_click(cx.listener(move |this, _, _, cx| this.scroll_to_file(&p1, cx)))
                .child(
                    div()
                        .id(("mtw", i))
                        .w(px(12.))
                        .flex_none()
                        .text_size(px(11.))
                        .font_family(t.mono.clone())
                        .text_color(t.muted)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_collapsed(&p2, cx)
                        }))
                        .child(if seen {
                            "✓"
                        } else if collapsed {
                            "▸"
                        } else {
                            "▾"
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family(t.mono.clone())
                        .text_size(px(13.))
                        .child(div().text_color(t.muted).child(dir))
                        .child(name),
                )
                .child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .text_color(if n > 0 || changed { yellow() } else { t.muted })
                        .child(if n > 0 {
                            format!("{n} rasc.")
                        } else if changed {
                            "mudou".into()
                        } else {
                            format!("+{a} −{r}")
                        }),
                )
        };
        let gens: Vec<(usize, &FileDiff)> = o
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| Self::is_gen(o, &f.path))
            .collect();
        for (i, f) in o.files.iter().enumerate() {
            if !Self::is_gen(o, &f.path) {
                col = col.child(row_of(i, f, cx));
            }
        }
        if !gens.is_empty() {
            col = col.child(
                div()
                    .id("mf-gen")
                    .flex()
                    .justify_between()
                    .px(px(12.))
                    .pt(px(6.))
                    .pb(px(2.))
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.gen_open = !this.gen_open;
                        cx.notify();
                    }))
                    .child(format!(
                        "{} GERADOS ({})",
                        if self.gen_open { "▾" } else { "▸" },
                        gens.len()
                    ))
                    .child(if self.hide_gen { "OCULTOS NO DIFF" } else { "" }),
            );
            if self.gen_open {
                for (i, f) in gens {
                    col = col.child(row_of(i, f, cx));
                }
            }
        }
        Some(col)
    }

    fn mr(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let Some((row, opened)) = &self.open else {
            return div();
        };
        let ci = Self::ci_label(self.ci.get(&row.key()).copied(), &t);
        let o = opened.as_ref().and_then(|o| o.as_ref().ok());
        let detail = o.map(|o| &o.detail);
        let merged = detail.is_some_and(|d| d["state"] == "merged");
        let scheduled = detail.is_some_and(|d| d["merge_when_pipeline_succeeds"] == true);
        let approved = o.is_some_and(|o| o.approvals["user_has_approved"] == true);
        let can_approve = o.is_some_and(|o| o.approvals["user_can_approve"] == true) || approved;
        let blocker = detail.and_then(mrs::merge_blocker);
        let busy = self.busy;
        let header = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(14.))
            .pt(px(10.))
            .border_b_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(12.))
                    .items_baseline()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_color(t.muted)
                            .child(format!("{} !{}", row.proj(), row.iid)),
                    )
                    .child(div().text_color(t.muted).child(row.who.clone()))
                    .child(div().text_color(ci.1).child(ci.0))
                    .when(merged, |d| d.child(pill(&t, "mergeado", t.ok, false)))
                    .when(scheduled && !merged, |d| {
                        d.child(pill(&t, "merge quando o pipeline passar", t.orange, false))
                    })
                    .when(approved, |d| {
                        d.child(pill(&t, "aprovado por você", t.ok, false))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .items_center()
                    .pb(px(6.))
                    .child(
                        button(&t, "mr-back", "← Inbox", false)
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    )
                    .child({
                        let r = row.clone();
                        button(&t, "mr-web", "Abrir no GitLab ↗", false)
                            .on_click(cx.listener(move |this, _, _, _| this.open_gitlab(&r)))
                    })
                    .child({
                        let url = row.web_url.clone();
                        button(&t, "mr-copy", "Copiar link", false).on_click(cx.listener(
                            move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                                cx.emit(Event::Toast(format!("Copiado: {url}")));
                            },
                        ))
                    })
                    .child(
                        button(&t, "mr-reload", "↻ Atualizar", false)
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(div().flex_1())
                    .when_some(blocker.filter(|_| !merged && !scheduled), |d, why| {
                        d.child(
                            div()
                                .text_size(px(13.))
                                .text_color(t.orange)
                                .child(format!("⚠ {why}")),
                        )
                    })
                    .when(o.is_some() && !merged, |d| {
                        d.child(
                            button(
                                &t,
                                "mr-approve",
                                if approved {
                                    "Remover aprovação"
                                } else {
                                    "✓ Aprovar"
                                },
                                false,
                            )
                            .when(busy || !can_approve, |b| b.opacity(0.45))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.busy && can_approve {
                                        this.toggle_approve(cx)
                                    }
                                },
                            )),
                        )
                        .child(
                            button(
                                &t,
                                "mr-merge",
                                if scheduled {
                                    "Cancelar merge agendado"
                                } else {
                                    "Merge…"
                                },
                                false,
                            )
                            .when(busy || (blocker.is_some() && !scheduled), |b| {
                                b.opacity(0.45)
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.busy && (blocker.is_none() || scheduled) {
                                        this.ask_merge(cx)
                                    }
                                },
                            )),
                        )
                    })
                    .child(
                        button(&t, "mr-rev", "Revisar com Claude ⇧R", true)
                            .on_click(cx.listener(|this, _, _, cx| this.start_review(cx))),
                    ),
            );
        let body = match &opened {
            None => div().p(px(24.)).text_color(t.muted).child("Lendo o MR…"),
            Some(Err(e)) => self.empty("Não deu para abrir o MR".into(), e.clone()),
            Some(Ok(o)) => {
                let gens: Vec<&FileDiff> = o
                    .files
                    .iter()
                    .filter(|f| Self::is_gen(o, &f.path))
                    .collect();
                let seg = |id: &'static str, label: &'static str, on: bool, split: bool| {
                    div()
                        .id(id)
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(if on { t.fg } else { t.muted })
                        .when(on, |d| d.bg(t.accent_soft))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.split != split {
                                this.toggle_split(cx)
                            }
                        }))
                        .child(label)
                };
                let hide = self.hide_gen;
                let tool =
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .items_center()
                        .px(px(10.))
                        .py(px(6.))
                        .bg(t.panel)
                        .border_b_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .flex()
                                .gap(px(2.))
                                .child(seg("lay-split", "Lado a lado", self.split, true))
                                .child(seg("lay-uni", "Unificado", !self.split, false)),
                        )
                        .child(
                            div()
                                .id("gen-t")
                                .flex()
                                .gap(px(6.))
                                .items_center()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.hide_gen = !this.hide_gen;
                                    cx.notify();
                                }))
                                .child(checkbox(&t, hide))
                                .child(format!("Ocultar gerados ({})", gens.len())),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .child(format!(
                                    "{}/{} vistos",
                                    o.files
                                        .iter()
                                        .filter(|f| self.is_viewed(o, &f.path))
                                        .count(),
                                    o.files.len()
                                )),
                        )
                        .child(button(&t, "exp-all", "Expandir todos", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.collapsed.clear();
                                cx.notify();
                            }),
                        ))
                        .child(button(&t, "col-all", "Colapsar todos", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some((_, Some(Ok(o)))) = &this.open {
                                    this.collapsed =
                                        o.files.iter().map(|f| f.path.clone()).collect();
                                }
                                cx.notify();
                            }),
                        ));
                let scroll = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .font_family(t.mono.clone())
                    .text_size(px(13.5))
                    .line_height(px(22.))
                    .child(
                        list(
                            self.diff.clone(),
                            cx.processor(|this, ix, _, cx| this.diff_row(ix, cx)),
                        )
                        .flex_1()
                        .pb(px(40.)),
                    );
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(tool)
                    .child(scroll)
            }
        };
        let pend = self.pending().len();
        let s = if pend > 1 { "s" } else { "" };
        let pubbar = div()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .items_center()
            .px(px(12.))
            .py(px(8.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .when(pend > 0, |d| {
                d.child(
                    div()
                        .text_color(yellow())
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("{pend} rascunho{s} não publicado{s}")),
                )
                .child(div().text_color(t.muted).child("só você vê"))
            })
            .when(pend == 0, |d| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.muted)
                        .child("C numa linha comenta; ⇧R pede revisão ao Claude."),
                )
            })
            .child(div().flex_1())
            .child(
                button(&t, "mr-dall", "Descartar todos", false)
                    .text_color(red())
                    .when(pend == 0, |d| d.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.discard_all(cx))),
            )
            .child(
                button(&t, "mr-pub", "Publicar review Ctrl+⏎", true)
                    .when(pend == 0 || self.posting, |d| d.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.ask_publish(cx))),
            );
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .child(pubbar)
    }

    fn drawer(&self, cx: &mut Context<Self>) -> Option<Div> {
        let d = self.drawer.as_ref()?;
        let t = self.theme.clone();
        let fieldset = |legend: &str| {
            div()
                .border_1()
                .border_color(t.line)
                .rounded(px(6.))
                .px(px(12.))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child(legend.to_string()),
                )
        };
        let radio = |on: bool| {
            div()
                .size(px(12.))
                .rounded_full()
                .border_1()
                .border_color(if on { t.accent } else { t.line })
                .when(on, |d| d.bg(t.accent))
        };
        let (body, foot) = match &d.phase {
            Phase::Setup => {
                let can_inc = mrs::last_seen(&self.seen, &d.row.path, d.row.iid)
                    .is_some_and(|s| s != d.row.sha);
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .when_some(d.ci_warn, |b, w| {
                        b.child(div().text_color(red()).child(format!(
                            "{w} em {} !{}. Revisar código que o CI ainda pode reprovar costuma gerar achados repetidos.",
                            d.row.proj(),
                            d.row.iid
                        )))
                    })
                    .child(
                        fieldset("Escopo")
                            .child(
                                div()
                                    .id("sc-all")
                                    .flex()
                                    .gap(px(8.))
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(d) = this.drawer.as_mut() {
                                            d.incremental = false;
                                        }
                                        cx.notify();
                                    }))
                                    .child(radio(!d.incremental))
                                    .child("MR inteiro"),
                            )
                            .child(
                                div()
                                    .id("sc-inc")
                                    .flex()
                                    .gap(px(8.))
                                    .items_center()
                                    .when(!can_inc, |x| x.opacity(0.45))
                                    .when(can_inc, |x| {
                                        x.cursor_pointer().on_click(cx.listener(
                                            |this, _, _, cx| {
                                                if let Some(d) = this.drawer.as_mut() {
                                                    d.incremental = true;
                                                }
                                                cx.notify();
                                            },
                                        ))
                                    })
                                    .child(radio(d.incremental))
                                    .child("Só commits novos desde a última revisão"),
                            ),
                    )
                    .child(
                        fieldset("Contexto enviado").child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .font_family(t.mono.clone())
                                .text_size(px(13.5))
                                .child(
                                    "· descrição do MR e todos os comentários existentes (dedup)",
                                )
                                .child("· rubrica ~/.claude/mr-review-rubric.md")
                                .child("· CLAUDE.md e skills do repo"),
                        ),
                    )
                    .child(
                        div()
                            .text_color(t.muted)
                            .child("Os achados viram rascunhos. Nada é publicado sem você."),
                    );
                let foot = div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        button(&t, "rv-x", "Cancelar", false)
                            .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx))),
                    )
                    .child(
                        button(&t, "rv-go", "Iniciar revisão", true)
                            .on_click(cx.listener(|this, _, _, cx| this.run_review(cx))),
                    );
                (body, foot)
            }
            Phase::Run(steps) => {
                let mut b = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .font_family(t.mono.clone())
                    .text_size(px(13.5));
                for (k, s) in steps {
                    let (g, c, pl) = match *k {
                        "ok" => ("✓", t.ok, 0.),
                        "run" => ("⟳", yellow(), 0.),
                        _ => ("↳", t.muted, 18.),
                    };
                    b = b.child(div().pl(px(pl)).text_color(c).child(format!("{g} {s}")));
                }
                b = b.child(
                    div()
                        .text_color(yellow())
                        .child("⟳ o Claude está lendo o MR…"),
                );
                let foot = div().child(
                    button(&t, "rv-stop", "Parar", false)
                        .text_color(red())
                        .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx))),
                );
                (b, foot)
            }
            Phase::Done(f, cost) => {
                let on = f.iter().filter(|x| x.1).count();
                let mut b = div().flex().flex_col().gap(px(8.));
                if f.is_empty() {
                    b = b.child(div().text_color(t.muted).child("Nada novo para comentar: o que o Claude achou já estava nos comentários do MR."));
                }
                for (i, (x, sel)) in f.iter().enumerate() {
                    let sc = match x.severity.as_str() {
                        "blocker" | "major" => red(),
                        "minor" => t.orange,
                        _ => t.muted,
                    };
                    b = b.child(
                        div()
                            .id(("fd", i))
                            .flex()
                            .gap(px(10.))
                            .px(px(10.))
                            .py(px(8.))
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(!sel, |d| d.opacity(0.55))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Drawer {
                                    phase: Phase::Done(f, _),
                                    ..
                                }) = this.drawer.as_mut()
                                {
                                    f[i].1 = !f[i].1;
                                }
                                cx.notify();
                            }))
                            .child(checkbox(&t, *sel))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap(px(8.))
                                            .items_center()
                                            .font_family(t.mono.clone())
                                            .text_size(px(13.))
                                            .child(
                                                div()
                                                    .px(px(6.))
                                                    .rounded(px(3.))
                                                    .border_1()
                                                    .border_color(sc)
                                                    .text_color(sc)
                                                    .text_size(px(12.))
                                                    .child(x.severity.clone()),
                                            )
                                            .child(format!(
                                                "{}:{}",
                                                x.file,
                                                x.line.map_or("–".into(), |l| l.to_string())
                                            ))
                                            .when(x.outside, |d| {
                                                d.child(
                                                    div()
                                                        .font_family(t.sans)
                                                        .text_color(t.orange)
                                                        .child(
                                                            "⚠ fora do diff, vai como nota geral",
                                                        ),
                                                )
                                            }),
                                    )
                                    .child(x.body.clone()),
                            ),
                    );
                }
                let foot = div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .child(format!("US$ {}", crate::sidebar::brl(*cost))),
                    )
                    .child(div().flex_1())
                    .child(
                        button(&t, "rv-x", "Descartar tudo", false)
                            .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx))),
                    )
                    .child(
                        button(
                            &t,
                            "rv-mk",
                            &format!("Criar {on} rascunho{}", if on != 1 { "s" } else { "" }),
                            true,
                        )
                        .when(on == 0, |d| d.opacity(0.45))
                        .when(on > 0, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.make_drafts(cx)))
                        }),
                    );
                (b, foot)
            }
        };
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(self.drawer_w))
                .child(
                    theme::resize_handle("rz-drawer", Drag::Drawer, &t)
                        .absolute()
                        .left_0()
                        .top_0(),
                )
                .bg(t.raise)
                .border_l_1()
                .border_color(t.line)
                .shadow_lg()
                .flex()
                .flex_col()
                .occlude()
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .px(px(14.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(t.line)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Revisão Claude"),
                        )
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .text_color(t.muted)
                                .child(format!("{} !{}", d.row.proj(), d.row.iid)),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("rv-c")
                                .cursor_pointer()
                                .text_color(t.muted)
                                .on_click(cx.listener(|this, _, _, cx| this.close_drawer(cx)))
                                .child("×"),
                        ),
                )
                .child(
                    div()
                        .id("rv-db")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(14.))
                        .py(px(12.))
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .px(px(14.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(t.line)
                        .child(foot),
                ),
        )
    }

    pub fn in_mr(&self) -> bool {
        self.open.is_some()
    }
}

impl Render for MrsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        self.sync_rows();
        let center = if self.open.is_some() {
            self.mr(cx)
        } else {
            self.inbox(cx)
        };
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<Drag>, _, cx| {
                let (b, x) = (e.bounds, e.event.position.x);
                let w = f32::from(b.size.width);
                match e.drag(cx) {
                    Drag::Files => {
                        this.files_w = f32::from(x - b.left()).clamp(180., (w - 300.).max(180.))
                    }
                    Drag::Drawer => {
                        this.drawer_w = f32::from(b.right() - x).clamp(320., (w - 200.).max(320.))
                    }
                }
                cx.notify();
            }))
            .when_some(self.files_panel(cx), |d, files| {
                d.child(files)
                    .child(theme::resize_handle("rz-files", Drag::Files, &t))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(center),
            )
            .children(self.drawer(cx))
    }
}
