use std::{collections::HashMap, ops::Range, time::Duration};

use gpui_kit::{
    component::input::{Editor, EditorState, InputEvent},
    prelude::FluentBuilder,
    *,
};

use crate::{
    form::{FieldEvent, TextField, button, checkbox},
    graph::{self, Graph},
    notes,
    review::Redo,
    sidebar::Sidebar,
    theme::{self, Theme},
    tour::{self, Anchors},
    vault::{self, Note},
};

/// A nota nova antes do Enter do título: ainda sem arquivo.
const NEW: &str = "notas/…";

pub enum Event {
    Toast(String, Option<Redo>),
    Focus(String),
}

#[derive(Clone, PartialEq)]
enum View {
    Note(String),
    Graph,
    Log,
}

/// Uma feature ativa como o vault vê: (branch, repo, pane, MR).
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureRef {
    pub branch: String,
    pub repo: String,
    pub pane: Option<String>,
    /// MR ainda aberto (o grafo só liga esses)
    pub mr: Option<u64>,
    /// para o resumo do dia: (estado, MR com "mergeado", custo)
    pub status: String,
    pub merged: Option<u64>,
    pub cost: f64,
}

/// O modo Vault do protótipo (`renderVaultTree` + `renderVaultMain`).
#[derive(Clone, Copy)]
struct LeftDrag;

pub struct VaultView {
    notes: Vec<Note>,
    view: View,
    query: Entity<TextField>,
    editing: Option<Entity<EditorState>>,
    /// o texto quando a edição começou: ao sair dela, um "editado por você" com Desfazer
    snap: Option<String>,
    edit_key: String,
    /// o texto em disco da nota em edição
    saved: String,
    formatting: Option<String>,
    _save: Option<Task<()>>,
    summary: Option<String>,
    graph: Entity<Graph>,
    sidebar: Option<Entity<Sidebar>>,
    focus: FocusHandle,
    left_w: f32,
    theme: Theme,
    anchors: Anchors,
    _poll: Task<()>,
}

impl EventEmitter<Event> for VaultView {}

/// O `inl()` do `vmd`: `código`, **negrito** e [[link]], com as faixas de cada um no texto sem marcação.
#[derive(Debug, PartialEq)]
pub enum Span {
    Code,
    Bold,
    Link(String),
}

pub fn inline(src: &str) -> (String, Vec<(Range<usize>, Span)>) {
    let mut out = String::new();
    let mut spans = Vec::new();
    let mut rest = src;
    loop {
        let next = [
            rest.find('`').map(|i| (i, "`", "`")),
            rest.find("**").map(|i| (i, "**", "**")),
            rest.find("[[").map(|i| (i, "[[", "]]")),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|x| x.0);
        let Some((i, open, close)) = next else {
            out.push_str(rest);
            break;
        };
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(close) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..i]);
        let inner = &after[..j];
        let start = out.len();
        match open {
            "`" => {
                out.push_str(inner);
                spans.push((start..out.len(), Span::Code));
            }
            "**" => {
                out.push_str(inner);
                spans.push((start..out.len(), Span::Bold));
            }
            _ => {
                let name = inner
                    .split(['|', '#'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                out.push_str(&format!("[[{inner}]]"));
                spans.push((start..out.len(), Span::Link(name)));
            }
        }
        rest = &after[j + close.len()..];
    }
    (out, spans)
}

pub fn signature(root: &std::path::Path) -> u64 {
    let mut sig = 0u64;
    let mut walk = vec![root.to_path_buf()];
    while let Some(d) = walk.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                walk.push(p);
            } else if let Ok(m) = e.metadata() {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos() as u64);
                sig = sig.wrapping_add(t ^ m.len()).rotate_left(5);
            }
        }
    }
    sig
}

impl VaultView {
    pub fn new(
        sidebar: Option<Entity<Sidebar>>,
        theme: Theme,
        anchors: Anchors,
        cx: &mut Context<Self>,
    ) -> Self {
        let t = theme.clone();
        let query = cx.new(|cx| TextField::new(t, "", "buscar nota…", false, false, cx));
        cx.subscribe(&query, |_, _, e: &FieldEvent, cx| {
            if matches!(e, FieldEvent::Changed) {
                cx.notify();
            }
        })
        .detach();
        let graph = cx.new(|cx| Graph::new(theme.clone(), cx));
        cx.subscribe(&graph, |this, _, e: &graph::Event, cx| match e {
            graph::Event::Open(k) => this.open(k.clone(), cx),
            graph::Event::Focus(p) => cx.emit(Event::Focus(p.clone())),
            graph::Event::Toast(m) => cx.emit(Event::Toast(m.clone(), None)),
        })
        .detach();
        // ponytail: relê o vault quando muda alguma data de arquivo (a cada 2 s); `notify` se o vault crescer muito
        let poll = cx.spawn(async move |this, cx| {
            let mut last = 0;
            loop {
                let (sig, notes) = cx
                    .background_executor()
                    .spawn(async move {
                        let root = vault::dir();
                        let sig = signature(&root);
                        (sig, (sig != last).then(|| vault::load(&root)))
                    })
                    .await;
                last = sig;
                if let Some(notes) = notes
                    && this
                        .update(cx, |this, cx| {
                            this.notes = notes;
                            this.refresh_graph(cx);
                            cx.notify();
                        })
                        .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            notes: Vec::new(),
            view: View::Note(String::new()),
            query,
            editing: None,
            snap: None,
            edit_key: String::new(),
            saved: String::new(),
            formatting: None,
            _save: None,
            summary: None,
            graph,
            sidebar,
            focus: cx.focus_handle(),
            left_w: 270.,
            theme,
            anchors,
            _poll: poll,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn features(&self, cx: &App) -> Vec<FeatureRef> {
        self.sidebar
            .as_ref()
            .map(|s| s.read(cx).feature_refs())
            .unwrap_or_default()
    }

    fn refresh_graph(&mut self, cx: &mut Context<Self>) {
        let feats = self.features(cx);
        let (notes, cur) = (self.notes.clone(), self.current_name());
        self.graph
            .update(cx, |g, cx| g.set_data(&notes, &feats, cur, cx));
    }

    fn today_key() -> String {
        format!("daily/{}.md", vault::today())
    }

    fn current_name(&self) -> String {
        match &self.view {
            View::Note(k) => k
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .trim_end_matches(".md")
                .into(),
            _ => String::new(),
        }
    }

    /// A nota de abertura do protótipo é a do repo em foco; sem ela, a daily.
    pub fn default_note(&mut self, repo: Option<String>) {
        if matches!(&self.view, View::Note(k) if k.is_empty()) {
            let k = repo
                .map(|r| format!("repos/{r}.md"))
                .filter(|k| vault::dir().join(k).exists())
                .unwrap_or_else(Self::today_key);
            self.view = View::Note(k);
        }
    }

    /// Os "Vault: …" da paleta.
    pub fn go(
        &mut self,
        to: &crate::palette_cmd::VaultGo,
        repo: Option<String>,
        cx: &mut Context<Self>,
    ) {
        use crate::palette_cmd::VaultGo::*;
        match to {
            Graph => {
                self.view = View::Graph;
                if let Some(r) = repo {
                    self.graph.update(cx, |g, _| g.set_center(r));
                }
                self.refresh_graph(cx);
            }
            Today => self.open(Self::today_key(), cx),
            Log => self.view = View::Log,
            RepoNote => match repo {
                Some(r) => self.open(format!("repos/{r}.md"), cx),
                None => self.open(Self::today_key(), cx),
            },
            Open => self.default_note(repo),
            NewNote => {}
        }
        cx.notify();
    }

    /// O foco ao entrar no modo: o editor, se houver um aberto.
    pub fn focus_target(&self, cx: &App) -> FocusHandle {
        self.editing
            .as_ref()
            .map_or_else(|| self.focus.clone(), |e| e.read(cx).focus_handle(cx))
    }

    fn open(&mut self, key: String, cx: &mut Context<Self>) {
        self.end_edit(cx);
        self.view = View::Note(key);
        self.summary = None;
        self.refresh_graph(cx);
        cx.notify();
    }

    fn open_link(&mut self, name: &str, cx: &mut Context<Self>) {
        match vault::find(&self.notes, name) {
            Some(n) => self.open(n.key.clone(), cx),
            None => cx.emit(Event::Toast(
                format!("[[{name}]] ainda não existe: link quebrado é TODO, não erro."),
                None,
            )),
        }
    }

    fn write(&mut self, rel: &str, text: String, desc: String, cx: &mut Context<Self>) {
        match vault::write(rel, &text, desc) {
            Ok(w) => {
                if let Some(n) = self.notes.iter_mut().find(|n| n.key == rel) {
                    n.text = text;
                } else {
                    self.notes = vault::load(&vault::dir());
                }
                cx.emit(Event::Toast(
                    format!("vault: {}", w.desc),
                    Some(Redo::Vault(w)),
                ));
            }
            Err(e) => cx.emit(Event::Toast(
                format!("vault: não deu para salvar ({e})"),
                None,
            )),
        }
        cx.notify();
    }

    fn start_edit(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = match key {
            NEW => "# ".to_string(),
            k => self
                .notes
                .iter()
                .find(|n| n.key == k)
                .map(|n| n.text.clone())
                .unwrap_or_default(),
        };
        let e = cx.new(|cx| {
            EditorState::new(window, cx)
                .soft_wrap(true)
                .default_value(text.clone())
        });
        cx.subscribe(&e, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this._save = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(800))
                        .await;
                    let _ = this.update(cx, |this, cx| this.autosave(cx));
                }));
                cx.notify();
            }
        })
        .detach();
        if key == NEW {
            e.update(cx, |s, cx| s.set_selected_range(2..2, cx));
        }
        let f = e.read(cx).focus_handle(cx);
        window.focus(&f, cx);
        self.snap = (key != NEW).then(|| text.clone());
        self.saved = if key == NEW { String::new() } else { text };
        self.edit_key = key.to_string();
        self.editing = Some(e);
        cx.notify();
    }

    /// Alt+7: uma nota em `notas/`, que só ganha arquivo no Enter do título.
    pub fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(NEW.into(), cx);
        self.start_edit(NEW, window, cx);
    }

    fn editing_key(&self) -> Option<String> {
        self.editing.as_ref().map(|_| self.edit_key.clone())
    }

    /// Salva sem linha na Atividade; a nota nova vira `notas/<título>.md` no primeiro Enter.
    fn autosave(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(e)) = (self.editing_key(), &self.editing) else {
            return;
        };
        let text = e.read(cx).value().to_string();
        if text == self.saved {
            return;
        }
        let new = key == NEW;
        if new && !notes::ready(&text) {
            return;
        }
        let key = if new {
            notes::free_key(&vault::dir(), &text)
        } else {
            key
        };
        if let Err(e) = vault::save(&key, &text) {
            return cx.emit(Event::Toast(
                format!("vault: não deu para salvar ({e})"),
                None,
            ));
        }
        self.saved = text.clone();
        match self.notes.iter_mut().find(|n| n.key == key) {
            Some(n) => n.text = text,
            None => self.notes = vault::load(&vault::dir()),
        }
        if new {
            if self.view == View::Note(NEW.into()) {
                self.view = View::Note(key.clone());
            }
            self.edit_key = key.clone();
            let w = vault::edited(&key, None, format!("{key} criada por você"));
            cx.emit(Event::Toast(
                format!("vault: {}", w.desc),
                Some(Redo::Vault(w)),
            ));
        }
        cx.notify();
    }

    /// Fecha o editor: o que mudou desde que ele abriu vai para a Atividade, com Desfazer.
    fn end_edit(&mut self, cx: &mut Context<Self>) {
        self.autosave(cx);
        self._save = None;
        if self.editing.take().is_none() {
            return;
        }
        let key = std::mem::take(&mut self.edit_key);
        if let Some(before) = self.snap.take()
            && before != self.saved
        {
            let w = vault::edited(&key, Some(before), format!("{key} editado por você"));
            cx.emit(Event::Toast(
                format!("vault: {}", w.desc),
                Some(Redo::Vault(w)),
            ));
        }
        if key == NEW && self.view == View::Note(NEW.into()) {
            self.view = View::Note(Self::today_key());
        }
    }

    /// O `✦ Formatar com IA`: o Haiku arruma o Markdown; o Desfazer devolve o texto de antes.
    pub fn format(&mut self, cx: &mut Context<Self>) {
        let View::Note(key) = self.view.clone() else {
            return;
        };
        if self.formatting.is_some() || key == Self::today_key() {
            return;
        }
        if key == NEW {
            return cx.emit(Event::Toast(
                "Escreva o título e dê Enter: a nota ganha o arquivo e aí dá para formatar".into(),
                None,
            ));
        }
        self.end_edit(cx);
        let Some(text) = self
            .notes
            .iter()
            .find(|n| n.key == key)
            .map(|n| n.text.clone())
            .filter(|t| !notes::title(t).is_empty())
        else {
            return cx.emit(Event::Toast("Nada para formatar ainda".into(), None));
        };
        self.formatting = Some(key.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { notes::format(&text) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.formatting = None;
                match r {
                    Ok((md, tokens)) => this.write(
                        &key,
                        md,
                        format!(
                            "{key} formatada com IA (~{} mil tokens)",
                            tokens.div_ceil(1000)
                        ),
                        cx,
                    ),
                    Err(e) => {
                        cx.emit(Event::Toast(format!("Formatar com IA falhou: {e}"), None));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// O `vSum` do protótipo: uma prévia, que só vai para a daily no "Salvar".
    fn summary_text(&self, cx: &App) -> String {
        let feats = self.features(cx);
        if feats.is_empty() {
            return "- nenhuma feature ativa hoje".into();
        }
        // o formato do `vSum` do protótipo
        let mut lines: Vec<String> = feats
            .iter()
            .map(|f| {
                let mr = match (f.mr, f.merged) {
                    (Some(m), _) => format!(" · MR !{m}"),
                    (None, Some(m)) => format!(" · MR !{m} mergeado"),
                    _ => String::new(),
                };
                format!(
                    "- {}: {}{mr} · US$ {}",
                    f.branch,
                    f.status,
                    crate::sidebar::brl(f.cost)
                )
            })
            .collect();
        let today = self
            .sidebar
            .as_ref()
            .and_then(|s| s.read(cx).top_repo())
            .and_then(|r| crate::audit::latest(&r))
            .filter(|r| r.date == crate::vault::today());
        lines.push(match today {
            Some(r) if r.kind == "full" => "- auditoria: completa rodada hoje".into(),
            Some(_) => "- auditoria: incremental rodada hoje".into(),
            None => "- auditoria: não rodou".into(),
        });
        lines.join("\n")
    }

    fn text(&self, src: &str, id: ElementId, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let (text, spans) = inline(src);
        let exists: Vec<bool> = spans
            .iter()
            .map(|(_, s)| match s {
                Span::Link(n) => vault::find(&self.notes, n).is_some(),
                _ => true,
            })
            .collect();
        let hl: Vec<(Range<usize>, HighlightStyle)> = spans
            .iter()
            .zip(&exists)
            .map(|((r, s), ok)| {
                let style = match s {
                    Span::Code => HighlightStyle {
                        background_color: Some(t.panel),
                        ..Default::default()
                    },
                    Span::Bold => HighlightStyle {
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                    Span::Link(_) => {
                        let c = if *ok { t.accent } else { t.orange };
                        HighlightStyle {
                            color: Some(c),
                            underline: Some(UnderlineStyle {
                                thickness: px(1.),
                                color: Some(c),
                                wavy: false,
                            }),
                            ..Default::default()
                        }
                    }
                };
                (r.clone(), style)
            })
            .collect();
        let mono: Vec<(Range<usize>, SharedString)> = spans
            .iter()
            .filter(|(_, s)| *s == Span::Code)
            .map(|(r, _)| (r.clone(), SharedString::from(t.mono.clone())))
            .collect();
        let links: Vec<(Range<usize>, String)> = spans
            .into_iter()
            .filter_map(|(r, s)| match s {
                Span::Link(n) => Some((r, n)),
                _ => None,
            })
            .collect();
        let ranges: Vec<Range<usize>> = links.iter().map(|l| l.0.clone()).collect();
        let names: Vec<String> = links.into_iter().map(|l| l.1).collect();
        let this = cx.entity().downgrade();
        InteractiveText::new(
            id,
            StyledText::new(text)
                .with_highlights(hl)
                .with_font_family_overrides(mono),
        )
        .on_click(ranges, move |i, _, cx| {
            let name = names[i].clone();
            let _ = this.update(cx, |this, cx| this.open_link(&name, cx));
        })
        .into_any_element()
    }

    /// O `vmd()` do protótipo: frontmatter numa linha, títulos, listas e parágrafos.
    fn markdown(&self, key: &str, src: &str, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let mut out = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(px(14.))
            .line_height(px(22.));
        let mut body = src;
        if let Some(rest) = src.strip_prefix("---\n")
            && let Some(end) = rest.find("\n---\n")
        {
            out = out.child(
                div()
                    .font_family(t.mono.clone())
                    .text_size(px(12.5))
                    .text_color(t.comment)
                    .child(rest[..end].replace('\n', " · ")),
            );
            body = &rest[end + 5..];
        }
        let mut code: Option<String> = None;
        for (i, l) in body.lines().enumerate() {
            let id: ElementId = (SharedString::from(key.to_string()), i).into();
            if l.trim_start().starts_with("```") {
                match code.take() {
                    Some(c) => out = out.child(self.code_block(c)),
                    None => code = Some(String::new()),
                }
                continue;
            }
            if let Some(c) = &mut code {
                c.push_str(l.strip_prefix("   ").unwrap_or(l));
                c.push('\n');
                continue;
            }
            let indent = ((l.len() - l.trim_start().len()) / 2).min(3) as f32 * 18.;
            if let Some(h) = l.strip_prefix("# ") {
                out = out.child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .mb(px(4.))
                        .child(self.text(h, id, cx)),
                );
            } else if let Some(h) = l.strip_prefix("## ") {
                out = out.child(
                    div()
                        .mt(px(10.))
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .font_weight(FontWeight::MEDIUM)
                        .child(h.to_uppercase()),
                );
            } else if let Some(h) = l.strip_prefix("### ") {
                out = out.child(
                    div()
                        .mt(px(6.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.text(h, id, cx)),
                );
            } else if let Some((mark, item)) = list_item(l.trim_start()) {
                out = out.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .pl(px(6. + indent))
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(12.))
                                .text_color(t.muted)
                                .font_family(t.mono.clone())
                                .child(mark),
                        )
                        .child(div().flex_1().min_w_0().child(self.text(item, id, cx))),
                );
            } else if let Some(q) = l.trim_start().strip_prefix('>') {
                out = out.child(
                    div()
                        .my(px(4.))
                        .pl(px(10.))
                        .border_l_2()
                        .border_color(t.comment)
                        .text_color(t.muted)
                        .child(self.text(q.trim_start(), id, cx)),
                );
            } else if !l.trim().is_empty() {
                out = out.child(div().pl(px(indent)).child(self.text(l.trim(), id, cx)));
            }
        }
        if let Some(c) = code {
            out = out.child(self.code_block(c));
        }
        out
    }

    fn code_block(&self, code: String) -> Div {
        let t = &self.theme;
        div()
            .ml(px(24.))
            .mb(px(4.))
            .px(px(10.))
            .py(px(8.))
            .rounded(px(5.))
            .bg(t.panel)
            .font_family(t.mono.clone())
            .text_size(px(12.5))
            .line_height(px(19.))
            .child(code.trim_end().to_string())
    }

    fn card(&self) -> Div {
        let t = &self.theme;
        div()
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .bg(t.raise)
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(6.))
    }

    fn h4(&self, title: &str, n: Option<usize>) -> Div {
        let t = &self.theme;
        div()
            .flex()
            .gap(px(8.))
            .items_center()
            .text_size(px(13.))
            .text_color(t.muted)
            .font_weight(FontWeight::MEDIUM)
            .child(title.to_uppercase())
            .when_some(n, |d, n| {
                d.child(div().font_family(t.mono.clone()).child(n.to_string()))
            })
    }

    fn link_list(&self, names: Vec<String>, id: &str, cx: &mut Context<Self>) -> AnyElement {
        if names.is_empty() {
            return div().into_any_element();
        }
        let src = names
            .iter()
            .map(|n| format!("[[{n}]]"))
            .collect::<Vec<_>>()
            .join(" · ");
        self.text(&src, SharedString::from(id.to_string()).into(), cx)
    }

    fn left(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let q = self.query.read(cx).value.to_lowercase();
        let today = Self::today_key();
        let pend = self.notes.iter().find(|n| n.key == today).map_or(0, |n| {
            n.text
                .lines()
                .filter(|l| l.trim_start().starts_with("- [ ]"))
                .count()
        });
        let feat = |id: &'static str, g: &str, gc: Hsla, b: &str, k: String, on: bool| {
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .gap(px(8.))
                .px_2()
                .py(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|d| d.bg(t.raise))
                .when(on, |d| d.bg(t.accent_soft))
                .child(div().w(px(16.)).text_color(gc).child(g.to_string()))
                .child(
                    div()
                        .flex_1()
                        .font_family(t.mono.clone())
                        .text_size(px(13.5))
                        .child(b.to_string()),
                )
                .child(div().text_size(px(12.)).text_color(t.muted).child(k))
        };
        let log_n = vault::activity().len();
        let mut col = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap(px(6.))
                    .items_baseline()
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(6.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("vault"))
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(13.))
                            .text_color(t.muted)
                            .truncate()
                            .child(crate::sidebar::tilde(&vault::dir().to_string_lossy())),
                    ),
            )
            .child(div().px(px(10.)).pb(px(8.)).child(self.query.clone()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(6.))
                    .child(
                        feat(
                            "v-graph",
                            "◎",
                            t.comment,
                            "Grafo",
                            format!("{} notas", self.notes.len()),
                            self.view == View::Graph,
                        )
                        .child(tour::anchor(&self.anchors, "vgraph"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::Graph;
                            this.refresh_graph(cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        feat(
                            "v-today",
                            "☐",
                            rgb(0xf1fa8c).into(),
                            "Hoje",
                            format!("{pend} pendente{}", if pend == 1 { "" } else { "s" }),
                            self.view == View::Note(today.clone()),
                        )
                        .child(tour::anchor(&self.anchors, "vtoday"))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open(Self::today_key(), cx)),
                        ),
                    )
                    .child(
                        feat(
                            "v-log",
                            "↺",
                            t.comment,
                            "Atividade",
                            if log_n > 0 {
                                log_n.to_string()
                            } else {
                                String::new()
                            },
                            self.view == View::Log,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::Log;
                            cx.notify();
                        })),
                    )
                    .child(
                        feat(
                            "v-new",
                            "+",
                            t.comment,
                            "Nova nota",
                            "notas/".into(),
                            self.view == View::Note(NEW.into()),
                        )
                        .on_click(cx.listener(|this, _, w, cx| this.new_note(w, cx))),
                    ),
            );
        let notes: Vec<&Note> = self
            .notes
            .iter()
            .filter(|n| q.is_empty() || n.name.to_lowercase().contains(&q))
            .collect();
        for (dir, label) in [
            ("notas", "Notas"),
            ("repos", "Repos"),
            ("decisions", "Decisões"),
            ("daily", "Daily"),
            (".", "Raiz"),
        ] {
            let mut xs: Vec<&&Note> = notes.iter().filter(|n| n.dir == dir).collect();
            if xs.is_empty() {
                continue;
            }
            if dir == "daily" {
                xs.sort_by(|a, b| b.name.cmp(&a.name));
            }
            col = col.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(label.to_uppercase())
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .child(xs.len().to_string()),
                    ),
            );
            for n in xs {
                let bl = vault::backlinks(&self.notes, &n.name).len();
                let key = n.key.clone();
                let on = self.view == View::Note(n.key.clone());
                col = col.child(
                    div()
                        .id(SharedString::from(format!("vk:{}", n.key)))
                        .flex()
                        .gap(px(6.))
                        .px(px(12.))
                        .py(px(3.))
                        .cursor_pointer()
                        .hover(|d| d.bg(t.raise))
                        .when(on, |d| d.bg(t.accent_soft))
                        .on_click(cx.listener(move |this, _, _, cx| this.open(key.clone(), cx)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(t.mono.clone())
                                .text_size(px(13.))
                                .child(n.name.clone()),
                        )
                        .when(bl > 0, |d| {
                            d.child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child(format!("←{bl}")),
                            )
                        }),
                );
            }
        }
        if notes.is_empty() {
            col = col.child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child("Nenhuma nota com esse nome."),
            );
        }
        div()
            .id("vault-left")
            .w(px(self.left_w))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .child(col)
    }

    fn daily(&self, note: Option<&Note>, key: &str, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let Some(note) = note else {
            let key = key.to_string();
            return self.card().child(self.h4("Fila", None)).child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div()
                            .text_color(t.muted)
                            .child("Ainda não há daily de hoje."),
                    )
                    .child(
                        button(&t, "v-mkdaily", "Criar daily de hoje", true).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let tpl = std::fs::read_to_string(
                                    vault::dir().join("templates/daily.md"),
                                )
                                .unwrap_or_else(|_| {
                                    "# {{date:YYYY-MM-DD}}\n\n## Fila\n- [ ] \n\n## Notas\n".into()
                                })
                                .replace("{{date:YYYY-MM-DD}}", &vault::today());
                                this.write(&key, tpl, format!("{key} criada do template"), cx);
                                this.notes = vault::load(&vault::dir());
                            },
                        )),
                    ),
            );
        };
        let mut card = self
            .card()
            .child(self.h4("Fila", None).child(div().flex_1()).child(
                button(&t, "v-sum", "Gerar resumo do dia", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.summary = Some(this.summary_text(cx));
                        cx.notify();
                    },
                )),
            ));
        for (i, l) in note.text.lines().enumerate() {
            let s = l.trim_start();
            let (done, task) = if let Some(x) = s.strip_prefix("- [ ]") {
                (false, x)
            } else if let Some(x) = s.strip_prefix("- [x]") {
                (true, x)
            } else {
                continue;
            };
            let (key, task_s) = (note.key.clone(), task.trim().to_string());
            card = card.child(
                div()
                    .id(("dt", i))
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(n) = this.notes.iter().find(|n| n.key == key) else {
                            return;
                        };
                        let mut lines: Vec<String> = n.text.lines().map(String::from).collect();
                        lines[i] = if done {
                            lines[i].replacen("- [x]", "- [ ]", 1)
                        } else {
                            lines[i].replacen("- [ ]", "- [x]", 1)
                        };
                        let mark = if done { "[ ]" } else { "[x]" };
                        let desc = format!("{key} · {mark} {task_s}");
                        this.write(&key.clone(), lines.join("\n") + "\n", desc, cx);
                    }))
                    .child(checkbox(&t, done))
                    .child(
                        div()
                            .when(done, |d| d.line_through().text_color(t.muted))
                            .child(task.trim().to_string()),
                    ),
            );
        }
        if let Some(sum) = self.summary.clone() {
            let yellow: Hsla = rgb(0xf1fa8c).into();
            let key = note.key.clone();
            card = card.child(
                div()
                    .mt(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_dashed()
                    .border_color(yellow)
                    .bg(Hsla { a: 0.07, ..yellow })
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
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
                                    .text_color(yellow)
                                    .child("PRÉVIA"),
                            )
                            .child("· vai para a seção “Resumo” da daily"),
                    )
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_size(px(13.5))
                                    .child(sum.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .child(
                                        button(&t, "v-sum-ok", "Salvar na daily", true).on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                let text = this
                                                    .notes
                                                    .iter()
                                                    .find(|n| n.key == key)
                                                    .map(|n| n.text.clone())
                                                    .unwrap_or_default();
                                                let text = format!(
                                                    "{}\n## Resumo\n{sum}\n",
                                                    text.trim_end()
                                                );
                                                this.summary = None;
                                                this.write(
                                                    &key,
                                                    text,
                                                    format!("{key} · Resumo do dia salvo"),
                                                    cx,
                                                );
                                            }),
                                        ),
                                    )
                                    .child(button(&t, "v-sum-no", "Descartar", false).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.summary = None;
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    ),
            );
        }
        card
    }

    fn main(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme.clone();
        let wrap = |d: Div| {
            div()
                .id("vault-main")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .px(px(18.))
                        .py(px(14.))
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(d),
                )
        };
        match &self.view {
            View::Graph => div()
                .flex_1()
                .min_h_0()
                .p(px(12.))
                .flex()
                .child(self.graph.clone())
                .into_any_element(),
            View::Log => {
                let items = vault::activity();
                let mut card = self.card().child(self.h4("Atividade do vault", None));
                if items.is_empty() {
                    card = card.child(div().text_color(t.muted).child("Nada ainda."));
                }
                for (when, what) in items.into_iter().take(200) {
                    card = card.child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .items_center()
                            .text_size(px(13.))
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_size(px(12.))
                                    .text_color(t.comment)
                                    .child(when),
                            )
                            .child(what),
                    );
                }
                card = card.child(div().text_color(t.muted).child(
                    "O Maestro escreve sozinho só o que o contrato do vault/CLAUDE.md permite: MR aberto ou mergeado → Histórico; tarefa da daily concluída → [x]; o resumo do dia só com o seu clique. Cada escrita aparece aqui e no toast, com desfazer.",
                ));
                wrap(div().flex().flex_col().gap(px(12.)).child(card)).into_any_element()
            }
            View::Note(key) => {
                let key = key.clone();
                let note = self.notes.iter().find(|n| n.key == key);
                let name = self.current_name();
                let ol = note.map(|n| n.links()).unwrap_or_default();
                let bl: Vec<String> = vault::backlinks(&self.notes, &name)
                    .iter()
                    .map(|n| n.name.clone())
                    .collect();
                let is_daily = key == Self::today_key();
                let mut body = div().flex().flex_col().gap(px(12.));
                body = body.child(if is_daily {
                    self.daily(note, &key, cx)
                } else if let Some(e) = self.editing.clone() {
                    self.card().child(
                        div()
                            .h(px(420.))
                            .bg(t.win)
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(5.))
                            .child(
                                Editor::new(&e)
                                    .appearance(false)
                                    .bordered(false)
                                    .h_full()
                                    .font_family(t.mono.clone())
                                    .text_size(px(13.5)),
                            ),
                    )
                } else {
                    match note {
                        Some(n) => self
                            .card()
                            .relative()
                            .child(tour::anchor(&self.anchors, "vcard"))
                            .child(self.markdown(&key, &n.text, cx)),
                        None => self.card().child(
                            div()
                                .text_color(t.muted)
                                .child(format!("~/vault/{key} não existe.")),
                        ),
                    }
                });
                if key.starts_with("repos/") {
                    let feats: Vec<FeatureRef> = self
                        .features(cx)
                        .into_iter()
                        .filter(|f| f.repo == name)
                        .collect();
                    if !feats.is_empty() {
                        let mut row = div().flex().flex_wrap().gap(px(6.)).items_center().child(
                            div().text_color(t.muted).child(format!("Toda feature de {name} abre o Claude com esta nota e o que ela linka. Ativas agora:")),
                        );
                        for (i, f) in feats.into_iter().enumerate() {
                            let pane = f.pane.clone();
                            row = row.child(
                                button(&t, ("vf", i), &format!("⎇ {}", f.branch), false).on_click(
                                    cx.listener(move |_, _, _, cx| {
                                        if let Some(p) = &pane {
                                            cx.emit(Event::Focus(p.clone()));
                                        }
                                    }),
                                ),
                            );
                        }
                        body = body.child(
                            self.card()
                                .child(self.h4("No contexto das features", None))
                                .child(row),
                        );
                    }
                }
                let ol_n = ol.len();
                let bl_n = bl.len();
                body = body
                    .child(
                        self.card()
                            .child(self.h4("Links desta nota", Some(ol_n)))
                            .child(if ol_n == 0 {
                                div()
                                    .text_color(t.muted)
                                    .child("Nenhum.")
                                    .into_any_element()
                            } else {
                                self.link_list(ol, "v-ol", cx)
                            }),
                    )
                    .child(
                        self.card()
                            .child(self.h4("Mencionado em", Some(bl_n)))
                            .child(if bl_n == 0 {
                                div()
                                    .text_color(t.muted)
                                    .child("Nenhuma nota aponta para esta.")
                                    .into_any_element()
                            } else {
                                self.link_list(bl, "v-bl", cx)
                            }),
                    );
                let editable = (note.is_some() || key == NEW) && !is_daily;
                let editing = self.editing.is_some();
                let busy = self.formatting.as_deref() == Some(key.as_str());
                let status = if busy {
                    "Claude formatando… (Haiku, sem ferramentas)"
                } else if key == NEW {
                    "título e Enter cria o arquivo"
                } else if !editing {
                    ""
                } else if self
                    .editing
                    .as_ref()
                    .is_some_and(|e| e.read(cx).value() != self.saved.as_str())
                {
                    "salvando…"
                } else {
                    "salvo"
                };
                let seg = |id: &'static str, label: &'static str, on: bool| {
                    div()
                        .id(id)
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(if on { t.fg } else { t.muted })
                        .when(on, |d| d.bg(t.accent_soft))
                        .child(label)
                };
                let k3 = key.clone();
                let n2 = name.clone();
                let header = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(18.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .items_baseline()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if key == NEW {
                                        "nova nota".to_string()
                                    } else {
                                        name.clone()
                                    }),
                            )
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_size(px(13.))
                                    .text_color(t.muted)
                                    .child(format!("~/vault/{key}")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .when(editable, |d| {
                                d.child(seg("v-ed", "Editar", editing).on_click(cx.listener(
                                    move |this, _, w, cx| {
                                        if this.editing.is_none() {
                                            this.start_edit(&k3, w, cx);
                                        }
                                    },
                                )))
                                .child(seg("v-ver", "Ver", !editing).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.end_edit(cx);
                                        cx.notify();
                                    },
                                )))
                                .child(div().text_size(px(12.)).text_color(t.muted).child(status))
                            })
                            .child(div().flex_1())
                            .when(key != NEW, |d| {
                                d.child(button(&t, "v-g", "◎ Ver no grafo", false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.view = View::Graph;
                                        let n = n2.clone();
                                        this.graph.update(cx, |g, cx| g.highlight(n, cx));
                                        this.refresh_graph(cx);
                                        cx.notify();
                                    }),
                                ))
                            })
                            .when(editable, |d| {
                                d.child(
                                    button(
                                        &t,
                                        "v-fmt",
                                        if busy {
                                            "Formatando…"
                                        } else {
                                            "✦ Formatar com IA"
                                        },
                                        true,
                                    )
                                    .when(busy, |d| d.opacity(0.45))
                                    .on_click(cx.listener(|this, _, _, cx| this.format(cx))),
                                )
                            }),
                    );
                let _ = window;
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(wrap(body))
                    .into_any_element()
            }
        }
    }
}

impl Render for VaultView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .bg(t.win)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<LeftDrag>, _, cx| {
                let (b, x) = (e.bounds, e.event.position.x);
                let max = (f32::from(b.size.width) - 300.).max(180.);
                this.left_w = f32::from(x - b.left()).clamp(180., max);
                cx.notify();
            }))
            .child(self.left(cx))
            .child(theme::resize_handle("rz-vault", LeftDrag, &t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.main(window, cx)),
            )
    }
}

/// `- x` → ("•", "x"); `12. x` → ("12.", "x").
fn list_item(l: &str) -> Option<(String, &str)> {
    if let Some(r) = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")) {
        return Some(("•".into(), r));
    }
    let n = l.find(". ")?;
    l[..n]
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| (format!("{}.", &l[..n]), &l[n + 2..]))
        .filter(|_| n > 0)
}

/// Para o grafo: as features que existem, com a nota do repo de cada uma.
pub fn features_from(
    inventories: &[crate::feature::Inventory],
    agents: &[crate::herdr::Agent],
    mrs: &HashMap<std::path::PathBuf, crate::mr::Mr>,
    usage: &HashMap<String, crate::session::Usage>,
) -> Vec<FeatureRef> {
    inventories
        .iter()
        .map(|i| {
            let wt = &i.worktrees[0].path;
            let agent = agents
                .iter()
                .find(|a| std::path::Path::new(&a.cwd).starts_with(wt));
            FeatureRef {
                status: agent
                    .map_or("sem Claude", |a| crate::sidebar::glyph(a.agent_status).1)
                    .into(),
                merged: mrs.get(wt).filter(|m| m.merged).map(|m| m.iid),
                cost: agent
                    .and_then(|a| usage.get(&a.pane_id))
                    .map_or(0., |u| u.cost),
                branch: i.branch.clone(),
                repo: i
                    .repo
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                pane: agents
                    .iter()
                    .find(|a| std::path::Path::new(&a.cwd).starts_with(wt))
                    .map(|a| a.pane_id.clone()),
                mr: mrs.get(wt).filter(|m| !m.merged).map(|m| m.iid),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Span, inline, list_item};

    #[test]
    fn item_de_lista() {
        assert_eq!(list_item("- a"), Some(("•".into(), "a")));
        assert_eq!(list_item("12. passo"), Some(("12.".into(), "passo")));
        assert_eq!(list_item("v1. não"), None);
        assert_eq!(list_item(". nada"), None);
        assert_eq!(list_item("texto"), None);
    }

    #[test]
    fn inline_do_vmd() {
        let (t, s) = inline("veja `cfg` e **isto** em [[bots-api|o repo]] ok");
        assert_eq!(t, "veja cfg e isto em [[bots-api|o repo]] ok");
        assert_eq!(s[0], (5..8, Span::Code));
        assert_eq!(s[1], (11..15, Span::Bold));
        assert_eq!(s[2].1, Span::Link("bots-api".into()));
        assert_eq!(&t[s[2].0.clone()], "[[bots-api|o repo]]");
        assert_eq!(inline("sem fim `x").0, "sem fim `x");
    }
}
