use std::ops::Range;

use gpui_kit::{prelude::FluentBuilder, *};

use crate::theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub enum VaultGo {
    Graph,
    RepoNote,
    Today,
    Log,
    Open,
}

/// O que a paleta pede para o Shell fazer.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    NextAttention,
    NewFeature,
    RemoveFeature,
    Cleanup,
    AddRepoMenu,
    Compare,
    Review,
    AddRepo(std::path::PathBuf),
    Blocked,
    Tutorial,
    Herdr(&'static str),
    Interrupt,
    ToggleLeft,
    Features,
    Feature(String),
    SendSelection,
    OpenFileMenu,
    OpenFile(String),
    Save,
    Affected,
    SendReview,
    OpenMr,
    AttentionMenu,
    Mrs,
    MrReview,
    Vault(VaultGo),
    Go(crate::sidebar::Go),
    /// o modo Auditoria; `true` já abre a "Nova auditoria…"
    Audit(bool),
    Help,
    Home,
    /// a árvore do projeto na lateral e o editor, se houver arquivo aberto
    Files,
    /// a aba Arquivos com o foco no "buscar no projeto"
    SearchProject,
    /// "▶ Rodar tarefa…": as tarefas da worktree em foco
    TaskMenu,
    /// (nome, comando) de uma tarefa do repo
    Task(String, String),
    Problems,
    Kcmd,
    News,
    NewsBrief,
    /// o modo Bancos (túneis dos aliases do zsh)
    Db,
    /// Ctrl+Shift+D: o seletor de bancos, de qualquer tela
    DbMenu,
    Tunnel(String),
    TunnelsOff,
    /// "+" / Alt+T: a lista de repos para a aba nova
    RepoMenu,
    /// abre uma aba de trabalho nesta pasta
    OpenWork(std::path::PathBuf),
    Work(usize),
    /// `None`: a aba atual
    CloseWork(Option<usize>),
    NextWork(i32),
}

#[derive(Clone)]
pub struct Item {
    pub title: String,
    pub key: String,
    /// `None`: ainda não existe (aparece apagado, como os outros pontos do protótipo)
    pub cmd: Option<Cmd>,
    /// o `.pr` da fila de atenção: (rótulo, cor da prioridade)
    pub tag: Option<(String, Hsla)>,
}

/// O `CMDS` do protótipo, na mesma ordem. Sem o "Simular: …", que só existe no protótipo.
pub fn commands() -> Vec<Item> {
    use Cmd::*;
    let c = |t: &str, k: &str, cmd: Option<Cmd>| Item {
        title: t.into(),
        key: k.into(),
        cmd,
        tag: None,
    };
    vec![
        c(
            "Fila: Ir para o próximo item",
            "Ctrl+Shift+Espaço",
            Some(NextAttention),
        ),
        c(
            "Fila: Ver tudo que espera por você",
            "Alt+6",
            Some(AttentionMenu),
        ),
        c("Feature: Nova…", "Alt+N", Some(NewFeature)),
        c("Feature: Rodar testes afetados", "", Some(Affected)),
        c("Feature: Comparar tentativas", "", Some(Compare)),
        c("Feature: Ver MR da feature", "", Some(OpenMr)),
        c("Vault: Grafo de notas", "", Some(Vault(VaultGo::Graph))),
        c("Vault: Nota do repo", "", Some(Vault(VaultGo::RepoNote))),
        c("Vault: Daily de hoje", "", Some(Vault(VaultGo::Today))),
        c("Vault: Atividade", "", Some(Vault(VaultGo::Log))),
        c("Ir para: Vault", "Alt+4", Some(Vault(VaultGo::Open))),
        c("Ajuda: Rever tutorial desta tela", "", Some(Tutorial)),
        c("Feature: Remover a atual…", "", Some(RemoveFeature)),
        // não está no protótipo; o desenho (5.4) pede o comando
        c("Feature: Limpeza…", "", Some(Cleanup)),
        c("Feature: Adicionar repo…", "", Some(AddRepoMenu)),
        c("Feature: Ir para a que precisa de você", "", Some(Blocked)),
        c("Feature: Revisar mudanças", "Ctrl+Shift+E", Some(Review)),
        c(
            "Feature: Enviar comentários ao Claude",
            "Ctrl+⏎",
            Some(SendReview),
        ),
        c(
            "Claude: Enviar seleção para o terminal",
            "Ctrl+Shift+L",
            Some(SendSelection),
        ),
        c(
            "Terminal: Dividir lado a lado",
            "Ctrl+Shift+\\",
            Some(Herdr("split_vertical")),
        ),
        c(
            "Terminal: Dividir empilhado",
            "Ctrl+Shift+Enter",
            Some(Herdr("split_horizontal")),
        ),
        c("Terminal: Nova aba zsh", "Ctrl+A c", Some(Herdr("new_tab"))),
        c(
            "Terminal: Juntar splits (zoom)",
            "Ctrl+Shift+Z",
            Some(Herdr("zoom")),
        ),
        c("Terminal: Abrir terminal da home", "", Some(Home)),
        c(
            "Trabalho: Nova aba (este repo ou outro)…",
            "Alt+T",
            Some(RepoMenu),
        ),
        c("Trabalho: Fechar aba", "Alt+W", Some(CloseWork(None))),
        c("Trabalho: Próxima aba", "Alt+]", Some(NextWork(1))),
        c("Trabalho: Aba anterior", "Alt+[", Some(NextWork(-1))),
        c("Claude: Interromper", "Esc / Ctrl+C", Some(Interrupt)),
        c("Arquivo: Abrir…", "Ctrl+Shift+O", Some(OpenFileMenu)),
        c("Arquivo: Salvar", "Ctrl+S", Some(Save)),
        c(
            "Layout: Mostrar/ocultar lateral",
            "Ctrl+Shift+B",
            Some(ToggleLeft),
        ),
        c("Auditoria: Abrir relatório", "", Some(Audit(false))),
        c(
            "Auditoria: Nova auditoria incremental",
            "",
            Some(Audit(true)),
        ),
        c("Ir para: Features", "Alt+1", Some(Features)),
        c("Ir para: MRs", "Alt+2", Some(Mrs)),
        c("Arquivos: Árvore do projeto", "Ctrl+Shift+F", Some(Files)),
        c("Arquivos: Buscar no projeto", "", Some(SearchProject)),
        c("MR: Revisar com Claude", "⇧R", Some(MrReview)),
        c("Ir para: News", "Alt+5", Some(News)),
        c("News: Resumo do dia", "", Some(NewsBrief)),
        c("Ir para: Bancos", "Alt+3", Some(Db)),
        c("Banco: ligar ou desligar…", "Ctrl+Shift+D", Some(DbMenu)),
        c("Banco: desligar todos", "", Some(TunnelsOff)),
        c("Ajuda: Atalhos e decisões de UX", "?", Some(Help)),
        c("Terminal: Rodar tarefa…", "", Some(TaskMenu)),
        c("Editor: Problemas", "", Some(Problems)),
        c(
            "Terminal: Comando por descrição",
            "Ctrl+Shift+K",
            Some(Kcmd),
        ),
    ]
}

/// O `fuzzy` do protótipo: letras em ordem, sequência vale mais; -1 se não casa.
pub fn fuzzy(q: &str, s: &str) -> i32 {
    let q: Vec<char> = q.to_lowercase().chars().collect();
    let (mut j, mut score, mut last) = (0, 0, -2i32);
    for (i, c) in s.to_lowercase().chars().enumerate() {
        if j < q.len() && c == q[j] {
            score += if i as i32 == last + 1 { 3 } else { 1 };
            last = i as i32;
            j += 1;
        }
    }
    if j == q.len() { score } else { -1 }
}

/// Comandos e features filtrados como no `palItems` do protótipo.
pub fn filter(items: Vec<Item>, q: &str) -> Vec<Item> {
    let q = q.trim();
    let mut scored: Vec<(i32, Item)> = items
        .into_iter()
        .map(|it| {
            let base =
                if it.title.starts_with("Feature: ") && matches!(it.cmd, Some(Cmd::Feature(_))) {
                    // feature só aparece quando você busca por ela
                    if q.is_empty() {
                        -1
                    } else {
                        fuzzy(q, &it.title.to_lowercase())
                    }
                } else if q.is_empty() {
                    0
                } else {
                    fuzzy(q, &it.title)
                };
            (base, it)
        })
        .filter(|(s, _)| *s >= 0)
        .collect();
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored.into_iter().map(|(_, it)| it).collect()
}

pub enum Event {
    Run(Cmd),
    Close,
}

const SHOWN: usize = 200;

pub struct CommandPalette {
    query: String,
    selected: usize,
    items: Vec<Item>,
    all: Vec<Item>,
    placeholder: &'static str,
    empty: &'static str,
    focus: FocusHandle,
    theme: Theme,
    scroll: ScrollHandle,
}

impl EventEmitter<Event> for CommandPalette {}

impl CommandPalette {
    pub fn new(
        theme: Theme,
        all: Vec<Item>,
        placeholder: &'static str,
        empty: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut p = Self {
            query: String::new(),
            selected: 0,
            items: Vec::new(),
            all,
            placeholder,
            empty,
            focus: cx.focus_handle(),
            theme,
            scroll: ScrollHandle::new(),
        };
        p.refilter();
        p
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn refilter(&mut self) {
        self.items = filter(self.all.clone(), &self.query);
        self.selected = 0;
        self.scroll.scroll_to_item(0);
    }

    fn run(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(cmd) = self.items.get(i).and_then(|it| it.cmd.clone()) {
            cx.emit(Event::Run(cmd));
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => cx.emit(Event::Close),
            "enter" => self.run(self.selected, cx),
            "down" => {
                self.selected =
                    (self.selected + 1).min(self.items.len().min(SHOWN).saturating_sub(1));
                self.scroll.scroll_to_item(self.selected);
            }
            "up" => {
                self.selected = self.selected.saturating_sub(1);
                self.scroll.scroll_to_item(self.selected);
            }
            "backspace" => {
                self.query.pop();
                self.refilter();
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

// o texto chega pelo input handler, como no terminal: acento de tecla morta funciona igual
impl EntityInputHandler for CommandPalette {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.query.push_str(text);
        self.refilter();
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let entity = cx.entity();
        let focus = self.focus.clone();
        let input = div()
            .relative()
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(t.line)
            .text_size(px(15.))
            .flex()
            .items_center()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when(self.query.is_empty(), |d| {
                d.child(div().text_color(t.muted).child(self.placeholder))
            })
            .when(!self.query.is_empty(), |d| d.child(self.query.clone()))
            .child(div().w(px(1.)).h(px(16.)).ml(px(1.)).bg(t.fg));
        let mut list = div()
            .id("pal-list")
            .flex()
            .flex_col()
            .py(px(4.))
            .max_h(px(340.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if self.items.is_empty() {
            list = list.child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .text_color(t.muted)
                    .child(self.empty),
            );
        }
        // ponytail: desenha só os 200 primeiros (a lista de arquivos de um repo grande); lista virtual se fizer falta
        for (i, it) in self.items.iter().enumerate().take(SHOWN) {
            let ready = it.cmd.is_some();
            list = list.child(
                div()
                    .id(i)
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .px(px(12.))
                    .py(px(6.))
                    .when(ready, |d| d.cursor_pointer())
                    .when(!ready, |d| d.opacity(0.45))
                    .when(i == self.selected, |d| d.bg(t.accent_soft))
                    .on_click(cx.listener(move |this, _, _, cx| this.run(i, cx)))
                    .when_some(it.tag.clone(), |d, (tag, color)| {
                        d.child(
                            div()
                                .flex_none()
                                .font_family(t.mono.clone())
                                .text_size(px(11.))
                                .px(px(5.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(color)
                                .text_color(color)
                                .child(tag),
                        )
                    })
                    .child(it.title.clone())
                    .child(div().flex_1())
                    .when(!it.key.is_empty(), |d| {
                        d.child(
                            div()
                                .font_family(t.mono.clone())
                                .text_size(px(12.))
                                .px(px(4.))
                                .border_1()
                                .border_color(t.line)
                                .rounded(px(3.))
                                .text_color(t.muted)
                                .bg(t.panel)
                                .whitespace_nowrap()
                                .child(it.key.clone()),
                        )
                    }),
            );
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .w(px(560.))
            .max_w_full()
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .overflow_hidden()
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(input)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::{commands, filter, fuzzy};

    #[test]
    fn fuzzy_do_prototipo() {
        assert_eq!(fuzzy("div", "Terminal: Dividir lado a lado"), 7);
        assert_eq!(fuzzy("xyz", "Terminal"), -1);
        assert!(fuzzy("tdl", "Terminal: Dividir lado a lado") > 0);
        let r = filter(commands(), "dividir emp");
        assert_eq!(r[0].title, "Terminal: Dividir empilhado");
        assert_eq!(filter(commands(), "").len(), commands().len());
    }
}
