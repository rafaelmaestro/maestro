use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    changes::{self, Changes},
    feature::{self, Inventory, Stack},
    form::{FieldEvent, TextField},
    herdr::{self, Agent, Herdr, Status, Workspace},
    mascot::{Kind, Note},
    mr::{self, Fetch, Mr},
    perms,
    session::{Sessions, Usage},
    theme::Theme,
    tour::{self, Anchors},
};

pub enum SidebarEvent {
    NewFeature,
    /// clique num arquivo de "Mudanças": abre a revisão nele
    OpenReview(String),
    /// clique em "Arquivos": abre o arquivo no editor
    OpenFile(String),
    /// o MR da feature desta worktree acabou de mergear: (worktree, iid, branch)
    Merged(PathBuf, u64, String),
    /// um agente passou a pedir permissão ou terminou (o `notify(f)` do protótipo)
    Notify(Note),
    /// a mesma permissão foi pedida vezes demais: sugerir a regra
    Learn(perms::Suggestion),
    /// o Maestro escreveu no vault (Histórico, daily)
    Vault(Result<crate::vault::Write, String>),
    Audit,
    Picked,
    /// "⎇ repo · N" de uma feature multi-repo: a revisão daquela worktree
    OpenReviewAt(PathBuf),
    /// a pílula da pilha: (worktree da filha, o que fazer)
    Stack(PathBuf, Stack),
    /// botão direito na aba Arquivos; o shell mexe no disco e nas abas do editor
    Fs(FsOp),
    Toast(String),
}

#[derive(Clone, Debug)]
pub enum FsOp {
    /// (caminho, é pasta)
    Create(String, bool),
    Rename(String, String),
    Delete(String, bool),
    /// (origem, pasta de destino; "" é a raiz)
    Copy(String, String),
}

/// Um item arrastado na árvore: o caminho dele.
#[derive(Clone)]
pub struct DragPath(String);

struct DragChip(String, Theme);

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = &self.1;
        div()
            .px(px(8.))
            .py(px(2.))
            .bg(t.raise)
            .border_1()
            .border_color(t.accent)
            .rounded(px(4.))
            .font_family(t.mono.clone())
            .text_size(px(13.))
            .text_color(t.fg)
            .child(self.0.clone())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    NewFile,
    NewDir,
    Cut,
    Copy,
    Paste,
    Duplicate,
    CopyPath,
    Rename,
    Delete,
}

impl Act {
    fn label(self) -> (&'static str, &'static str) {
        match self {
            Act::NewFile => ("Novo arquivo", ""),
            Act::NewDir => ("Nova pasta", ""),
            Act::Cut => ("Recortar", "Ctrl+X"),
            Act::Copy => ("Copiar", "Ctrl+C"),
            Act::Paste => ("Colar", "Ctrl+V"),
            Act::Duplicate => ("Duplicar", ""),
            Act::CopyPath => ("Copiar caminho", ""),
            Act::Rename => ("Renomear", "F2"),
            Act::Delete => ("Excluir", "Delete"),
        }
    }
}

fn parent(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

/// O nome sendo digitado na árvore: novo item dentro da pasta (é pasta) ou o item renomeado.
enum Naming {
    New(String, bool),
    Rename(String),
}

impl EventEmitter<SidebarEvent> for Sidebar {}

/// Lateral "Features" do protótipo: agentes por repositório e, embaixo, os workspaces ("Terminais").
pub struct Sidebar {
    herdr: Arc<Herdr>,
    kick: futures::channel::mpsc::UnboundedSender<()>,
    pub agents: Vec<Agent>,
    pub workspaces: Vec<Workspace>,
    /// cwd de cada workspace do herdr (o pane em foco nele)
    ws_cwd: HashMap<String, String>,
    /// as abas de trabalho, na ordem da faixa; `cur` é a da lateral
    pub works: Vec<Work>,
    pub cur: usize,
    next_work: u64,
    /// as abas em disco, por sessão do herdr (os ids `w1`… são dela)
    works_file: PathBuf,
    /// "Nova aba" pediu: o próximo workspace novo em foco abre um trabalho, não entra no atual
    pub fresh: bool,
    inited: bool,
    pub offline: bool,
    since: HashMap<String, (Status, Instant)>,
    pub usage: HashMap<String, Usage>,
    pub inventories: Vec<Inventory>,
    changes: Option<Changes>,
    /// aba Arquivos à frente e a lista dela (lida no clique da aba)
    files: Option<Vec<String>>,
    /// MR de cada feature (pela worktree), o `mrSum` do card
    mrs: HashMap<PathBuf, Mr>,
    learned: HashSet<(PathBuf, String)>,
    /// comentários já mandados ao Claude, por worktree (`<id>.sent`)
    sent: HashMap<PathBuf, HashSet<u64>>,
    /// o pedido de permissão de cada pane bloqueado ("pediu permissão: npm test")
    asks: HashMap<String, String>,
    /// o card "Auditoria": (rodando, k, m, aberta, pronta e não vista)
    pub audit: (bool, String, String, bool, bool),
    /// o pane em foco, com ou sem Claude: dá o repo da lateral num terminal comum
    focused_cwd: Option<String>,
    /// aba Arquivos: as pastas abertas da árvore
    expanded: HashSet<String>,
    /// aba Arquivos: o menu do botão direito (item, é pasta, onde); item vazio é a raiz
    menu: Option<(String, bool, Point<Pixels>)>,
    menu_focus: FocusHandle,
    tree_focus: FocusHandle,
    /// item selecionado na árvore (caminho, é pasta): teclado e colar
    sel: Option<(String, bool)>,
    /// (caminho, recortado) de Ctrl+C/Ctrl+X na árvore
    clip: Option<(String, bool)>,
    naming: Option<(Naming, Entity<TextField>)>,
    /// por worktree principal: os outros repos da feature (nome, worktree, arquivos mudados)
    extra: HashMap<PathBuf, Vec<(String, PathBuf, usize)>>,
    /// por worktree principal da filha: o que a pilha pede
    stacks: HashMap<PathBuf, Stack>,
    /// (arquivo do editor, sujo, aba do editor à frente)
    open: Option<(String, bool, bool)>,
    sessions: Arc<Mutex<Sessions>>,
    anchors: Anchors,
    theme: Theme,
    _poll: Task<()>,
}

/// Aonde um item da fila leva.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    Pane(String),
    /// (pane, worktree): a feature e a aba MR dela
    Mr(String, PathBuf),
    /// (pane, worktree): a feature e o painel Revisar
    Review(String, PathBuf),
    Compare(String),
    Audit,
}

/// Um item do `attention()` do protótipo; `p` é a prioridade (0 = permissão … 4 = daily).
#[derive(Clone, Debug, PartialEq)]
pub struct AttItem {
    pub p: u8,
    pub tag: &'static str,
    pub text: String,
    pub go: Go,
}

/// agentes, workspaces e o cwd de cada workspace, lidos do herdr a cada polling
type HerdrState = (Vec<Agent>, Vec<Workspace>, HashMap<String, String>);

/// Uma aba de trabalho: os workspaces do herdr dela e o último em foco.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Work {
    id: u64,
    pub ws: Vec<String>,
    pub last: String,
}

pub struct Repo {
    pub name: String,
    pub path: String,
    pub agents: Vec<Agent>,
}

struct Row {
    id: String,
    glyph: &'static str,
    glyph_color: Hsla,
    b: String,
    k: String,
    m: String,
    m_color: Hsla,
    m2: Option<(String, String, bool)>,
    mr: Option<Mr>,
    on: bool,
    unseen: Option<Hsla>,
    dim: bool,
    /// filha numa pilha: indentada sob a mãe
    child: bool,
    /// linhas embaixo do card: os outros repos e a pílula da pilha
    extra: Vec<AnyElement>,
}

impl Sidebar {
    pub fn new(herdr: Arc<Herdr>, theme: Theme, anchors: Anchors, cx: &mut Context<Self>) -> Self {
        // ponytail: polling de 500 ms (status não tem evento global no protocolo 19, só por pane);
        // trocar por events.subscribe por pane se a latência ou a CPU incomodarem
        let h = herdr.clone();
        let (kick, mut kicked) = futures::channel::mpsc::unbounded::<()>();
        let poll = cx.spawn(async move |this, cx| {
            // troca de aba acorda o polling: as Mudanças da aba nova não esperam os 3 s
            let mut heavy = false;
            for tick in 0u64.. {
                let h = h.clone();
                let state = cx
                    .background_executor()
                    .spawn(async move {
                        let ws = h.workspaces()?;
                        let panes = h.panes().ok();
                        let cwd = panes.as_ref().and_then(|p| {
                            let w = ws
                                .iter()
                                .find(|w| w.focused)
                                .map(|w| w.workspace_id.as_str());
                            herdr::focused_cwd(p, w)
                        });
                        let cwds = panes
                            .as_ref()
                            .map(herdr::workspace_cwds)
                            .unwrap_or_default();
                        Ok(((h.agents()?, ws, cwds), cwd))
                    })
                    .await;
                let (state, cwd) = match state {
                    Ok((s, c)) => (Ok(s), c),
                    Err(e) => (Err(e), None),
                };
                // "há N min" envelhece sozinho: redesenha a cada 30 s mesmo sem mudança
                let alive = this.update(cx, |this, cx| {
                    let (mut changed, notes) = this.apply(state);
                    if this.focused_cwd != cwd {
                        this.focused_cwd = cwd;
                        changed = true;
                        heavy = true;
                    }
                    for n in notes {
                        cx.emit(SidebarEvent::Notify(n));
                    }
                    if changed || tick % 60 == 0 {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
                // custo/contexto e Mudanças são mais caros (JSONL e git): a cada 3 s
                if tick % 6 == 0 || std::mem::take(&mut heavy) {
                    let Ok((jobs, root, sessions, merged)) = this.read_with(cx, |this, _| {
                        let jobs: Vec<_> = this
                            .agents
                            .iter()
                            .map(|a| (a.pane_id.clone(), a.cwd.clone(), title(a)))
                            .collect();
                        let merged: HashSet<PathBuf> = this
                            .mrs
                            .iter()
                            .filter(|(_, m)| m.merged)
                            .map(|(w, _)| w.clone())
                            .collect();
                        (jobs, this.top_worktree(), this.sessions.clone(), merged)
                    }) else {
                        break;
                    };
                    let (usage, changes, inventories, learn, extra, stacks) = cx
                        .background_executor()
                        .spawn(async move {
                            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                            let mut s = sessions.lock().unwrap();
                            let usage: HashMap<_, _> = jobs
                                .into_iter()
                                .filter_map(|(pane, cwd, title)| {
                                    Some((pane.clone(), s.usage(&home, &pane, &cwd, &title)?))
                                })
                                .collect();
                            let invs = feature::inventories();
                            let (extra, stacks) = stack_and_repos(&invs, &merged);
                            (
                                usage,
                                root.and_then(|r| changes::changes(&r)),
                                invs,
                                perms::pending(),
                                extra,
                                stacks,
                            )
                        })
                        .await;
                    let Ok(invs) = this.read_with(cx, |this, _| this.inventories.clone()) else {
                        break;
                    };
                    let sent: HashMap<PathBuf, HashSet<u64>> = cx
                        .background_executor()
                        .spawn(async move {
                            invs.iter()
                                .map(|i| {
                                    let ids = feature::state_lines(&i.id, "sent")
                                        .iter()
                                        .filter_map(|l| l.parse().ok())
                                        .collect();
                                    (i.worktrees[0].path.clone(), ids)
                                })
                                .collect()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        for s in learn {
                            if this.learned.insert((s.repo.clone(), s.rule.clone())) {
                                cx.emit(SidebarEvent::Learn(s));
                            }
                        }
                        if this.sent != sent {
                            this.sent = sent;
                            cx.notify();
                        }
                        if this.usage != usage
                            || this.changes != changes
                            || this.inventories != inventories
                            || this.extra != extra
                            || this.stacks != stacks
                        {
                            this.usage = usage;
                            this.changes = changes;
                            this.inventories = inventories;
                            this.extra = extra;
                            this.stacks = stacks;
                            cx.notify();
                        }
                    });
                }
                // ponytail: MRs a cada 60 s (4 chamadas do glab por feature); webhook não chega aqui
                if tick % 120 == 4 {
                    let Ok(jobs) = this.read_with(cx, |this, _| {
                        this.inventories
                            .iter()
                            .map(|i| (i.worktrees[0].path.clone(), i.branch.clone()))
                            .collect::<Vec<_>>()
                    }) else {
                        break;
                    };
                    let mrs: HashMap<PathBuf, Mr> = cx
                        .background_executor()
                        .spawn(async move {
                            jobs.into_iter()
                                .filter_map(|(wt, b)| match mr::fetch(&wt, &b) {
                                    Fetch::Mr(m) => Some((wt, *m)),
                                    _ => None,
                                })
                                .collect()
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        for (wt, m) in &mrs {
                            let Some(inv) =
                                this.inventories.iter().find(|i| i.worktrees[0].path == *wt)
                            else {
                                continue;
                            };
                            // MR aberto ou mergeado → Histórico (e [x] na daily), uma vez cada
                            let repo = inv
                                .repo
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let hist = feature::state_lines(&inv.id, "hist");
                            let step = if m.merged {
                                format!("merged:{}", m.iid)
                            } else {
                                format!("open:{}", m.iid)
                            };
                            if !m.closed && !hist.contains(&step) {
                                feature::add_state_line(&inv.id, "hist", &step);
                                for r in crate::vault::on_mr(
                                    &repo,
                                    m.iid,
                                    &inv.branch,
                                    &m.title,
                                    m.merged,
                                ) {
                                    cx.emit(SidebarEvent::Vault(r));
                                }
                            }
                            // uma vez por MR, mesmo depois de reiniciar
                            let iid = m.iid.to_string();
                            if m.merged && !feature::state_lines(&inv.id, "merged").contains(&iid) {
                                feature::add_state_line(&inv.id, "merged", &iid);
                                cx.emit(SidebarEvent::Merged(
                                    wt.clone(),
                                    m.iid,
                                    inv.branch.clone(),
                                ));
                            }
                        }
                        if this.mrs != mrs {
                            this.mrs = mrs;
                            cx.notify();
                        }
                    });
                }
                let timer = cx.background_executor().timer(Duration::from_millis(500));
                if let futures::future::Either::Right(_) =
                    futures::future::select(timer, futures::StreamExt::next(&mut kicked)).await
                {
                    heavy = true;
                }
            }
        });
        let works_file = works_file(herdr.session());
        let works = load_works(&works_file);
        Self {
            next_work: works.iter().map(|w| w.id).max().unwrap_or(0),
            inited: !works.is_empty(),
            works,
            works_file,
            herdr,
            kick,
            agents: Vec::new(),
            workspaces: Vec::new(),
            ws_cwd: HashMap::new(),
            cur: 0,
            fresh: false,
            offline: false,
            since: HashMap::new(),
            usage: HashMap::new(),
            inventories: Vec::new(),
            changes: None,
            files: None,
            mrs: HashMap::new(),
            learned: HashSet::new(),
            sent: HashMap::new(),
            asks: HashMap::new(),
            audit: (false, String::new(), "nunca rodou".into(), false, false),
            focused_cwd: None,
            expanded: HashSet::new(),
            menu: None,
            menu_focus: cx.focus_handle(),
            tree_focus: cx.focus_handle(),
            sel: None,
            clip: None,
            naming: None,
            extra: HashMap::new(),
            stacks: HashMap::new(),
            open: None,
            sessions: Arc::default(),
            anchors,
            theme,
            _poll: poll,
        }
    }

    /// Aplica o estado do herdr; devolve se mudou e os agentes que acabaram de pedir atenção.
    fn apply(&mut self, state: std::io::Result<HerdrState>) -> (bool, Vec<Note>) {
        let offline = state.is_err();
        let (agents, workspaces, ws_cwd) = state.unwrap_or_default();
        let now = Instant::now();
        let mut notes = Vec::new();
        for a in &agents {
            let entry = self
                .since
                .entry(a.pane_id.clone())
                .or_insert((a.agent_status, now));
            if entry.0 != a.agent_status {
                *entry = (a.agent_status, now);
                let kind = match a.agent_status {
                    Status::Blocked => Some(Kind::Ask),
                    Status::Done => Some(Kind::Done),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let wt = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf);
                    let inv = self
                        .inventories
                        .iter()
                        .find(|i| i.worktrees.iter().any(|w| Some(&w.path) == wt.as_ref()));
                    let branch = inv
                        .map(|i| i.branch.clone())
                        .or_else(|| branch_of(a))
                        .unwrap_or_else(|| title(a));
                    let text = if kind == Kind::Ask {
                        let ask = inv
                            .and_then(|i| perms::last_ask(&i.id))
                            .unwrap_or_else(|| title(a));
                        self.asks.insert(a.pane_id.clone(), ask.clone());
                        ask
                    } else {
                        "terminou, pronto pra revisar".into()
                    };
                    notes.push(Note {
                        pane: a.pane_id.clone(),
                        kind,
                        branch,
                        text,
                    });
                }
            }
        }
        let mut changed = self.agents != agents
            || self.workspaces != workspaces
            || self.ws_cwd != ws_cwd
            || self.offline != offline;
        self.agents = agents;
        self.workspaces = workspaces;
        self.ws_cwd = ws_cwd;
        self.offline = offline;
        if !offline && self.sync_works() {
            save_works(&self.works_file, &self.works);
            changed = true;
        }
        (changed, notes)
    }

    /// Repositório principal do workspace, pelo cwd do pane em foco nele.
    fn ws_repo(&self, ws: &str) -> Option<PathBuf> {
        let cwd = self.ws_cwd.get(ws)?;
        herdr::repo_root(Path::new(cwd)).map(herdr::main_repo)
    }

    /// Distribui os workspaces do herdr nas abas de trabalho e segue o em foco.
    /// Na abertura agrupa por repo; depois, workspace novo em foco entra no trabalho atual (ou abre um, se `fresh`).
    fn sync_works(&mut self) -> bool {
        let before = self.works.clone();
        let cur_id = self.works.get(self.cur).map(|w| w.id);
        let live: HashSet<String> = self
            .workspaces
            .iter()
            .map(|w| w.workspace_id.clone())
            .collect();
        for w in &mut self.works {
            w.ws.retain(|id| live.contains(id));
            if !w.ws.contains(&w.last) {
                w.last = w.ws.first().cloned().unwrap_or_default();
            }
        }
        self.works.retain(|w| !w.ws.is_empty());
        self.cur = cur_id
            .and_then(|id| self.works.iter().position(|w| w.id == id))
            .unwrap_or(0);
        for w in self.workspaces.clone() {
            let id = w.workspace_id;
            if self.works.iter().any(|x| x.ws.contains(&id)) {
                continue;
            }
            let target = if self.inited && w.focused {
                (!self.fresh && !self.works.is_empty()).then_some(self.cur)
            } else {
                let repo = self.ws_repo(&id);
                repo.as_ref().and_then(|r| {
                    self.works
                        .iter()
                        .position(|x| x.ws.iter().any(|o| self.ws_repo(o).as_ref() == Some(r)))
                })
            };
            match target {
                Some(i) => self.works[i].ws.push(id),
                None => {
                    if w.focused {
                        self.fresh = false;
                    }
                    self.next_work += 1;
                    self.works.push(Work {
                        id: self.next_work,
                        ws: vec![id.clone()],
                        last: id,
                    });
                }
            }
        }
        self.inited = true;
        if let Some(f) = self.workspaces.iter().find(|w| w.focused)
            && let Some(i) = self
                .works
                .iter()
                .position(|w| w.ws.contains(&f.workspace_id))
        {
            self.cur = i;
            self.works[i].last = f.workspace_id.clone();
        }
        self.works != before
    }

    fn here(&self, ws: &str) -> bool {
        self.works
            .get(self.cur)
            .is_none_or(|w| w.ws.iter().any(|x| x == ws))
    }

    /// (repo ou rótulo, branch) da aba: o que estava em foco nela.
    pub fn work_label(&self, i: usize) -> (String, Option<String>) {
        let Some(w) = self.works.get(i) else {
            return (String::new(), None);
        };
        let root = self
            .ws_cwd
            .get(&w.last)
            .and_then(|c| herdr::repo_root(Path::new(c)).map(Path::to_path_buf));
        match root {
            Some(r) => (
                herdr::main_repo(&r)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                herdr::branch(&r),
            ),
            None => (
                self.workspaces
                    .iter()
                    .find(|x| x.workspace_id == w.last)
                    .map(|x| x.label.clone())
                    .unwrap_or_default(),
                None,
            ),
        }
    }

    /// O estado mais urgente dos Claudes da aba (`Blocked` > `Working`).
    pub fn work_status(&self, i: usize) -> Option<Status> {
        let w = self.works.get(i)?;
        let mine: Vec<_> = self
            .agents
            .iter()
            .filter(|a| w.ws.contains(&a.workspace_id))
            .map(|a| a.agent_status)
            .collect();
        [Status::Blocked, Status::Working]
            .into_iter()
            .find(|s| mine.contains(s))
    }

    /// A faixa e a lateral trocam já; o polling confirma quando o herdr focar.
    pub fn go_work(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.works.len() && i != self.cur {
            self.cur = i;
            cx.notify();
        }
        self.focus_work(i);
    }

    fn focus_work(&self, i: usize) {
        if let Some(w) = self.works.get(i) {
            self.focus_workspace(w.last.clone());
        }
    }

    /// Fecha os workspaces da aba (as worktrees ficam). Recusa com Claude trabalhando ou pedindo algo.
    pub fn close_work(&self, i: usize) -> Result<(), &'static str> {
        let Some(w) = self.works.get(i) else {
            return Ok(());
        };
        if self.work_status(i).is_some() {
            return Err(
                "Tem Claude trabalhando ou esperando você nesta aba. Resolva antes de fechar.",
            );
        }
        if self.works.len() < 2 {
            return Err("É a única aba. Abra outra (Alt+T) antes de fechar esta.");
        }
        if i == self.cur {
            self.focus_work(if i > 0 { i - 1 } else { 1 });
        }
        let h = self.herdr.clone();
        let ws = w.ws.clone();
        std::thread::spawn(move || {
            for id in ws {
                let _ = h.close_workspace(&id);
            }
        });
        Ok(())
    }

    /// Repositórios em ordem alfabética; agentes fora de git vão com os Terminais.
    pub fn repos(&self) -> (Vec<Repo>, Vec<&Agent>) {
        self.repos_in(false)
    }

    /// `here`: só os Claudes da aba de trabalho atual (a lateral).
    fn repos_in(&self, here: bool) -> (Vec<Repo>, Vec<&Agent>) {
        let mut repos: Vec<Repo> = Vec::new();
        let mut loose = Vec::new();
        for a in self
            .agents
            .iter()
            .filter(|a| !here || self.here(&a.workspace_id))
        {
            let Some(root) = herdr::repo_root(Path::new(&a.cwd)).map(herdr::main_repo) else {
                loose.push(a);
                continue;
            };
            let root = root.as_path();
            let path = root.to_string_lossy().into_owned();
            match repos.iter_mut().find(|r| r.path == path) {
                Some(r) => r.agents.push(a.clone()),
                None => repos.push(Repo {
                    name: root
                        .file_name()
                        .map_or(path.clone(), |n| n.to_string_lossy().into()),
                    path,
                    agents: vec![a.clone()],
                }),
            }
        }
        repos.sort_by(|a, b| a.name.cmp(&b.name));
        for r in &mut repos {
            r.agents = self.stack_order(std::mem::take(&mut r.agents));
        }
        (repos, loose)
    }

    /// A feature (inventário) do agente, pela worktree em que ele roda.
    fn inv_of(&self, a: &Agent) -> Option<&Inventory> {
        let wt = herdr::repo_root(Path::new(&a.cwd))?;
        self.inventories
            .iter()
            .find(|i| i.worktrees.iter().any(|w| w.path == wt))
    }

    /// Filha logo abaixo da mãe (e as netas abaixo dela), na ordem do herdr para o resto.
    fn stack_order(&self, agents: Vec<Agent>) -> Vec<Agent> {
        let id = |a: &Agent| self.inv_of(a).map(|i| i.id.clone());
        let mother = |a: &Agent| {
            self.inv_of(a)
                .and_then(|i| i.parent.as_ref())
                .map(|p| p.id.clone())
                .filter(|m| agents.iter().any(|x| id(x).as_ref() == Some(m)))
        };
        fn place<'a>(
            a: &'a Agent,
            all: &'a [Agent],
            id: &dyn Fn(&Agent) -> Option<String>,
            mother: &dyn Fn(&Agent) -> Option<String>,
            out: &mut Vec<Agent>,
        ) {
            if out.iter().any(|x| x.pane_id == a.pane_id) {
                return;
            }
            out.push(a.clone());
            if let Some(me) = id(a) {
                for c in all.iter().filter(|c| mother(c).as_ref() == Some(&me)) {
                    place(c, all, id, mother, out);
                }
            }
        }
        let mut out = Vec::new();
        for a in agents.iter().filter(|a| mother(a).is_none()) {
            place(a, &agents, &id, &mother, &mut out);
        }
        out
    }

    /// Worktree do agente em foco (ou do primeiro em git): a de "Mudanças".
    pub fn top_worktree(&self) -> Option<PathBuf> {
        let focused = self
            .agents
            .iter()
            .find(|a| a.focused)
            .map(|a| a.cwd.as_str());
        let here = self
            .agents
            .iter()
            .filter(|a| self.here(&a.workspace_id))
            .map(|a| a.cwd.as_str());
        pick_worktree(focused.or(self.focused_cwd.as_deref()), here)
    }

    /// Clique numa feature ou terminal: foca e avisa o Shell para sair de MRs, Vault ou Auditoria.
    fn pick(&mut self, focus: impl FnOnce(&Self), cx: &mut Context<Self>) {
        focus(self);
        cx.emit(SidebarEvent::Picked);
    }

    /// Ctrl+Shift+F: a aba Arquivos, relida do repo em foco.
    pub fn show_files(&mut self, cx: &mut Context<Self>) {
        self.files = Some(
            self.top_worktree()
                .map(|r| changes::files(&r))
                .unwrap_or_default(),
        );
        cx.notify();
    }

    /// Relê a aba Arquivos depois de mexer no disco, com as pastas de `reveal` abertas.
    pub fn reload_files(&mut self, reveal: Option<&str>, cx: &mut Context<Self>) {
        if let Some(p) = reveal {
            let mut dir = p;
            while let Some((d, _)) = dir.rsplit_once('/') {
                self.expanded.insert(d.into());
                dir = d;
            }
        }
        self.show_files(cx);
        if let (Some(p), Some(files)) = (reveal, &self.files) {
            let inner = format!("{p}/");
            self.sel = Some((p.into(), files.iter().any(|f| f.starts_with(&inner))));
        }
    }

    /// Soltou (ou colou recortado) `from` dentro da pasta `dir`: o shell move e leva as abas.
    fn move_into(&mut self, from: &str, dir: &str, cx: &mut Context<Self>) {
        if parent(from) == dir || from == dir {
            return;
        }
        let name = from.rsplit('/').next().unwrap_or(from);
        let to = if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        };
        cx.emit(SidebarEvent::Fs(FsOp::Rename(from.into(), to)));
    }

    fn paste(&mut self, dir: &str, cx: &mut Context<Self>) {
        match self.clip.clone() {
            Some((from, true)) => {
                self.clip = None;
                self.move_into(&from, dir, cx);
            }
            Some((from, false)) => cx.emit(SidebarEvent::Fs(FsOp::Copy(from, dir.into()))),
            None => cx.emit(SidebarEvent::Toast(
                "Nada para colar: Ctrl+C ou Ctrl+X num item".into(),
            )),
        }
    }

    /// Teclado da árvore em foco: setas, Enter, F2, Delete, Ctrl+C/X/V.
    fn tree_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.naming.is_some() || self.menu.is_some() {
            return;
        }
        let Some(files) = &self.files else { return };
        let k = &e.keystroke;
        let m = &k.modifiers;
        let rows = changes::tree(files, &self.expanded);
        let at = |p: &str| rows.iter().position(|r| r.2 == p);
        let i = self.sel.as_ref().and_then(|(p, _)| at(p));
        let pick = |j: usize| rows.get(j).map(|r| (r.2.clone(), r.3));
        let sel = self.sel.clone();
        match (k.key.as_str(), m.control, m.shift || m.alt) {
            ("down", false, false) => {
                self.sel = pick(i.map_or(0, |i| (i + 1).min(rows.len().saturating_sub(1))))
            }
            ("up", false, false) => self.sel = pick(i.map_or(0, |i| i.saturating_sub(1))),
            ("right", false, false) => match sel {
                Some((p, true)) if !self.expanded.contains(&p) => {
                    self.expanded.insert(p);
                }
                Some((_, true)) => self.sel = i.and_then(|i| pick(i + 1)),
                _ => {}
            },
            ("left", false, false) => match sel {
                Some((p, true)) if self.expanded.contains(&p) => {
                    self.expanded.remove(&p);
                }
                Some((p, _)) if !parent(&p).is_empty() => {
                    self.sel = Some((parent(&p).into(), true))
                }
                _ => {}
            },
            ("enter", false, false) => match sel {
                Some((p, true)) => {
                    if !self.expanded.remove(&p) {
                        self.expanded.insert(p);
                    }
                }
                Some((p, false)) => cx.emit(SidebarEvent::OpenFile(p)),
                None => return,
            },
            ("f2", false, false) => match sel {
                Some((p, _)) => self.start_naming(Naming::Rename(p), window, cx),
                None => return,
            },
            ("delete", false, false) => match sel {
                Some((p, dir)) => cx.emit(SidebarEvent::Fs(FsOp::Delete(p, dir))),
                None => return,
            },
            ("c" | "x", true, false) => match sel {
                Some((p, _)) => {
                    let cut = k.key == "x";
                    cx.emit(SidebarEvent::Toast(format!(
                        "{} {p} · Ctrl+V cola na pasta selecionada",
                        if cut { "Recortado" } else { "Copiado" }
                    )));
                    self.clip = Some((p, cut));
                }
                None => return,
            },
            ("v", true, false) => {
                let dir = match &sel {
                    Some((p, true)) => p.clone(),
                    Some((p, false)) => parent(p).into(),
                    None => String::new(),
                };
                self.paste(&dir, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn menu_pick(
        &mut self,
        a: Act,
        p: &str,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        let here = if dir { p } else { parent(p) }.to_string();
        match a {
            Act::NewFile | Act::NewDir => {
                self.start_naming(Naming::New(here, a == Act::NewDir), window, cx)
            }
            Act::Cut | Act::Copy => self.clip = Some((p.into(), a == Act::Cut)),
            Act::Paste => self.paste(&here, cx),
            Act::Duplicate => cx.emit(SidebarEvent::Fs(FsOp::Copy(p.into(), parent(p).into()))),
            Act::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(p.into()));
                cx.emit(SidebarEvent::Toast(format!("Copiado: {p}")));
            }
            Act::Rename => self.start_naming(Naming::Rename(p.into()), window, cx),
            Act::Delete => cx.emit(SidebarEvent::Fs(FsOp::Delete(p.into(), dir))),
        }
        cx.notify();
    }

    fn start_naming(&mut self, n: Naming, window: &mut Window, cx: &mut Context<Self>) {
        let value = match &n {
            Naming::New(parent, _) if !parent.is_empty() => {
                self.expanded.insert(parent.clone());
                format!("{parent}/")
            }
            Naming::New(..) => String::new(),
            Naming::Rename(p) => p.clone(),
        };
        let t = self.theme.clone();
        let f = cx.new(|cx| TextField::new(t, &value, "nome", false, true, cx));
        cx.subscribe_in(&f, window, |this, f, e: &FieldEvent, _, cx| match e {
            FieldEvent::Submit => {
                let v = f.read(cx).value.trim().to_string();
                let op = match this.naming.take() {
                    Some((Naming::New(_, dir), _)) => Some(FsOp::Create(v, dir)),
                    Some((Naming::Rename(p), _)) => (p != v).then_some(FsOp::Rename(p, v)),
                    None => None,
                };
                if let Some(op) = op {
                    cx.emit(SidebarEvent::Fs(op));
                }
                cx.notify();
            }
            FieldEvent::Cancel => {
                this.naming = None;
                cx.notify();
            }
            FieldEvent::Changed => {}
        })
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        self.naming = Some((n, f));
        cx.notify();
    }

    pub fn set_open(&mut self, open: Option<(String, bool, bool)>, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    pub fn feature_refs(&self) -> Vec<crate::vault_view::FeatureRef> {
        crate::vault_view::features_from(&self.inventories, &self.agents, &self.mrs, &self.usage)
    }

    /// Repositório principal dessa worktree: o do cabeçalho e o da "Nova feature".
    pub fn top_repo(&self) -> Option<PathBuf> {
        self.top_worktree().map(|w| herdr::main_repo(&w))
    }

    pub fn today_cost(&self) -> f64 {
        self.usage.values().map(|u| u.today).sum::<f64>() + 0.0
    }

    pub fn first_blocked(&self) -> Option<String> {
        let order = self.order();
        order.into_iter().find(|id| {
            self.agents
                .iter()
                .any(|a| &a.pane_id == id && a.agent_status == Status::Blocked)
        })
    }

    /// (pane, nome na lateral) na ordem da tela, para a paleta.
    pub fn features(&self) -> Vec<(String, String)> {
        let (repos, loose) = self.repos_in(true);
        let label = |pane: &str| -> String {
            repos
                .iter()
                .flat_map(|r| &r.agents)
                .find(|a| a.pane_id == pane)
                .and_then(branch_of)
                .or_else(|| loose.iter().find(|a| a.pane_id == pane).map(|a| title(a)))
                .unwrap_or_default()
        };
        self.order_in(true)
            .into_iter()
            .map(|p| {
                let l = label(&p);
                (p, l)
            })
            .collect()
    }

    /// Ordem em que aparecem na lateral (Ctrl+Shift+1…9).
    pub fn order(&self) -> Vec<String> {
        self.order_in(false)
    }

    /// Ctrl+Shift+1…9: a ordem da lateral, só da aba atual.
    pub fn order_here(&self) -> Vec<String> {
        self.order_in(true)
    }

    fn order_in(&self, here: bool) -> Vec<String> {
        let (repos, loose) = self.repos_in(here);
        let top = self.top_repo().map(|p| p.to_string_lossy().into_owned());
        repos
            .iter()
            .filter(|r| Some(&r.path) == top.as_ref())
            .chain(repos.iter().filter(|r| Some(&r.path) != top.as_ref()))
            .flat_map(|r| r.agents.iter().map(|a| a.pane_id.clone()))
            .chain(loose.iter().map(|a| a.pane_id.clone()))
            .collect()
    }

    // uma chamada curta ao socket; thread própria para não segurar a UI nem o contexto
    /// Repositórios principais com algum agente aberto (onde a Limpeza procura).
    pub fn repo_paths(&self) -> Vec<PathBuf> {
        self.repos()
            .0
            .into_iter()
            .map(|r| PathBuf::from(r.path))
            .collect()
    }

    pub fn mr_of(&self, wt: &Path) -> Option<Mr> {
        self.mrs.get(wt).cloned()
    }

    pub fn herdr(&self) -> Arc<Herdr> {
        self.herdr.clone()
    }

    pub fn focus(&self, pane_id: String) {
        let h = self.herdr.clone();
        std::thread::spawn(move || h.focus(&pane_id));
    }

    pub fn focus_workspace(&self, id: String) {
        let (h, kick) = (self.herdr.clone(), self.kick.clone());
        std::thread::spawn(move || {
            let _ = h.focus_workspace(&id);
            let _ = kick.unbounded_send(());
        });
    }

    /// Próximo da fila: quem precisa de você primeiro, depois quem terminou.
    /// A fila: permissão > CI/Sonar > comentários, prontas e comparar > contexto alto.
    pub fn attention(&self) -> Vec<AttItem> {
        let mut q = Vec::new();
        let mut groups_done: HashSet<String> = HashSet::new();
        for pane in self.order() {
            let Some(a) = self.agents.iter().find(|a| a.pane_id == pane) else {
                continue;
            };
            let Some(wt) = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf) else {
                continue;
            };
            let inv = self
                .inventories
                .iter()
                .find(|i| i.worktrees.iter().any(|w| w.path == wt));
            // o git só roda quando o item existe (a fila é recalculada a cada desenho do título)
            let branch = || {
                inv.map_or_else(
                    || branch_of(a).unwrap_or_else(|| title(a)),
                    |i| i.branch.clone(),
                )
            };
            let item = |p, tag, text: String, go| AttItem { p, tag, text, go };
            if a.agent_status == Status::Blocked {
                let msg = self.asks.get(&pane).cloned().unwrap_or_else(|| title(a));
                q.push(item(
                    0,
                    "permissão",
                    format!("{}: {msg}", branch()),
                    Go::Pane(pane.clone()),
                ));
            }
            if let Some(m) = self.mrs.get(&wt).filter(|m| !m.merged) {
                let go = Go::Mr(pane.clone(), wt.clone());
                if m.pipe == mr::Status::Fail {
                    q.push(item(
                        1,
                        "CI",
                        format!("CI falhou em !{} ({})", m.iid, branch()),
                        go.clone(),
                    ));
                }
                if m.sonar == mr::Status::Fail {
                    q.push(item(
                        1,
                        "Sonar",
                        format!("Quality gate reprovado em !{}", m.iid),
                        go.clone(),
                    ));
                }
                let sent = self.sent.get(&wt);
                let n = m
                    .comments
                    .iter()
                    .filter(|c| sent.is_none_or(|s| !s.contains(&c.id)))
                    .count();
                if n > 0 {
                    let s = if n > 1 { "s" } else { "" };
                    q.push(item(
                        2,
                        "MR",
                        format!("{n} comentário{s} novo{s} em !{}", m.iid),
                        go,
                    ));
                }
            }
            if a.agent_status == Status::Done {
                q.push(item(
                    2,
                    "revisar",
                    format!("{} pronta pra revisar", branch()),
                    Go::Review(pane.clone(), wt.clone()),
                ));
            }
            if let Some(g) = inv.and_then(|i| i.group.clone()) {
                let attempts: Vec<&Inventory> = self
                    .inventories
                    .iter()
                    .filter(|i| i.group.as_ref() == Some(&g))
                    .collect();
                let done = attempts.len() > 1
                    && attempts.iter().all(|i| {
                        self.agents.iter().any(|x| {
                            x.agent_status == Status::Done
                                && i.worktrees
                                    .iter()
                                    .any(|w| Path::new(&x.cwd).starts_with(&w.path))
                        })
                    });
                if done && groups_done.insert(g.clone()) {
                    q.push(item(
                        2,
                        "comparar",
                        format!("Tentativas de {g} prontas para comparar"),
                        Go::Compare(pane.clone()),
                    ));
                }
            }
            if let Some(u) = self.usage.get(&pane).filter(|u| u.ctx >= 80) {
                q.push(item(
                    3,
                    "contexto",
                    format!(
                        "{} com {}% do contexto: perto de compactar",
                        branch(),
                        u.ctx
                    ),
                    Go::Pane(pane.clone()),
                ));
            }
        }
        if self.audit.4 {
            q.push(AttItem {
                p: 3,
                tag: "auditoria",
                text: "Auditoria pronta com achados para revisar".into(),
                go: Go::Audit,
            });
        }
        q.sort_by_key(|i| i.p);
        q
    }

    fn agent_row(&self, a: &Agent, branch: Option<&str>) -> Row {
        let t = &self.theme;
        let (g, label) = glyph(a.agent_status);
        let color = match a.agent_status {
            Status::Blocked => t.orange,
            Status::Working => t.cyan,
            Status::Done => t.ok,
            Status::Idle | Status::Unknown => t.comment,
        };
        let attention = matches!(a.agent_status, Status::Blocked | Status::Done);
        let wt = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf);
        let port = self
            .inventories
            .iter()
            .find(|i| i.worktrees.iter().any(|w| Some(&w.path) == wt.as_ref()))
            .map_or(String::new(), |i| format!(":{} · ", i.port));
        let m2 = self.usage.get(&a.pane_id).map(|u| {
            (
                format!("{port}US$ {}", brl(u.cost)),
                format!("ctx {}%{}", u.ctx, if u.ctx >= 80 { " ⚠" } else { "" }),
                u.ctx >= 80,
            )
        });
        Row {
            id: a.pane_id.clone(),
            glyph: g,
            glyph_color: color,
            b: branch.map_or_else(|| title(a), Into::into),
            k: self
                .since
                .get(&a.pane_id)
                .map(|(_, t)| ago(t.elapsed()))
                .unwrap_or_default(),
            m: match (attention, branch.is_some()) {
                (true, true) => format!("{label} · {}", title(a)),
                (true, false) => label.into(),
                (false, true) => title(a),
                (false, false) => tilde(&a.cwd),
            },
            m_color: if attention { color } else { t.muted },
            m2,
            mr: wt
                .as_ref()
                .and_then(|w| self.mrs.get(w))
                .cloned()
                .map(|mut m| {
                    let sent = wt.as_ref().and_then(|w| self.sent.get(w));
                    m.comments
                        .retain(|c| sent.is_none_or(|s| !s.contains(&c.id)));
                    m
                }),
            on: a.focused,
            unseen: attention.then_some(color),
            dim: false,
            child: false,
            extra: Vec::new(),
        }
    }

    fn row(&self, r: Row, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id(SharedString::from(r.id))
            .relative()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px_2()
            .py(px(6.))
            .rounded(px(6.))
            .when(r.dim, |d| d.opacity(0.45))
            .when(!r.dim, |d| d.cursor_pointer().hover(|d| d.bg(t.raise)))
            .when(r.on, |d| d.bg(t.accent_soft))
            .when(r.child, |d| {
                d.ml(px(16.)).child(
                    div()
                        .absolute()
                        .left(px(-12.))
                        .top(px(6.))
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .text_color(t.comment)
                        .child("↳"),
                )
            })
            .on_click(move |_, _, cx| on_click(cx))
            .when_some(r.unseen, |d, c| {
                d.child(
                    div()
                        .absolute()
                        .left(px(2.))
                        .top(px(8.))
                        .bottom(px(8.))
                        .w(px(2.))
                        .rounded(px(2.))
                        .bg(c),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(16.))
                            .flex_none()
                            .text_center()
                            .font_family(t.mono.clone())
                            .text_size(px(13.))
                            .text_color(r.glyph_color)
                            .child(r.glyph),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(t.mono.clone())
                            .text_size(px(13.5))
                            .child(r.b),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(r.k),
                    ),
            )
            .child(
                div()
                    .pl(px(24.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(r.m_color)
                    .child(r.m),
            )
            .when_some(r.m2, |d, (cost, ctx, warn)| {
                d.child(
                    div()
                        .pl(px(24.))
                        .flex()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .text_color(t.comment)
                        .child(format!("{cost} · "))
                        .child(div().when(warn, |d| d.text_color(t.orange)).child(ctx))
                        .when_some(r.mr, |d, m| d.children(mr_sum(t, &m))),
                )
            })
            .children(r.extra)
    }

    /// Embaixo do card: "⎇ repo · N" de cada outro repo e a pílula da pilha.
    fn feature_extra(&self, a: &Agent, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = &self.theme;
        let Some(wt) = herdr::repo_root(Path::new(&a.cwd)).map(Path::to_path_buf) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(repos) = self.extra.get(&wt) {
            let mut line = div()
                .pl(px(24.))
                .flex()
                .flex_wrap()
                .gap_x(px(8.))
                .gap_y(px(2.))
                .font_family(t.mono.clone())
                .text_size(px(12.))
                .text_color(t.muted);
            for (i, (name, path, n)) in repos.iter().enumerate() {
                let path = path.clone();
                line = line.child(
                    div()
                        .id(SharedString::from(format!("sub:{}:{i}", wt.display())))
                        .flex()
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.fg))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.emit(SidebarEvent::OpenReviewAt(path.clone()));
                        }))
                        .child(div().text_color(t.comment).child("⎇ "))
                        .child(if *n > 0 {
                            format!("{name} · {n}")
                        } else {
                            name.clone()
                        }),
                );
            }
            out.push(line.into_any_element());
        }
        if let Some(&st) = self.stacks.get(&wt) {
            let (label, color) = match st {
                Stack::Rebase => ("base mudou · Rebase", t.orange),
                Stack::MoveToBase => ("mergeada · Mover p/ main", t.ok),
            };
            let label = match (st, self.inv_of(a).and_then(|i| i.parent.as_ref())) {
                (Stack::MoveToBase, Some(p)) => format!("mergeada · Mover p/ {}", p.base),
                _ => label.to_string(),
            };
            out.push(
                div()
                    .pl(px(24.))
                    .flex()
                    .child(
                        div()
                            .id(SharedString::from(format!("stk:{}", wt.display())))
                            .cursor_pointer()
                            .px(px(6.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(color)
                            .text_color(color)
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(SidebarEvent::Stack(wt.clone(), st));
                            }))
                            .child(label),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn file_menu(&self, p: &str, dir: bool, at: Point<Pixels>, cx: &mut Context<Self>) -> Deferred {
        let t = &self.theme;
        use Act::*;
        let acts: &[Option<Act>] = if p.is_empty() {
            &[Some(NewFile), Some(NewDir), None, Some(Paste)]
        } else {
            &[
                Some(NewFile),
                Some(NewDir),
                None,
                Some(Cut),
                Some(Copy),
                Some(Paste),
                Some(Duplicate),
                None,
                Some(CopyPath),
                None,
                Some(Rename),
                Some(Delete),
            ]
        };
        let mut m = div()
            .id("fl-menu")
            .track_focus(&self.menu_focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    this.menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .occlude()
            .min_w(px(170.))
            .p(px(4.))
            .flex()
            .flex_col()
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .shadow_lg()
            .font_family(t.sans)
            .text_size(px(13.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }));
        for (i, a) in acts.iter().enumerate() {
            let Some(a) = *a else {
                m = m.child(div().my(px(4.)).h(px(1.)).bg(t.line));
                continue;
            };
            let (label, key) = a.label();
            let off = a == Paste && self.clip.is_none();
            let p = p.to_string();
            m = m.child(
                div()
                    .id(("flm", i))
                    .flex()
                    .justify_between()
                    .gap(px(18.))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .when(off, |d| d.opacity(0.45))
                    .when(!off, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(t.accent_soft))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.menu_pick(a, &p, dir, window, cx)
                            }))
                    })
                    .when(a == Delete, |d| d.text_color(rgb(0xff5555)))
                    .child(label)
                    .child(div().text_color(t.comment).child(key)),
            );
        }
        deferred(anchored().position(at).child(m))
    }

    fn changes_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let n = self.changes.as_ref().map_or(0, |c| c.files.len());
        let on_files = self.files.is_some();
        let tab = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .cursor_pointer()
                .px(px(8.))
                .py(px(4.))
                .rounded_t(px(5.))
                .text_size(px(13.))
                .text_color(if on { t.fg } else { t.muted })
                .when(on, |d| d.border_b_2().border_color(t.accent))
                .child(label)
        };
        let mut list = div().flex().flex_col().pt(px(4.)).pb(px(10.));
        let row = |id: String, dir: &str, name: &str| {
            let dir = if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            };
            div()
                .id(SharedString::from(id))
                .cursor_pointer()
                .hover(|d| d.bg(t.raise))
                .flex()
                .gap(px(6.))
                .items_center()
                .px(px(12.))
                .py(px(3.))
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
                        .child(name.to_string()),
                )
        };
        let split = |p: &str| -> (String, String) {
            p.rsplit_once('/')
                .map_or((String::new(), p.to_string()), |(d, n)| {
                    (d.into(), n.into())
                })
        };
        match (&self.files, &self.changes) {
            (Some(files), _) => {
                let field = |depth: usize, f: &Entity<TextField>| {
                    div()
                        .pl(px(8. + depth as f32 * 14.))
                        .pr(px(8.))
                        .py(px(2.))
                        .child(f.clone())
                };
                if let Some((Naming::New(parent, _), f)) = &self.naming
                    && parent.is_empty()
                {
                    list = list.child(field(0, f));
                }
                // ponytail: 500 linhas visíveis; lista virtual se uma pasta aberta passar disso
                for (depth, name, p, dir) in
                    changes::tree(files, &self.expanded).into_iter().take(500)
                {
                    match &self.naming {
                        Some((Naming::Rename(r), f)) if *r == p => {
                            list = list.child(field(depth, f));
                            continue;
                        }
                        _ => {}
                    }
                    let new_here = match &self.naming {
                        Some((Naming::New(parent, _), f)) if *parent == p => Some(f.clone()),
                        _ => None,
                    };
                    let menu_path = p.clone();
                    let open = self.open.as_ref().filter(|o| o.0 == p);
                    let glyph = match (dir, self.expanded.contains(&p)) {
                        (true, true) => "▾",
                        (true, false) => "▸",
                        _ => " ",
                    };
                    let path = p.clone();
                    let here = if dir {
                        p.clone()
                    } else {
                        parent(&p).to_string()
                    };
                    let selected = self.sel.as_ref().is_some_and(|s| s.0 == p);
                    let cut = self.clip.as_ref().is_some_and(|c| c.1 && c.0 == p);
                    let (drag, soft, theme) = (DragPath(p.clone()), t.accent_soft, t.clone());
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("fl:{p}")))
                            .cursor_pointer()
                            .hover(|d| d.bg(t.raise))
                            .border_l_2()
                            .border_color(if selected {
                                t.accent
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .when(cut, |d| d.opacity(0.5))
                            .on_drag(drag, move |d: &DragPath, _, _, cx| {
                                let name = d.0.rsplit('/').next().unwrap_or(&d.0).to_string();
                                let theme = theme.clone();
                                cx.new(|_| DragChip(name, theme))
                            })
                            .drag_over::<DragPath>(move |s, _, _, _| s.bg(soft))
                            .on_drop(cx.listener(move |this, d: &DragPath, _, cx| {
                                cx.stop_propagation();
                                this.move_into(&d.0, &here, cx);
                            }))
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .pl(px(12. + depth as f32 * 14.))
                            .pr(px(12.))
                            .py(px(2.))
                            .font_family(t.mono.clone())
                            .text_size(px(13.))
                            .when(open.is_some_and(|o| o.2), |d| d.bg(t.accent_soft))
                            .child(
                                div()
                                    .w(px(12.))
                                    .flex_none()
                                    .font_family(t.sans)
                                    .text_size(px(14.))
                                    .text_color(t.muted)
                                    .child(glyph),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .when(dir, |d| d.text_color(t.muted))
                                    .child(name),
                            )
                            .when(open.is_some_and(|o| o.1), |d| {
                                d.child(div().text_size(px(13.)).text_color(t.muted).child("●"))
                            })
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                    this.menu = Some((menu_path.clone(), dir, e.position));
                                    window.focus(&this.menu_focus, cx);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.sel = Some((path.clone(), dir));
                                if dir {
                                    window.focus(&this.tree_focus, cx);
                                    if !this.expanded.remove(&path) {
                                        this.expanded.insert(path.clone());
                                    }
                                } else {
                                    cx.emit(SidebarEvent::OpenFile(path.clone()))
                                }
                                cx.notify();
                            })),
                    );
                    if let Some(f) = &new_here {
                        list = list.child(field(depth + 1, f));
                    }
                }
                // a lista estica até o fim da lateral: botão direito no vazio cria na raiz
                let raise = t.raise;
                list = list
                    .track_focus(&self.tree_focus)
                    .on_key_down(cx.listener(Self::tree_key))
                    .drag_over::<DragPath>(move |s, _, _, _| s.bg(raise))
                    .on_drop(cx.listener(|this, d: &DragPath, _, cx| this.move_into(&d.0, "", cx)))
                    .flex_grow(1.)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, e: &MouseDownEvent, window, cx| {
                            this.menu = Some((String::new(), true, e.position));
                            window.focus(&this.menu_focus, cx);
                            cx.notify();
                        }),
                    );
                if let Some((p, dir, at)) = &self.menu {
                    list = list.child(self.file_menu(p, *dir, *at, cx));
                }
            }
            (None, None) => {}
            (None, Some(c)) if c.files.is_empty() => {
                list = list.child(none(t, "Nada mudou nesta branch.".into()));
            }
            (None, Some(c)) => {
                for f in &c.files {
                    let (dir, name) = split(&f.path);
                    let path = f.path.clone();
                    list = list.child(
                        row(format!("ch:{}", f.path), &dir, &name)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SidebarEvent::OpenReview(path.clone()))
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(t.mono.clone())
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child(format!(
                                        "{}+{} −{}",
                                        if f.new { "novo " } else { "" },
                                        f.added,
                                        f.removed
                                    )),
                            ),
                    );
                }
                let pending = if c.uncommitted > 0 {
                    format!(
                        "{} arquivo{} não commitado{} · ",
                        c.uncommitted,
                        if c.uncommitted > 1 { "s" } else { "" },
                        if c.uncommitted > 1 { "s" } else { "" }
                    )
                } else {
                    String::new()
                };
                list = list.child(
                    div()
                        .flex()
                        .px(px(12.))
                        .py(px(6.))
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .child(format!("{pending}base "))
                        .child(div().font_family(t.mono.clone()).child(c.base.clone())),
                );
            }
        }
        div()
            .flex()
            .flex_col()
            .flex_grow(1.)
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .pt(px(6.))
                    .px(px(8.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        tab("lt-ch", format!("Mudanças {n}"), !on_files).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.files = None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        tab("lt-files", "Arquivos".into(), on_files).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.files = Some(
                                    this.top_worktree()
                                        .map(|r| changes::files(&r))
                                        .unwrap_or_default(),
                                );
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(list)
    }
}

/// O `mrSum` do protótipo: ` · !148 CI✗ Sonar✓ 2 coment.`
fn mr_sum(t: &Theme, m: &Mr) -> Vec<Div> {
    let red: Hsla = rgb(0xff5555).into();
    let mark = |label: &str, s: mr::Status| {
        let (g, c) = match s {
            mr::Status::Ok => ("✓", t.ok),
            mr::Status::Fail => ("✗", red),
            mr::Status::Running => ("⟳", rgb(0xf1fa8c).into()),
            mr::Status::None => ("–", t.comment),
        };
        div().ml(px(4.)).text_color(c).child(format!("{label}{g}"))
    };
    let mut v = vec![div().child(format!(" · !{}", m.iid))];
    if m.merged {
        v.push(div().ml(px(4.)).text_color(t.ok).child("mergeado"));
        return v;
    }
    v.push(mark("CI", m.pipe));
    v.push(mark("Sonar", m.sonar));
    if !m.comments.is_empty() {
        v.push(
            div()
                .ml(px(4.))
                .text_color(t.orange)
                .child(format!("{} coment.", m.comments.len())),
        );
    }
    v
}

fn none(t: &Theme, text: String) -> Div {
    div()
        .px(px(12.))
        .py(px(6.))
        .text_size(px(13.))
        .text_color(t.muted)
        .child(text)
}

/// `0.42` → `0,42`, como o protótipo escreve dinheiro
pub fn brl(v: f64) -> String {
    format!("{v:.2}").replace('.', ",")
}

pub fn ago(d: Duration) -> String {
    match d.as_secs() / 60 {
        0 => "agora".into(),
        m @ 1..60 => format!("há {m} min"),
        m => format!("há {} h", m / 60),
    }
}

pub fn glyph(s: Status) -> (&'static str, &'static str) {
    match s {
        Status::Blocked => ("!", "precisa de você"),
        Status::Working => ("◐", "trabalhando"),
        Status::Done => ("✓", "pronto pra revisar"),
        Status::Idle | Status::Unknown => ("○", "parado"),
    }
}

fn works_file(session: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join(".local/share/maestro")
        .join(format!("works-{session}.json"))
}

fn load_works(f: &Path) -> Vec<Work> {
    std::fs::read_to_string(f)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Escrita atômica: um arquivo pela metade viraria "nenhuma aba" na próxima abertura.
fn save_works(f: &Path, works: &[Work]) {
    let Ok(json) = serde_json::to_string(works) else {
        return;
    };
    let tmp = f.with_extension("json.tmp");
    let _ = f
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|_| std::fs::write(&tmp, json))
        .and_then(|_| std::fs::rename(&tmp, f));
}

type Extra = HashMap<PathBuf, Vec<(String, PathBuf, usize)>>;

/// Os outros repos de cada feature multi-repo e o que a pilha pede para cada filha (git: fora da UI).
fn stack_and_repos(
    invs: &[Inventory],
    merged: &HashSet<PathBuf>,
) -> (Extra, HashMap<PathBuf, Stack>) {
    let mut extra = HashMap::new();
    let mut stacks = HashMap::new();
    for inv in invs {
        let main = inv.worktrees[0].path.clone();
        if inv.worktrees.len() > 1 {
            let list = inv.worktrees[1..]
                .iter()
                .map(|w| {
                    (
                        w.repo
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        w.path.clone(),
                        changes::changes(&w.path).map_or(0, |c| c.files.len()),
                    )
                })
                .collect();
            extra.insert(main.clone(), list);
        }
        if let Some(p) = &inv.parent {
            let mother_merged = invs
                .iter()
                .find(|m| m.id == p.id)
                .is_some_and(|m| merged.contains(&m.worktrees[0].path));
            if let Some(st) = feature::stack_state(inv, mother_merged) {
                stacks.insert(main, st);
            }
        }
    }
    (extra, stacks)
}

fn branch_of(a: &Agent) -> Option<String> {
    herdr::repo_root(Path::new(&a.cwd)).and_then(herdr::branch)
}

fn title(a: &Agent) -> String {
    let t = a.terminal_title_stripped.as_deref().unwrap_or(&a.agent);
    // o claude põe o spinner no começo do título
    let t = t.trim_start_matches(|c: char| !c.is_alphanumeric());
    if t.is_empty() {
        a.agent.clone()
    } else {
        t.into()
    }
}

pub fn tilde(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.into(),
    }
}

fn section(t: &Theme, label: &str) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_h(px(30.))
        .px(px(10.))
        .text_size(px(12.))
        .text_color(t.muted)
        .border_b_1()
        .border_color(t.line)
        .child(label.to_uppercase())
}

fn feats() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pt(px(2.))
        .px(px(6.))
        .pb(px(6.))
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let this = cx.entity();
        let (repos, loose) = self.repos_in(true);
        let total: usize = repos.iter().map(|r| r.agents.len()).sum();
        let top = self.top_repo().map(|p| p.to_string_lossy().into_owned());
        let repo_head = |name: String, path: &str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .pt(px(10.))
                .pb(px(6.))
                .px(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(name)
                .child(
                    div()
                        .truncate()
                        .font_family(t.mono.clone())
                        .text_size(px(13.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(t.muted)
                        .child(tilde(path)),
                )
        };

        let mut col = div().flex().flex_col().flex_grow(1.).flex_shrink_0();
        // ordem do protótipo: repo · Auditoria · FEATURES · cards · Nova feature · TERMINAIS · Mudanças
        // o repo em foco, com ou sem Claude rodando nele
        if let Some(top) = top.as_ref() {
            let name = Path::new(top)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            col = col.child(repo_head(name, top));
            let (run, k, m, on, _) = self.audit.clone();
            col = col.child(feats().child(self.row(
                Row {
                    id: "auditoria".into(),
                    glyph: if run { "◐" } else { "◇" },
                    glyph_color: if run { t.cyan } else { t.comment },
                    b: "Auditoria".into(),
                    k,
                    m,
                    m_color: t.muted,
                    m2: None,
                    mr: None,
                    on,
                    unseen: None,
                    dim: false,
                    child: false,
                    extra: Vec::new(),
                },
                {
                    let this = this.clone();
                    move |cx| this.update(cx, |_, cx| cx.emit(SidebarEvent::Audit))
                },
            )));
        }
        col = col.child(
            section(&t, "Features")
                .child(div().flex_1())
                .child(div().font_family(t.mono.clone()).child(total.to_string())),
        );
        if self.offline {
            col = col.child(none(&t, "herdr fora do ar; tentando de novo".into()));
        } else if repos.is_empty() {
            col = col.child(none(&t, "Nenhum Claude rodando num repositório.".into()));
        }
        let mut list = feats();
        // o repo do cabeçalho primeiro; os outros depois, cada um com o seu cabeçalho
        let ordered = repos
            .iter()
            .filter(|r| Some(&r.path) == top.as_ref())
            .chain(repos.iter().filter(|r| Some(&r.path) != top.as_ref()));
        for repo in ordered {
            if Some(&repo.path) != top.as_ref() {
                list = list.child(repo_head(repo.name.clone(), &repo.path));
            }
            for a in &repo.agents {
                let pane = a.pane_id.clone();
                let this = this.clone();
                let mut row = self.agent_row(a, branch_of(a).as_deref());
                row.child = self
                    .inv_of(a)
                    .and_then(|i| i.parent.as_ref())
                    .is_some_and(|p| {
                        repo.agents
                            .iter()
                            .any(|x| self.inv_of(x).is_some_and(|i| i.id == p.id))
                    });
                row.extra = self.feature_extra(a, cx);
                list = list.child(self.row(row, move |cx| {
                    this.update(cx, |s, cx| s.pick(|s| s.focus(pane.clone()), cx))
                }));
            }
        }
        col = col.child(
            list.relative()
                .child(tour::anchor(&self.anchors, "features")),
        );
        col = col.child(
            div()
                .id("newf")
                .cursor_pointer()
                .hover(|d| d.text_color(t.fg).border_color(t.accent))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::NewFeature)))
                .relative()
                .child(tour::anchor(&self.anchors, "newf"))
                .mx(px(6.))
                .mt(px(2.))
                .mb(px(8.))
                .px(px(8.))
                .py(px(6.))
                .border_1()
                .border_dashed()
                .border_color(t.line)
                .rounded(px(6.))
                .flex()
                .justify_between()
                .items_center()
                .text_color(t.muted)
                .child("+ Nova feature")
                .child(
                    div()
                        .font_family(t.mono.clone())
                        .text_size(px(12.))
                        .px(px(4.))
                        .border_1()
                        .border_color(t.line)
                        .rounded(px(3.))
                        .bg(t.panel)
                        .child("Alt+N"),
                ),
        );
        col = col.child(
            div()
                .pt(px(8.))
                .pb(px(2.))
                .px(px(14.))
                .text_size(px(12.))
                .text_color(t.muted)
                .child("TERMINAIS"),
        );
        let mut terms = feats();
        for w in self
            .workspaces
            .iter()
            .filter(|w| self.here(&w.workspace_id))
        {
            let id = w.workspace_id.clone();
            let this = this.clone();
            let row = Row {
                id: format!("ws:{}", w.workspace_id),
                glyph: "$",
                glyph_color: t.comment,
                b: w.label.clone(),
                k: format!(
                    "{} aba{}",
                    w.tab_count,
                    if w.tab_count == 1 { "" } else { "s" }
                ),
                m: "só terminal".into(),
                m_color: t.muted,
                m2: None,
                mr: None,
                on: false,
                unseen: None,
                dim: false,
                child: false,
                extra: Vec::new(),
            };
            terms = terms.child(self.row(row, move |cx| {
                this.update(cx, |s, cx| s.pick(|s| s.focus_workspace(id.clone()), cx))
            }));
        }
        for a in loose {
            let pane = a.pane_id.clone();
            let this = this.clone();
            let row = self.agent_row(a, None);
            terms = terms.child(self.row(row, move |cx| {
                this.update(cx, |s, cx| s.pick(|s| s.focus(pane.clone()), cx))
            }));
        }
        col = col.child(terms);
        if top.is_some() {
            col = col.child(self.changes_panel(cx));
        }

        div()
            .id("sidebar")
            .w_full()
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(col)
    }
}

/// O pane em foco manda: com `cd ~` a lateral fica sem repo; os Claudes da aba só valem sem foco conhecido.
fn pick_worktree<'a>(
    focused: Option<&str>,
    mut here: impl Iterator<Item = &'a str>,
) -> Option<PathBuf> {
    match focused {
        Some(c) => herdr::repo_root(Path::new(c)),
        None => here.find_map(|c| herdr::repo_root(Path::new(c))),
    }
    .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::{Work, ago, load_works, pick_worktree, save_works, title};

    #[test]
    fn lateral_segue_a_pasta_do_pane_em_foco() {
        let dir = std::env::temp_dir().join(format!("maestro-top-{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(a.join(".git")).unwrap();
        std::fs::create_dir_all(b.join(".git")).unwrap();
        let (sa, sb) = (a.to_str().unwrap(), b.to_str().unwrap());
        assert_eq!(pick_worktree(Some(sb), [sa].into_iter()), Some(b.clone()));
        assert_eq!(
            pick_worktree(Some(dir.to_str().unwrap()), [sa].into_iter()),
            None
        );
        assert_eq!(pick_worktree(None, [sa].into_iter()), Some(a.clone()));
        let _ = std::fs::remove_dir_all(dir);
    }
    use crate::herdr::{Agent, Status};
    use std::time::Duration;

    #[test]
    fn abas_de_trabalho_voltam_do_disco() {
        let dir = std::env::temp_dir().join(format!("maestro-works-{}", std::process::id()));
        let f = dir.join("works-test.json");
        assert!(load_works(&f).is_empty());
        let works = vec![
            Work {
                id: 1,
                ws: vec!["w1".into(), "w3".into()],
                last: "w3".into(),
            },
            Work {
                id: 4,
                ws: vec!["w2".into()],
                last: "w2".into(),
            },
        ];
        save_works(&f, &works);
        assert_eq!(load_works(&f), works);
        std::fs::write(&f, "{corrompido").unwrap();
        assert!(load_works(&f).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn titulo_e_tempo() {
        let a = Agent {
            pane_id: "p".into(),
            tab_id: "t".into(),
            workspace_id: "w".into(),
            agent: "claude".into(),
            agent_status: Status::Idle,
            terminal_title_stripped: Some("◑ revisão".into()),
            cwd: "/".into(),
            focused: false,
        };
        assert_eq!(title(&a), "revisão");
        assert_eq!(ago(Duration::from_secs(30)), "agora");
        assert_eq!(ago(Duration::from_secs(180)), "há 3 min");
        assert_eq!(ago(Duration::from_secs(7300)), "há 2 h");
    }
}
