use std::{path::PathBuf, sync::Arc, time::Duration};

use gpui_kit::{
    component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp, OutdentInline},
    prelude::FluentBuilder,
    *,
};

use crate::{
    feature::{self, Inventory, Request},
    form::{FieldEvent, TextField, button, checkbox},
    herdr::Herdr,
    jira,
    sidebar::tilde,
    theme::Theme,
};

pub enum Event {
    /// o aviso de alguma feature criada sem hooks
    Done(Option<String>),
}

const ATTEMPTS: &[&str] = &["1", "2 (comparar no fim)", "3 (comparar no fim)"];
const MODELS: &[&str] = &["mesmo modelo", "variar entre Opus e Sonnet"];

/// O `newFeatureDlg` do protótipo.
pub struct NewFeature {
    herdr: Arc<Herdr>,
    repo: PathBuf,
    jira: Entity<TextField>,
    branch: Entity<TextField>,
    prompt: Entity<TextField>,
    bases: Vec<String>,
    base: usize,
    /// "Em cima de": as features deste repo (0 = nenhuma)
    parents: Vec<Inventory>,
    parent: usize,
    attempts: usize,
    model: usize,
    open: Option<&'static str>,
    copy_env: bool,
    setup: bool,
    claude: bool,
    env_files: Vec<String>,
    setup_cmd: Option<String>,
    registry_auth: bool,
    jira_info: Option<(String, bool)>,
    /// a busca falhou: mostra "↻ tentar de novo"
    jira_failed: bool,
    /// o `@` do prompt: (início do `@`, caminhos, escolhido)
    at: Option<(usize, Vec<String>, usize)>,
    paths: Vec<String>,
    /// os outros repos (caminho absoluto): o `@` mais comum
    repos: Vec<String>,
    err: String,
    busy: bool,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Event> for NewFeature {}

impl NewFeature {
    pub fn new(
        herdr: Arc<Herdr>,
        repo: PathBuf,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |v: &str, ph: &'static str, multi: bool, mono: bool, cx: &mut Context<Self>| {
            let t = theme.clone();
            cx.new(|cx| TextField::new(t, v, ph, multi, mono, cx))
        };
        let jira = field("", "ex.: PROJ-123", false, true, cx);
        let branch = field("feat/", "", false, true, cx);
        let prompt = field(
            "",
            "ex.: adiciona paginação no GET /pedidos",
            true,
            false,
            cx,
        );
        for f in [&jira, &branch, &prompt] {
            cx.subscribe(f, |this, f, e: &FieldEvent, cx| match e {
                FieldEvent::Changed => {
                    this.err.clear();
                    if f == this.prompt {
                        this.update_at(cx);
                    }
                    cx.notify();
                }
                FieldEvent::Submit if f == this.jira => this.lookup_jira(false, cx),
                FieldEvent::Submit => this.create(cx),
                FieldEvent::Cancel => cx.emit(Event::Done(None)),
            })
            .detach();
        }
        let jf = jira.read(cx).focus_handle();
        cx.on_focus_out(&jf, window, |this, _, _, cx| this.lookup_jira(false, cx))
            .detach();
        window.focus(&jf, cx);
        let (r, r2) = (repo.clone(), repo.clone());
        cx.spawn(async move |this, cx| {
            let ps = cx
                .background_executor()
                .spawn(async move { feature::paths(&r) })
                .await;
            let rs = cx
                .background_executor()
                .spawn(async move { feature::repos(&crate::mrs::repos_root(), &r2) })
                .await;
            let _ = this.update(cx, |this, _| {
                this.paths = ps;
                this.repos = rs;
            });
        })
        .detach();
        let parents = feature::inventories()
            .into_iter()
            .filter(|i| i.repo == repo && i.group.is_none() && i.worktrees[0].path.exists())
            .collect();
        Self {
            parents,
            parent: 0,
            bases: feature::bases(&repo),
            env_files: feature::env_files(&repo),
            setup_cmd: feature::setup_command(&repo),
            registry_auth: feature::registry_auth(&repo).is_some(),
            herdr,
            repo,
            jira,
            branch,
            prompt,
            base: 0,
            attempts: 0,
            model: 0,
            open: None,
            copy_env: true,
            setup: true,
            claude: true,
            jira_info: None,
            jira_failed: false,
            at: None,
            paths: Vec::new(),
            repos: Vec::new(),
            err: String::new(),
            busy: false,
            focus: cx.focus_handle(),
            theme,
        }
    }

    fn repo_name(&self) -> String {
        self.repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn ports(&self, n: usize) -> Vec<u16> {
        let mut used: Vec<u16> = feature::inventories().iter().map(|i| i.port).collect();
        (0..n)
            .map(|_| {
                let p = feature::next_port(&used);
                used.push(p);
                p
            })
            .collect()
    }

    /// A mãe escolhida em "Em cima de".
    fn mother(&self) -> Option<&Inventory> {
        self.parent.checked_sub(1).and_then(|i| self.parents.get(i))
    }

    fn base_name(&self) -> Option<String> {
        self.mother()
            .map(|m| m.branch.clone())
            .or_else(|| self.bases.get(self.base).cloned())
    }

    fn preview(&self, cx: &App) -> String {
        let b = self.branch.read(cx).value.clone();
        let n = self.attempts + 1;
        let ports = self.ports(n);
        let path = feature::worktree_path(&self.repo, if b.is_empty() { "…" } else { &b });
        let base = self.base_name().unwrap_or_else(|| "…".into());
        format!(
            "worktree {}{} --base {}{} · PORT={}{} · docker compose -p {}",
            tilde(&path.to_string_lossy()),
            if n > 1 {
                format!(" (×{n}: -a, -b{})", if n > 2 { ", -c" } else { "" })
            } else {
                String::new()
            },
            base,
            if self.mother().is_some() && n == 1 {
                format!(" · MR → {base}")
            } else {
                String::new()
            },
            ports[0],
            if n > 1 {
                format!("…{}", ports[n - 1])
            } else {
                String::new()
            },
            feature::compose_project(&self.repo, if b.is_empty() { "…" } else { &b }),
        )
    }

    /// `retry`: a segunda tentativa, depois de o MCP do Atlassian falhar (ele às vezes ainda está conectando).
    /// Vem preenchido de fora (a ACR de um PRC): key, branch e pedido; a busca no Jira segue igual.
    pub fn prefill(&mut self, jira: &str, branch: &str, prompt: &str, cx: &mut Context<Self>) {
        self.jira.update(cx, |f, cx| f.set(jira.to_string(), cx));
        self.branch
            .update(cx, |f, cx| f.set(branch.to_string(), cx));
        self.prompt
            .update(cx, |f, cx| f.set(prompt.to_string(), cx));
        self.lookup_jira(false, cx);
    }

    fn lookup_jira(&mut self, retry: bool, cx: &mut Context<Self>) {
        let raw = self.jira.read(cx).value.clone();
        if raw.trim().is_empty() {
            self.jira_info = None;
            self.jira_failed = false;
            return cx.notify();
        }
        let Some(key) = jira::key_from(&raw) else {
            self.jira_info = Some((
                "Formato esperado: PROJ-123 ou o link da issue".into(),
                false,
            ));
            return cx.notify();
        };
        if raw != key {
            self.jira.update(cx, |f, cx| f.set(key.clone(), cx));
        }
        if !retry && matches!(&self.jira_info, Some((s, _)) if s.contains(&key)) {
            return;
        }
        self.jira_failed = false;
        self.jira_info = Some((format!("buscando {key} no Jira…"), false));
        cx.notify();
        let k = key.clone();
        cx.spawn(async move |this, cx| {
            let issue = cx.background_executor().spawn(async move { jira::fetch(&k) }).await;
            if let Err(e) = &issue
                && !retry
                && unavailable(e)
            {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = this.update(cx, |this, cx| this.lookup_jira(true, cx));
                return;
            }
            let _ = this.update(cx, |this, cx| {
                match issue {
                    Ok(i) => {
                        this.jira_info = Some((format!("✓ {key} · {} · {} (MCP Atlassian)", i.kind, i.title), true));
                        let b = this.branch.read(cx).value.clone();
                        if b.is_empty() || b == "feat/" {
                            let s = feature::slug(&i.title);
                            let s = s.get(..s.len().min(28)).unwrap_or(&s).trim_end_matches('-').to_string();
                            this.branch.update(cx, |f, cx| f.set(format!("feat/{key}-{s}"), cx));
                        }
                        if this.prompt.read(cx).value.is_empty() {
                            let p = format!(
                                "Implementar {key}: {}.\nCritérios: {}.\nLeia a issue no Jira antes de começar e não commite.",
                                i.title, i.criteria
                            );
                            this.prompt.update(cx, |f, cx| f.set(p, cx));
                        }
                    }
                    Err(e) => {
                        this.jira_failed = true;
                        this.jira_info = Some((
                            if unavailable(&e) {
                                format!("{key}: Jira indisponível agora (MCP Atlassian)")
                            } else {
                                format!("{key}: {}", e.lines().next().unwrap_or_default())
                            },
                            false,
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let br = self.branch.read(cx).value.trim().to_string();
        if br.is_empty() || br.ends_with('/') {
            self.err = "Dê um nome à branch, ex.: feat/paginacao-bots".into();
            return cx.notify();
        }
        let Some(base) = self.base_name() else {
            self.err = "O repositório não tem branch para servir de base.".into();
            return cx.notify();
        };
        let n = self.attempts + 1;
        let models: [Option<&'static str>; 3] = if self.model == 1 {
            [Some("opus"), Some("sonnet"), Some("opus[1m]")]
        } else {
            [None; 3]
        };
        let prompt = self.prompt.read(cx).value.clone();
        let key = jira::key_from(&self.jira.read(cx).value);
        let reqs: Vec<Request> = (0..n)
            .map(|i| Request {
                repo: self.repo.clone(),
                branch: if n > 1 {
                    format!("{br}-{}", ['a', 'b', 'c'][i])
                } else {
                    br.clone()
                },
                base: base.clone(),
                prompt: prompt.clone(),
                copy_env: self.copy_env,
                setup: self.setup,
                claude: self.claude,
                model: models[i],
                group: (n > 1).then(|| br.clone()),
                // ponytail: outros repos entram depois, pela paleta ("Feature: adicionar repo…")
                others: Vec::new(),
                parent: self.mother().filter(|_| n == 1).cloned(),
                jira: key.clone(),
            })
            .collect();
        let ports = self.ports(n);
        self.busy = true;
        cx.notify();
        let herdr = self.herdr.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    reqs.iter()
                        .zip(ports)
                        .map(|(r, p)| feature::create(&herdr, r, p))
                        .collect::<Result<Vec<_>, _>>()
                        .map(|w| w.into_iter().flatten().next())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(w) => cx.emit(Event::Done(w)),
                    Err(e) => this.err = e,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cycle(&mut self, step: usize, window: &mut Window, cx: &mut Context<Self>) {
        let order = [&self.jira, &self.branch, &self.prompt].map(|f| f.read(cx).focus_handle());
        let i = order
            .iter()
            .position(|f| f.contains_focused(window, cx))
            .unwrap_or(0);
        window.focus(&order[(i + step) % 3], cx);
        self.open = None;
        cx.notify();
    }

    /// O `@` que termina no cursor do prompt abre a lista de caminhos do repo.
    fn update_at(&mut self, cx: &mut Context<Self>) {
        let f = self.prompt.read(cx);
        let before = f.value.get(..f.cursor(cx)).unwrap_or_default();
        // o @ que abre a menção vem depois de espaço; o HOME pode ter @ no meio do caminho
        let at = before
            .rmatch_indices('@')
            .map(|(i, _)| i)
            .find(|&i| before[..i].chars().last().is_none_or(char::is_whitespace))
            .filter(|&i| !before[i + 1..].contains(char::is_whitespace));
        self.at = at
            .map(|i| {
                let q = &before[i + 1..];
                let hits: Vec<String> = if q.starts_with(['/', '~']) {
                    feature::disk_matching(q, 8)
                } else {
                    let name = |p: &String| {
                        p.trim_end_matches('/')
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_lowercase()
                    };
                    self.repos
                        .iter()
                        .filter(|p| name(p).contains(&q.to_lowercase()))
                        .take(3)
                        .chain(feature::matching(&self.paths, q, 8))
                        .take(8)
                        .cloned()
                        .collect()
                };
                (i, hits, 0)
            })
            .filter(|(_, hits, _)| !hits.is_empty());
    }

    fn pick_at(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((start, hits, i)) = self.at.take() else {
            return;
        };
        let p = &hits[i];
        let text = if p.ends_with('/') {
            format!("@{p}")
        } else {
            format!("@{p} ")
        };
        let end = self.prompt.read(cx).cursor(cx);
        self.prompt
            .update(cx, |f, cx| f.splice(start..end, &text, window, cx));
    }

    /// Ações do input focado (setas, Enter, Tab, Esc) passam aqui antes dele: o `@` e o Tab entre campos.
    fn on_input_action(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        // o cursor pode ter andado (Home, clique) ou o foco saído do prompt sem um Changed
        let prompt_focused = self
            .prompt
            .read(cx)
            .focus_handle()
            .contains_focused(window, cx);
        if let Some(sel) = self.at.take().map(|a| a.2)
            && prompt_focused
        {
            self.update_at(cx);
            if let Some(a) = &mut self.at {
                a.2 = sel.min(a.1.len() - 1);
            }
        }
        if let Some((_, hits, i)) = &mut self.at {
            let n = hits.len();
            match key {
                "down" => *i = (*i + 1) % n,
                "up" => *i = (*i + n - 1) % n,
                "enter" | "tab" => self.pick_at(window, cx),
                "escape" => self.at = None,
                _ => return,
            }
        } else {
            match key {
                "tab" => self.cycle(1, window, cx),
                "shift-tab" => self.cycle(2, window, cx),
                "escape" if self.open.is_some() => self.open = None,
                "escape" => cx.emit(Event::Done(None)),
                _ => return,
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "tab" => self.cycle(if k.modifiers.shift { 2 } else { 1 }, window, cx),
            "escape" if self.open.is_some() => self.open = None,
            "escape" => cx.emit(Event::Done(None)),
            "enter" if k.modifiers.control => self.create(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn label(&self, text: &str) -> Div {
        div()
            .text_size(px(13.))
            .text_color(self.theme.muted)
            .child(text.to_string())
    }

    fn select(
        &self,
        id: &'static str,
        options: Vec<String>,
        selected: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let open = self.open == Some(id);
        div()
            .relative()
            .child(
                div()
                    .id(id)
                    .flex()
                    .justify_between()
                    .items_center()
                    .px(px(8.))
                    .py(px(6.))
                    .bg(t.win)
                    .border_1()
                    .border_color(if open { t.accent } else { t.line })
                    .rounded(px(5.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open = if this.open == Some(id) {
                            None
                        } else {
                            Some(id)
                        };
                        cx.notify();
                    }))
                    .child(options.get(selected).cloned().unwrap_or_default())
                    .child(div().text_color(t.muted).child("▾")),
            )
            .when(open, |d| {
                let mut list = div()
                    .absolute()
                    .top(px(34.))
                    .left_0()
                    .w_full()
                    .py(px(4.))
                    .bg(t.raise)
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(5.))
                    .shadow_lg();
                for (i, o) in options.iter().enumerate() {
                    list = list.child(
                        div()
                            .id(i)
                            .px(px(8.))
                            .py(px(4.))
                            .cursor_pointer()
                            .when(i == selected, |d| d.bg(t.accent_soft))
                            .hover(|d| d.bg(t.accent_soft))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match id {
                                    "base" => this.base = i,
                                    "parent" => this.parent = i,
                                    "attempts" => this.attempts = i,
                                    _ => this.model = i,
                                }
                                this.open = None;
                                cx.notify();
                            }))
                            .child(o.clone()),
                    );
                }
                d.child(deferred(list))
            })
    }

    fn check(
        &self,
        id: &'static str,
        on: bool,
        label: impl IntoElement,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .gap(px(8.))
            .items_center()
            .when(enabled, |d| d.cursor_pointer())
            .when(!enabled, |d| d.opacity(0.45))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !enabled {
                    return;
                }
                match id {
                    "env" => this.copy_env = !this.copy_env,
                    "setup" => this.setup = !this.setup,
                    _ => this.claude = !this.claude,
                }
                cx.notify();
            }))
            .child(checkbox(&self.theme, on && enabled))
            .child(div().flex_1().min_w_0().child(label))
    }
}

impl Render for NewFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.at.is_some()
            && !self
                .prompt
                .read(cx)
                .focus_handle()
                .contains_focused(window, cx)
        {
            self.at = None;
        }
        let t = self.theme.clone();
        let mono = |s: String| div().font_family(t.mono.clone()).child(s);
        let row2 = |a: Div, b: Div| {
            div()
                .flex()
                .gap(px(10.))
                .child(a.flex_basis(relative(0.66)).min_w_0())
                .child(b.flex_basis(relative(0.34)).min_w_0())
        };
        let fld = |label: Div, input: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(label)
                .child(input)
        };
        let env_label = if self.env_files.is_empty() {
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .child("Copiar")
                .child(mono(".env".into()))
                .child(
                    div()
                        .text_color(t.muted)
                        .child("(nenhum ignorado pelo git)"),
                )
        } else {
            let names = self.env_files.join(", ");
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .child("Copiar")
                .child(mono(names))
                .child("(ignorados pelo git)")
        };
        let setup_label = match &self.setup_cmd {
            Some(c) => div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .child("Rodar")
                .child(mono(c.clone()))
                .child(div().text_color(t.muted).child(if self.registry_auth {
                    "(renova antes o token do Artifact Registry no ~/.npmrc)"
                } else {
                    "(setup do repo, vem do .maestro.toml)"
                })),
            None => div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .child("Rodar o setup")
                .child(
                    div()
                        .text_color(t.muted)
                        .child("(sem lockfile nem .maestro.toml)"),
                ),
        };
        let bases = match self.mother() {
            Some(m) => vec![m.branch.clone()],
            None => self.bases.clone(),
        };
        let base_sel = if self.mother().is_some() {
            0
        } else {
            self.base
        };
        let parents: Vec<String> = std::iter::once("nenhuma: direto na base acima".to_string())
            .chain(self.parents.iter().map(|i| i.branch.clone()))
            .collect();
        div()
            .track_focus(&self.focus)
            .key_context("NewFeature")
            // clicar num seletor ou checkbox não pode tirar o foco do diálogo (Esc e Ctrl+⏎ param de chegar)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &MoveDown, w, cx| this.on_input_action("down", w, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &MoveUp, w, cx| this.on_input_action("up", w, cx)),
            )
            .capture_action(cx.listener(|this, e: &Enter, w, cx| {
                if e.secondary {
                    this.create(cx);
                    cx.stop_propagation();
                } else if !e.shift {
                    this.on_input_action("enter", w, cx);
                }
            }))
            .capture_action(
                cx.listener(|this, _: &IndentInline, w, cx| this.on_input_action("tab", w, cx)),
            )
            .capture_action(cx.listener(|this, _: &OutdentInline, w, cx| {
                this.on_input_action("shift-tab", w, cx)
            }))
            .capture_action(
                cx.listener(|this, _: &Escape, w, cx| this.on_input_action("escape", w, cx)),
            )
            // fora de um campo (seletor, caixa) as teclas chegam cruas
            .capture_key_down(cx.listener(Self::on_key_down))
            .w(px(480.))
            .max_w_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Nova feature em {}", self.repo_name())),
            )
            .child(fld(
                self.label("Issue do Jira (opcional: preenche branch e tarefa)"),
                self.jira.clone().into_any_element(),
            ))
            .when_some(self.jira_info.clone(), |d, (s, ok)| {
                d.child(
                    div()
                        .mt(px(-6.))
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .text_color(if ok { t.ok } else { t.muted })
                        .child(s)
                        .when(self.jira_failed, |d| {
                            d.child(
                                div()
                                    .id("jira-retry")
                                    .cursor_pointer()
                                    .text_color(t.accent)
                                    .child("↻ tentar de novo")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.lookup_jira(true, cx)),
                                    ),
                            )
                        }),
                )
            })
            .child(row2(
                fld(self.label("Branch"), self.branch.clone().into_any_element()),
                fld(
                    self.label("A partir de"),
                    div()
                        .when(self.mother().is_some(), |d| d.opacity(0.45))
                        .child(self.select("base", bases, base_sel, cx))
                        .into_any_element(),
                ),
            ))
            .child(fld(
                self.label(
                    "Em cima de (MRs empilhados: a base e o alvo do MR viram a branch da mãe)",
                ),
                div()
                    .when(self.parents.is_empty(), |d| d.opacity(0.45))
                    .child(self.select("parent", parents, self.parent, cx))
                    .into_any_element(),
            ))
            .child(fld(
                div()
                    .flex()
                    .gap(px(4.))
                    .child(self.label("O que o Claude deve fazer (opcional ·"))
                    .child(mono("@".into()).text_size(px(13.)).text_color(t.muted))
                    .child(self.label("menciona pasta ou arquivo)")),
                div()
                    .relative()
                    .child(self.prompt.clone())
                    .when_some(self.at.clone(), |d, (_, hits, sel)| {
                        let mut list = div()
                            .absolute()
                            .top_full()
                            .left_0()
                            .right_0()
                            .mt(px(2.))
                            .py(px(3.))
                            .bg(t.raise)
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(6.))
                            .shadow_lg()
                            .font_family(t.mono.clone())
                            .text_size(px(12.5));
                        for (i, p) in hits.into_iter().enumerate() {
                            list = list.child(
                                div()
                                    .id(("at", i))
                                    .flex()
                                    .gap(px(10.))
                                    .px(px(10.))
                                    .cursor_pointer()
                                    .when(i == sel, |d| d.bg(t.accent_soft))
                                    .hover(|d| d.bg(t.accent_soft))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            if let Some(a) = &mut this.at {
                                                a.2 = i;
                                            }
                                            this.pick_at(window, cx);
                                        }),
                                    )
                                    .child(
                                        div()
                                            .w(px(14.))
                                            .text_color(t.accent)
                                            .child(if p.ends_with('/') { "▸" } else { "·" }),
                                    )
                                    .child(div().min_w_0().truncate().child(tilde(&p))),
                            );
                        }
                        d.child(deferred(list).with_priority(1))
                    })
                    .into_any_element(),
            ))
            .child(row2(
                fld(
                    self.label("Tentativas em paralelo"),
                    self.select(
                        "attempts",
                        ATTEMPTS.iter().map(|s| s.to_string()).collect(),
                        self.attempts,
                        cx,
                    )
                    .into_any_element(),
                ),
                fld(
                    self.label("Modelos"),
                    self.select(
                        "model",
                        MODELS.iter().map(|s| s.to_string()).collect(),
                        self.model,
                        cx,
                    )
                    .into_any_element(),
                ),
            ))
            .child(
                div()
                    .font_family(t.mono.clone())
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .bg(t.win)
                    .rounded(px(5.))
                    .px(px(8.))
                    .py(px(6.))
                    .overflow_hidden()
                    .child(self.preview(cx)),
            )
            .child(
                div()
                    .relative()
                    .border_1()
                    .border_color(t.line)
                    .rounded(px(6.))
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .absolute()
                            .top(px(-9.))
                            .left(px(8.))
                            .px(px(4.))
                            .bg(t.raise)
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .child("Ao criar"),
                    )
                    .child(self.check(
                        "env",
                        self.copy_env,
                        env_label,
                        !self.env_files.is_empty(),
                        cx,
                    ))
                    .child(self.check(
                        "setup",
                        self.setup,
                        setup_label,
                        self.setup_cmd.is_some(),
                        cx,
                    ))
                    .child(
                        self.check(
                            "claude",
                            self.claude,
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(4.))
                                .child("Abrir o")
                                .child(mono("claude".into()))
                                .child("em modo plano")
                                .child(
                                    div()
                                        .text_color(t.muted)
                                        .child("(só implementa quando você aprovar o plano)"),
                                ),
                            true,
                            cx,
                        ),
                    ),
            )
            .when(!self.err.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(0xff5555))
                        .child(self.err.clone()),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        button(&t, "nf-no", "Cancelar", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Done(None)))),
                    )
                    .child(
                        button(
                            &t,
                            "nf-ok",
                            if self.busy { "Criando…" } else { "Criar" },
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.create(cx)))
                        .child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(12.))
                                .px(px(4.))
                                .border_1()
                                .border_color(rgb(0x282a36))
                                .rounded(px(3.))
                                .child("Ctrl+⏎"),
                        ),
                    ),
            )
    }
}

/// O MCP do Atlassian fora do ar ou ainda conectando, não uma issue que não existe.
fn unavailable(e: &str) -> bool {
    let e = e.to_lowercase();
    ["mcp", "atlassian", "conect", "connect", "oauth", "indispon"]
        .iter()
        .any(|w| e.contains(w))
}
