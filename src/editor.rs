use std::{
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use gpui_kit::{
    base::input::{
        Diagnostic, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope, RopeExt,
        TextDecoration, TextDecorationCollection,
    },
    component::input::{Editor, EditorState, InputEvent},
    prelude::FluentBuilder,
    *,
};
use lsp_types::DiagnosticSeverity;

use crate::{
    blame::{self, Blame},
    diff::{self, Tok},
    form::{FieldEvent, TextField, button},
    lsp::{self, Server},
    lsp_editor::{Doc, Provider},
    mrs::{self, Row},
    theme::Theme,
};

actions!(editor, [LeaveEditor]);

pub enum Event {
    Changed,
    Toast(String),
    /// "Ver diff" do banner de conflito
    ShowDiff(String),
    /// ir para a definição em outro arquivo: (caminho absoluto, linha 0-based)
    Open(PathBuf, u32),
    /// "Abrir MR" do blame
    OpenMr(Row),
    /// "Histórico do arquivo" do blame
    History,
    /// Ctrl+W
    CloseTab,
    /// Ctrl+Tab (true) e Ctrl+Shift+Tab (false)
    CycleTab(bool),
}

/// O servidor de linguagem deste arquivo.
pub enum Lsp {
    Off,
    Missing(&'static Server),
    Starting(&'static Server),
    Ready(Arc<Doc>),
    Failed(&'static Server, String),
}

/// O blame da linha do cursor e o MR de onde ela veio (lido só quando o popover abre).
struct BlameLine {
    line: usize,
    blame: Option<Blame>,
    mr: Option<Option<Row>>,
}

/// `ctbz/experts/bots-api` do `origin` no GitLab.
fn gitlab_path(root: &Path) -> Option<String> {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let path = url
        .split_once("gitlab.com:")
        .or_else(|| url.split_once("gitlab.com/"))?
        .1;
    Some(path.trim_end_matches(".git").to_string())
}

/// O editor do `openFile` do protótipo: o arquivo da worktree, salvo com Ctrl+S.
pub struct FileEditor {
    pub path: String,
    file: PathBuf,
    state: Entity<EditorState>,
    /// o que está no disco, pela última leitura ou gravação nossa
    saved: String,
    /// o disco mudou por fora enquanto havia edição sua
    conflict: Option<String>,
    root: PathBuf,
    pub lsp: Lsp,
    /// × na faixa de "sem servidor"
    lsp_closed: bool,
    diag_gen: u64,
    squiggle: Option<TextDecorationCollection>,
    /// (erros, avisos) do arquivo, para a barra de status
    pub problems: (usize, usize),
    blame: Option<BlameLine>,
    blame_pop: bool,
    /// mouse em cima de (blame, popover)
    blame_hover: (bool, bool),
    _blame_task: Task<()>,
    /// onde o editor está na janela, medido no último frame (o blame se posiciona por ele)
    area: Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Ctrl+G: o campo "Ir para a linha"
    goto_field: Option<Entity<TextField>>,
    theme: Theme,
    _poll: Task<()>,
}

impl EventEmitter<Event> for FileEditor {}

#[derive(Debug, PartialEq)]
enum Sync {
    Same,
    Reload,
    Conflict,
}

fn sync(saved: &str, buffer: &str, disk: &str) -> Sync {
    if disk == saved || disk == buffer {
        Sync::Same
    } else if buffer == saved {
        Sync::Reload
    } else {
        Sync::Conflict
    }
}

/// `@arquivo:a-b` da seleção, como o `sendSel` do protótipo.
pub fn line_ref(path: &str, text: &str, sel: Range<usize>) -> String {
    let line = |o: usize| text[..o.min(text.len())].matches('\n').count() + 1;
    let (a, b) = (line(sel.start), line(sel.end));
    if a == b {
        format!("@{path}:{a}")
    } else {
        format!("@{path}:{a}-{b}")
    }
}

/// As linhas inteiras que a seleção toca, sem o `\n` final; seleção que termina no começo de linha não leva ela.
fn line_block(text: &str, sel: Range<usize>) -> Range<usize> {
    let end = if sel.end > sel.start && text[..sel.end].ends_with('\n') {
        sel.end - 1
    } else {
        sel.end
    };
    let a = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
    let b = text[end..].find('\n').map_or(text.len(), |i| end + i);
    a..b.max(a)
}

fn comment_prefix(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    Some(match ext {
        "py" | "sh" | "bash" | "zsh" | "yml" | "yaml" | "toml" | "rb" | "tf" | "conf" | "env" => {
            "#"
        }
        "sql" | "lua" | "hs" => "--",
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "go" | "java" | "kt" | "c" | "h"
        | "cpp" | "cs" | "swift" | "scss" | "dart" | "php" | "proto" | "groovy" => "//",
        _ => return None,
    })
}

/// Ctrl+/: comenta as linhas (no recuo da menos recuada) ou descomenta se todas já estão.
fn toggle_comment(block: &str, prefix: &str) -> String {
    let lines: Vec<&str> = block.split('\n').collect();
    let code = || lines.iter().filter(|l| !l.trim().is_empty());
    let all = code().all(|l| l.trim_start().starts_with(prefix));
    let indent = code()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                l.to_string()
            } else if all {
                let i = l.len() - l.trim_start().len();
                let rest = &l[i + prefix.len()..];
                format!("{}{}", &l[..i], rest.strip_prefix(' ').unwrap_or(rest))
            } else {
                format!("{}{prefix} {}", &l[..indent], &l[indent..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Alt+↑/↓: troca o bloco com a linha vizinha. Devolve (trecho trocado, texto novo, quanto o bloco andou).
fn move_lines(text: &str, block: Range<usize>, up: bool) -> Option<(Range<usize>, String, isize)> {
    let body = &text[block.clone()];
    if up {
        let prev_start = text[..block.start.checked_sub(1)?]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let prev = &text[prev_start..block.start - 1];
        Some((
            prev_start..block.end,
            format!("{body}\n{prev}"),
            -(prev.len() as isize + 1),
        ))
    } else {
        if block.end >= text.len() {
            return None;
        }
        let next_end = text[block.end + 1..]
            .find('\n')
            .map_or(text.len(), |i| block.end + 1 + i);
        let next = &text[block.end + 1..next_end];
        Some((
            block.start..next_end,
            format!("{next}\n{body}"),
            next.len() as isize + 1,
        ))
    }
}

pub fn style(t: &Theme, tok: Tok) -> HighlightStyle {
    HighlightStyle {
        color: Some(match tok {
            Tok::Comment => t.comment,
            Tok::Str => rgb(0xf1fa8c).into(),
            Tok::Keyword => rgb(0xff79c6).into(),
            Tok::Number => t.accent,
        }),
        font_style: (tok == Tok::Comment).then_some(FontStyle::Italic),
        ..Default::default()
    }
}

/// O `hl()` do protótipo dentro do editor do gpui-component.
// ponytail: realce por linha sem parser, relendo o arquivo do início a cada pintura (O(arquivo)); tree-sitter do gpui-component quando um arquivo grande pesar
struct Highlighter {
    text: String,
    theme: Theme,
}

impl InputHighlighter for Highlighter {
    fn language(&self) -> SharedString {
        "maestro".into()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.text = text.to_string();
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut out = Vec::new();
        let mut at = range.start;
        let mut start = 0;
        for line in self.text.split_inclusive('\n') {
            let end = start + line.len();
            if end > range.start && start < range.end {
                for (r, tok) in diff::highlight(line.trim_end_matches('\n')) {
                    let r = (r.start + start).max(range.start)..(r.end + start).min(range.end);
                    if r.start >= r.end {
                        continue;
                    }
                    if r.start > at {
                        out.push((at..r.start, HighlightStyle::default()));
                    }
                    at = r.end;
                    out.push((r, style(&self.theme, tok)));
                }
            }
            if end >= range.end {
                break;
            }
            start = end;
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

impl FileEditor {
    pub fn new(
        root: PathBuf,
        path: String,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file = root.join(&path);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let t = theme.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .line_number(true)
                .indent_guides(false)
                .default_value(text.clone());
            s.set_highlighter_factory(
                Rc::new(move |_| {
                    Some(Box::new(Highlighter {
                        text: String::new(),
                        theme: t.clone(),
                    }) as Box<dyn InputHighlighter>)
                }),
                cx,
            );
            s
        });
        cx.subscribe(&state, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                if let Lsp::Ready(doc) = &this.lsp {
                    doc.sync(&this.buffer(cx));
                }
                // o gpui-base limpa os diagnósticos a cada edição
                this.apply_diagnostics(cx);
                cx.emit(Event::Changed);
            }
        })
        .detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // ponytail: lê o arquivo a cada 1 s; `notify` quando houver muitos editores abertos
        let poll = cx.spawn_in(window, {
            let file = file.clone();
            async move |this, cx| {
                let mut tick = 0u32;
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    tick += 1;
                    let alive = this.update(cx, |this, cx| {
                        if let Lsp::Ready(doc) = &this.lsp
                            && doc.client.generation.load(Ordering::Relaxed) != this.diag_gen
                        {
                            this.apply_diagnostics(cx);
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                    if !tick.is_multiple_of(4) {
                        continue;
                    }
                    let f = file.clone();
                    let disk = cx
                        .background_executor()
                        .spawn(async move { std::fs::read_to_string(f).ok() })
                        .await;
                    let ok = this.update_in(cx, |this, window, cx| {
                        if let Some(disk) = disk {
                            this.on_disk(disk, window, cx);
                        }
                    });
                    if ok.is_err() {
                        break;
                    }
                }
            }
        });
        let focus = state.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let lsp = match lsp::server_for(&path) {
            None => Lsp::Off,
            Some(s) if !lsp::installed(&root, s) => Lsp::Missing(s),
            Some(s) => {
                let r = root.clone();
                cx.spawn(async move |this, cx| {
                    let res = cx
                        .background_executor()
                        .spawn(async move { lsp::client(&r, s) })
                        .await;
                    this.update(cx, |this, cx| this.lsp_ready(s, res, cx)).ok();
                })
                .detach();
                Lsp::Starting(s)
            }
        };
        Self {
            path,
            file,
            state,
            saved: text,
            conflict: None,
            root,
            lsp,
            lsp_closed: false,
            diag_gen: 0,
            squiggle: None,
            problems: (0, 0),
            blame: None,
            blame_pop: false,
            blame_hover: (false, false),
            _blame_task: Task::ready(()),
            area: Rc::default(),
            goto_field: None,
            theme,
            _poll: poll,
        }
    }

    fn lsp_ready(
        &mut self,
        server: &'static Server,
        res: Result<Arc<lsp::Client>, String>,
        cx: &mut Context<Self>,
    ) {
        let client = match res {
            Ok(c) => c,
            Err(e) => {
                self.lsp = Lsp::Failed(server, e);
                return cx.notify();
            }
        };
        let doc = Doc::open(client, self.file.clone(), self.buffer(cx));
        let weak = cx.entity().downgrade();
        let me = self.file.clone();
        self.state.update(cx, |s, _| {
            let lsp = s.lsp_mut();
            let p = Rc::new(Provider(doc.clone()));
            lsp.completion_provider = Some(p.clone());
            lsp.hover_provider = Some(p.clone());
            lsp.definition_provider = Some(p.clone());
            lsp.code_action_providers = vec![p];
            lsp.show_document = Some(Rc::new(move |params, _, cx| {
                let Some(path) = lsp::uri_path(params.uri.as_str()) else {
                    return false;
                };
                if path == me {
                    return false;
                }
                let line = params.selection.map_or(0, |r| r.start.line);
                weak.update(cx, |_, cx| cx.emit(Event::Open(path, line)))
                    .is_ok()
            }));
        });
        self.lsp = Lsp::Ready(doc);
        cx.notify();
    }

    fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
        let Lsp::Ready(doc) = &self.lsp else { return };
        self.diag_gen = doc.client.generation.load(Ordering::Relaxed);
        // dica (variável não usada etc.) não sublinha nem conta
        let list: Vec<_> = doc
            .diagnostics()
            .into_iter()
            .filter(|d| d.severity != Some(DiagnosticSeverity::HINT))
            .collect();
        let errors = list
            .iter()
            .filter(|d| d.severity.is_none_or(|s| s == DiagnosticSeverity::ERROR))
            .count();
        let warnings = list
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::WARNING))
            .count();
        if self.problems != (errors, warnings) {
            self.problems = (errors, warnings);
            cx.emit(Event::Changed);
        }
        // ponytail: o realce do gpui-component vem sem tree-sitter e pinta o diagnóstico transparente;
        // o ondulado sai como decoração, e o DiagnosticSet fica só para a mensagem do hover
        let text = Rope::from(self.buffer(cx).as_str());
        let squiggles: Vec<_> = list
            .iter()
            .map(|d| {
                let start = text.position_to_offset(&d.range.start);
                let end = text.position_to_offset(&d.range.end).max(start + 1);
                let color: Hsla = match d.severity {
                    Some(DiagnosticSeverity::WARNING) => rgb(0xf1fa8c).into(),
                    Some(DiagnosticSeverity::INFORMATION) => self.theme.cyan,
                    _ => rgb(0xff5555).into(),
                };
                TextDecoration::new(
                    start..end.min(text.len()),
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            color: Some(color),
                            thickness: px(1.),
                            wavy: true,
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let squiggle = self.squiggle.get_or_insert_with(|| {
            self.state
                .update(cx, |s, cx| s.create_decorations_collection(Vec::new(), cx))
        });
        squiggle.set(squiggles, cx);
        self.state.update(cx, |s, cx| {
            if let Some(set) = s.diagnostics_mut() {
                set.clear();
                set.extend(list.into_iter().map(Diagnostic::from));
            }
            cx.notify();
        });
    }

    /// Troca `range` por `text` numa edição só do desfazer e deixa `sel` selecionado.
    fn edit(
        &mut self,
        range: Range<usize>,
        text: String,
        sel: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.set_selected_range(range, cx);
            s.replace(text, window, cx);
            s.set_selected_range(sel, cx);
        });
        if let Lsp::Ready(doc) = &self.lsp {
            doc.sync(&self.buffer(cx));
        }
        self.apply_diagnostics(cx);
        cx.emit(Event::Changed);
        cx.notify();
    }

    fn toggle_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prefix) = comment_prefix(&self.path) else {
            return cx.emit(Event::Toast(format!("Não sei comentar {}", self.path)));
        };
        let (text, sel) = {
            let s = self.state.read(cx);
            (s.value().to_string(), s.selected_range())
        };
        let block = line_block(&text, sel.clone());
        let new = toggle_comment(&text[block.clone()], prefix);
        let delta = new.len() as isize - block.len() as isize;
        let end = (sel.end as isize + delta).max(block.start as isize) as usize;
        let start = if sel.is_empty() { end } else { block.start };
        self.edit(block, new, start..end, window, cx);
    }

    fn move_lines(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (text, sel) = {
            let s = self.state.read(cx);
            (s.value().to_string(), s.selected_range())
        };
        let Some((range, new, by)) = move_lines(&text, line_block(&text, sel.clone()), up) else {
            return;
        };
        let shift = |o: usize| (o as isize + by) as usize;
        self.edit(range, new, shift(sel.start)..shift(sel.end), window, cx);
    }

    fn open_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.theme.clone();
        let f = cx.new(|cx| TextField::new(t, "", "Ir para a linha", false, true, cx));
        cx.subscribe_in(&f, window, |this, f, e: &FieldEvent, window, cx| {
            match e {
                FieldEvent::Submit => {
                    let n: u32 = f.read(cx).value.trim().parse().unwrap_or(0);
                    if n > 0 {
                        let last = this.buffer(cx).matches('\n').count() as u32;
                        this.goto((n - 1).min(last), window, cx);
                    }
                }
                FieldEvent::Cancel => {}
                FieldEvent::Changed => return,
            }
            this.goto_field = None;
            let f = this.focus_handle(cx);
            window.focus(&f, cx);
            cx.notify();
        })
        .detach();
        window.focus(&f.read(cx).focus_handle(), cx);
        self.goto_field = Some(f);
        cx.notify();
    }

    /// Vai para a linha (0-based), como no "ir para a definição".
    pub fn goto(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.set_cursor_position(lsp_types::Position::new(line, 0), window, cx)
        });
    }

    /// "TS · typescript-language-server", para a barra de status.
    pub fn lsp_label(&self) -> Option<String> {
        let short = |s: &Server| match s.id {
            "typescript" => "TS",
            _ => s.lang,
        };
        match &self.lsp {
            Lsp::Ready(d) => Some(format!("{} · {}", short(d.client.server), d.client.bin)),
            Lsp::Starting(s) => Some(format!("{}: iniciando…", short(s))),
            Lsp::Missing(s) => Some(format!("{}: sem servidor", s.lang)),
            Lsp::Failed(s, _) => Some(format!("{}: servidor falhou", short(s))),
            Lsp::Off => None,
        }
    }

    /// Relê o blame quando o cursor muda de linha (300 ms parado).
    fn track_blame(&mut self, cx: &mut Context<Self>) {
        let (line, _) = self.cursor(cx);
        if self.blame.as_ref().is_some_and(|b| b.line == line) {
            return;
        }
        self.blame = Some(BlameLine {
            line,
            blame: None,
            mr: None,
        });
        self.blame_pop = false;
        let (root, path, text) = (self.root.clone(), self.path.clone(), self.buffer(cx));
        self._blame_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let b = cx
                .background_executor()
                .spawn(async move { blame::line(&root, &path, line, &text) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(bl) = this.blame.as_mut().filter(|bl| bl.line == line) {
                    bl.blame = b;
                    cx.notify();
                }
            })
            .ok();
        });
    }

    /// O MR que trouxe o commit, pela API do GitLab (só leitura), quando o popover abre.
    fn load_blame_mr(&mut self, cx: &mut Context<Self>) {
        let Some(bl) = self.blame.as_mut() else {
            return;
        };
        let Some(sha) = bl
            .blame
            .as_ref()
            .map(|b| b.sha.clone())
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        if bl.mr.is_some() {
            return;
        }
        bl.mr = Some(None);
        let (root, line) = (self.root.clone(), bl.line);
        cx.spawn(async move |this, cx| {
            let row = cx
                .background_executor()
                .spawn(async move {
                    let path = gitlab_path(&root)?;
                    let v = mrs::read(
                        &format!(
                            "projects/{}/repository/commits/{sha}/merge_requests",
                            mrs::enc(&path)
                        ),
                        false,
                    )
                    .ok()?;
                    mrs::rows(&v).into_iter().next()
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(bl) = this.blame.as_mut().filter(|bl| bl.line == line) {
                    bl.mr = Some(row);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn lsp_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let t = &self.theme;
        let (text, cmd) = match &self.lsp {
            Lsp::Missing(s) => (
                format!("Sem servidor de linguagem para {} · instale com", s.lang),
                Some(s.install),
            ),
            Lsp::Failed(s, e) => (
                format!("O servidor de linguagem de {} não subiu: {e}", s.lang),
                None,
            ),
            _ => return None,
        };
        if self.lsp_closed {
            return None;
        }
        Some(
            div()
                .flex()
                .gap(px(10.))
                .items_center()
                .px(px(12.))
                .py(px(5.))
                .flex_none()
                .bg(Hsla {
                    a: 0.10,
                    ..t.orange
                })
                .border_b_1()
                .border_color(t.line)
                .text_size(px(13.))
                .text_color(t.orange)
                .child(text)
                .children(cmd.map(|c| div().font_family(t.mono.clone()).text_color(t.fg).child(c)))
                .children(cmd.map(|c| {
                    button(t, "lsp-copy", "Copiar", false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(c.into()));
                            cx.emit(Event::Toast(format!("Copiado: {c}")));
                        },
                    ))
                }))
                .child(div().flex_1())
                .child(
                    div()
                        .id("lsp-x")
                        .px(px(4.))
                        .cursor_pointer()
                        .text_color(t.muted)
                        .hover(|d| d.text_color(t.fg))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.lsp_closed = true;
                            cx.notify();
                        }))
                        .child("×"),
                ),
        )
    }

    /// Sair do blame para o popover passa um instante fora dos dois: só fecha se continuar fora.
    fn close_pop_later(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.blame_hover != (false, false) {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            this.update(cx, |this, cx| {
                if this.blame_hover == (false, false) && this.blame_pop {
                    this.blame_pop = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// O blame apagado no fim da linha do cursor e, com o mouse em cima, o popover do commit.
    fn blame_overlay(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let t = self.theme.clone();
        let bl = self.blame.as_ref()?;
        let b = bl.blame.clone()?;
        let s = self.state.read(cx);
        let text = Rope::from(s.value().as_ref());
        let eol = text.line_end_offset(bl.line.saturating_sub(1));
        let end = s.range_to_bounds(&(eol..eol))?;
        let area = self.area.get();
        let (x, y) = (
            end.left() - area.origin.x + px(32.),
            end.top() - area.origin.y,
        );
        let pop_x = x.min(area.size.width - px(348.)).max(px(8.));
        let ago = blame::ago(blame::now().saturating_sub(b.time));
        let label = if b.sha.is_empty() {
            "você · não commitado".to_string()
        } else {
            format!("{} · {ago} · {}", b.author, b.summary)
        };
        let mr = bl.mr.clone().flatten();
        let mut out = div().absolute().top_0().left_0().child(
            div()
                .id("blame")
                .absolute()
                .left(x)
                .top(y)
                .h(end.size.height)
                .flex()
                .items_center()
                .whitespace_nowrap()
                .font_family(t.sans)
                .italic()
                .text_size(px(12.5))
                .text_color(if self.blame_pop { t.muted } else { t.comment })
                .on_hover(cx.listener(|this, h: &bool, _, cx| {
                    this.blame_hover.0 = *h;
                    if *h && !this.blame_pop {
                        this.blame_pop = true;
                        this.load_blame_mr(cx);
                    }
                    this.close_pop_later(cx);
                }))
                .child(label),
        );
        if self.blame_pop && !b.sha.is_empty() {
            let short: String = b.sha.chars().take(7).collect();
            out = out.child(
                div()
                    .id("blame-pop")
                    .absolute()
                    .left(pop_x)
                    .top(y + end.size.height)
                    .w(px(340.))
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(t.line)
                    .bg(t.raise)
                    .shadow_lg()
                    .font_family(t.sans)
                    .text_size(px(13.))
                    .occlude()
                    .on_hover(cx.listener(|this, h: &bool, _, cx| {
                        this.blame_hover.1 = *h;
                        this.close_pop_later(cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                div()
                                    .font_family(t.mono.clone())
                                    .text_color(rgb(0xf1fa8c))
                                    .child(short),
                            )
                            .child(format!("· {} · {ago}", b.author)),
                    )
                    .child(b.summary.clone())
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .when_some(mr, |d, row| {
                                let label = format!("Abrir MR !{}", row.iid);
                                d.child(button(&t, "blame-mr", &label, false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.blame_pop = false;
                                        cx.emit(Event::OpenMr(row.clone()));
                                    }),
                                ))
                            })
                            .child(
                                button(&t, "blame-hist", "Histórico do arquivo", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.blame_pop = false;
                                        cx.emit(Event::History);
                                    }),
                                ),
                            ),
                    ),
            );
        }
        Some(out)
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }

    fn buffer(&self, cx: &App) -> String {
        self.state.read(cx).value().to_string()
    }

    pub fn dirty(&self, cx: &App) -> bool {
        self.buffer(cx) != self.saved
    }

    /// Linha e coluna do cursor, para a barra de status.
    pub fn cursor(&self, cx: &App) -> (usize, usize) {
        let s = self.state.read(cx);
        let text = s.value();
        let o = s.cursor().min(text.len());
        let before = &text[..o];
        let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count());
        (before.matches('\n').count() + 1, col + 1)
    }

    pub fn selection_ref(&self, cx: &App) -> String {
        let s = self.state.read(cx);
        line_ref(&self.path, &s.value(), s.selected_range())
    }

    fn on_disk(&mut self, disk: String, window: &mut Window, cx: &mut Context<Self>) {
        let buffer = self.buffer(cx);
        match sync(&self.saved, &buffer, &disk) {
            Sync::Same => {
                if disk == buffer {
                    self.saved = disk;
                    self.conflict = None;
                }
                return;
            }
            Sync::Reload => self.load(disk, window, cx),
            Sync::Conflict => {
                if self.conflict.as_ref() == Some(&disk) {
                    return;
                }
                self.conflict = Some(disk);
            }
        }
        cx.emit(Event::Changed);
        cx.notify();
    }

    fn load(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let at = s.cursor().min(text.len());
            s.set_value(text.clone(), window, cx);
            s.set_selected_range(at..at, cx);
        });
        self.saved = text;
        self.conflict = None;
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer(cx);
        if text == self.saved {
            return cx.emit(Event::Toast("Nada a salvar".into()));
        }
        // o polling só lê a cada 1 s: sem reler aqui, a edição do Claude nessa janela some
        if let Ok(disk) = std::fs::read_to_string(&self.file)
            && disk != self.saved
        {
            self.conflict = Some(disk);
            cx.notify();
            return cx.emit(Event::Toast(
                "O arquivo mudou no disco; escolha no aviso".into(),
            ));
        }
        match std::fs::write(&self.file, &text) {
            Ok(()) => {
                self.saved = text;
                self.conflict = None;
                if let Lsp::Ready(doc) = &self.lsp {
                    doc.client.did_save(&self.file);
                }
                cx.emit(Event::Toast(format!("Salvo: {}", self.path)));
            }
            Err(e) => cx.emit(Event::Toast(format!(
                "Não deu para salvar {}: {e}",
                self.path
            ))),
        }
        cx.emit(Event::Changed);
        cx.notify();
    }
}

impl Render for FileEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        self.track_blame(cx);
        let bar = self.lsp_bar(cx);
        let blame = self.blame_overlay(cx);
        div()
            .key_context("MaestroEditor")
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                let m = &k.modifiers;
                // dentro do Ctrl+F ou do "Ir para a linha" estas teclas são do campo
                let here = this.state.read(cx).focus_handle(cx).is_focused(window);
                match (k.key.as_str(), m.control, m.alt, m.shift) {
                    ("s", true, false, false) => this.save(cx),
                    _ if !here => return,
                    ("/", true, false, false) => this.toggle_comment(window, cx),
                    ("up" | "down", false, true, false) => {
                        this.move_lines(k.key == "up", window, cx)
                    }
                    ("g", true, false, false) => this.open_goto(window, cx),
                    ("w", true, false, false) => cx.emit(Event::CloseTab),
                    ("tab", true, false, back) => cx.emit(Event::CycleTab(!back)),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .when(self.conflict.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .items_center()
                        .px(px(12.))
                        .py(px(6.))
                        .flex_none()
                        .bg(Hsla {
                            a: 0.12,
                            ..t.orange
                        })
                        .border_b_1()
                        .border_color(t.orange)
                        .child(
                            div()
                                .text_color(t.orange)
                                .child("Claude alterou este arquivo enquanto você editava"),
                        )
                        .child(div().flex_1())
                        .child(
                            button(&t, "cf-diff", "Ver diff", false).on_click(cx.listener(
                                |this, _, _, cx| cx.emit(Event::ShowDiff(this.path.clone())),
                            )),
                        )
                        .child(
                            button(&t, "cf-mine", "Manter meu", false).on_click(cx.listener(
                                |this, _, window, cx| {
                                    // o próximo Ctrl+S grava o seu por cima
                                    if let Some(disk) = this.conflict.take() {
                                        this.saved = disk;
                                    }
                                    window.focus(&this.focus_handle(cx), cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(button(&t, "cf-theirs", "Aceitar dele", true).on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(disk) = this.conflict.take() {
                                    this.load(disk, window, cx);
                                }
                                window.focus(&this.focus_handle(cx), cx);
                                cx.emit(Event::Changed);
                                cx.notify();
                            }),
                        )),
                )
            })
            .children(bar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        Editor::new(&self.state)
                            .appearance(false)
                            .bordered(false)
                            .h_full()
                            .font_family(t.mono.clone())
                            .text_size(px(14.))
                            .line_height(px(22.)),
                    )
                    .child({
                        let area = self.area.clone();
                        canvas(move |b, _, _| area.set(b), |_, _, _, _| {})
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                    })
                    .children(blame)
                    .children(self.goto_field.clone().map(|f| {
                        div()
                            .absolute()
                            .top(px(8.))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .w(px(260.))
                                    .p(px(6.))
                                    .bg(t.raise)
                                    .border_1()
                                    .border_color(t.accent)
                                    .rounded(px(6.))
                                    .shadow_lg()
                                    .child(f),
                            )
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Sync, line_block, line_ref, move_lines, sync, toggle_comment};

    #[test]
    fn comenta_e_descomenta() {
        assert_eq!(toggle_comment("  a\n\n    b", "//"), "  // a\n\n  //   b");
        assert_eq!(toggle_comment("  // a\n\n  //   b", "//"), "  a\n\n    b");
        assert_eq!(toggle_comment("x = 1", "#"), "# x = 1");
        assert_eq!(toggle_comment("#x", "#"), "x");
        assert_eq!(
            toggle_comment("  const nome = \"ação\"", "//"),
            "  // const nome = \"ação\""
        );
    }

    #[test]
    fn bloco_e_mover_linha() {
        let t = "a\nbb\nccc\n";
        assert_eq!(line_block(t, 3..3), 2..4);
        assert_eq!(line_block(t, 0..5), 0..4);
        assert_eq!(line_block(t, 2..7), 2..8);
        let (r, s, by) = move_lines(t, 2..4, true).unwrap();
        assert_eq!((r, s.as_str(), by), (0..4, "bb\na", -2));
        let (r, s, by) = move_lines(t, 2..4, false).unwrap();
        assert_eq!((r, s.as_str(), by), (2..8, "ccc\nbb", 4));
        assert!(move_lines(t, 0..1, true).is_none());
        assert!(move_lines("a\nb", 2..3, false).is_none());
        let (r, s, _) = move_lines("a\nb", 2..3, true).unwrap();
        assert_eq!((r, s.as_str()), (0..3, "b\na"));
        let t = "ação\né\n";
        let b = line_block(t, 7..7);
        assert_eq!(&t[b.clone()], "é");
        let (r, s, by) = move_lines(t, b, true).unwrap();
        assert_eq!((r, s.as_str(), by), (0..9, "é\nação", -7));
    }

    #[test]
    fn disco_mudou() {
        assert_eq!(sync("a", "a", "a"), Sync::Same);
        assert_eq!(sync("a", "a", "b"), Sync::Reload);
        assert_eq!(sync("a", "meu", "b"), Sync::Conflict);
        assert_eq!(sync("a", "b", "b"), Sync::Same);
    }

    #[test]
    fn referencia_da_selecao() {
        let t = "um\ndois\ntres\n";
        assert_eq!(line_ref("x.ts", t, 4..4), "@x.ts:2");
        assert_eq!(line_ref("x.ts", t, 0..9), "@x.ts:1-3");
    }
}
