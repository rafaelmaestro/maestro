use gpui_kit::{prelude::FluentBuilder, *};

use std::time::Duration;

use crate::{
    ask::{AskEvent, AskView, KEvent, KcmdView},
    audit_view::{self, AuditView},
    changes,
    cleanup::{self, Cleanup},
    compare::{self, Attempt, Compare},
    confirm::{self, Answer, Choice, Chose, Confirm},
    editor, feature,
    form::button,
    herdr::{self, Status},
    keys,
    mascot::{self, Note},
    mrs_view::{self, MrsView},
    new_feature::{self, NewFeature},
    news_view::{self, NewsView},
    palette_cmd::{self, Cmd, CommandPalette},
    review::{self, Redo, Review},
    sidebar::{FsOp, Go, Sidebar, SidebarEvent},
    terminal::TerminalView,
    theme::Theme,
    tour::{self, Anchors},
    tunnels_view::{self, TunnelsView},
    vault_view::{self, VaultView},
};

/// Janela do protótipo: título (36) · lateral | terminal · dicas (22) · status (24).
pub struct Shell {
    pub sidebar: Option<Entity<Sidebar>>,
    pub terminal: Entity<TerminalView>,
    pub theme: Theme,
    pub font: String,
    prefix: bool,
    /// (passos, id do tutorial, passo atual)
    tour: Option<(&'static [tour::Step], &'static str, usize)>,
    anchors: Anchors,
    palette: Option<Entity<CommandPalette>>,
    dialog: Option<Entity<NewFeature>>,
    confirm: Option<Entity<Confirm>>,
    cleanup: Option<Entity<Cleanup>>,
    compare: Option<Entity<Compare>>,
    /// a gaveta "Perguntar ao Claude" e o Ctrl+Shift+K
    ask: Option<Entity<AskView>>,
    kcmd: Option<Entity<KcmdView>>,
    /// (id, texto, patch que refaz o que foi desfeito)
    toasts: Vec<(usize, String, Option<Redo>)>,
    /// toast de notificação de MR: id do toast → `path!iid` do "Abrir"
    toast_open: Vec<(usize, String)>,
    want_merge: Option<(String, String, crate::mrs::MergeOpts, bool)>,
    merge_dlg: Option<Entity<confirm::MergeDlg>>,
    review: Option<Entity<Review>>,
    want_review: Option<Option<String>>,
    want_file: Option<String>,
    want_fs: Option<FsOp>,
    want_merged: Option<(std::path::PathBuf, u64, String)>,
    /// "⎇ repo · N" da lateral: a revisão daquela worktree
    want_review_at: Option<std::path::PathBuf>,
    /// a pílula da pilha na lateral: o diálogo abre no próximo render
    want_stack: Option<(std::path::PathBuf, feature::Stack)>,
    /// a fila do mascote e o que está na tela: (pedido, sequência, saindo)
    mq: Vec<Note>,
    /// as permissões que aprendem: sugestões esperando o diálogo
    learn: Vec<crate::perms::Suggestion>,
    choice: Option<Entity<Choice>>,
    /// o modo Vault do título (Features · MRs · Vault)
    vault: Option<Entity<VaultView>>,
    in_vault: bool,
    /// o modo MRs (inbox, MR e revisor)
    mrs: Option<Entity<MrsView>>,
    in_mrs: bool,
    /// o modo News (daily.dev); nasce com o app para a contagem e o Resumo do dia
    news: Entity<NewsView>,
    in_news: bool,
    /// o modo Bancos; nasce com o app para a contagem do título e da barra de status
    tunnels: Entity<TunnelsView>,
    in_db: bool,
    /// o Resumo do dia na tela (não bloqueia nada)
    brief: bool,
    /// o modo Auditoria (dentro de Features, como no protótipo)
    audit: Option<Entity<AuditView>>,
    in_audit: bool,
    want_audit: bool,
    want_create: Option<(String, String)>,
    want_fix: Option<(String, String, String, String)>,
    /// o `openHelp` do protótipo
    help: bool,
    want_publish: Option<(String, String)>,
    mascot: Option<(Note, usize, bool)>,
    mascot_seq: usize,
    mascot_frame: u64,
    window_active: bool,
    maximized: bool,
    /// larguras arrastadas: a lateral e o painel da direita (`None` = o padrão do protótipo)
    left_w: f32,
    right_w: Option<f32>,
    _blink: Option<Task<()>>,
    toast_seq: usize,
    want_dialog: bool,
    left: bool,
}

/// As dicas da inbox e do MR (`HINTS.mrs`/`HINTS.mr` do protótipo).
const MRS_HINTS: &[(&str, &str, bool)] = &[
    ("j k", "navegar", true),
    ("Enter", "abrir", true),
    ("O", "abrir no GitLab", true),
    ("⇧R", "revisar com Claude", true),
    ("/", "filtrar", true),
];
const MR_HINTS: &[(&str, &str, bool)] = &[
    ("n p", "próximo/anterior arquivo", true),
    ("V", "visto e próximo", true),
    ("O", "abrir no GitLab", true),
    ("R", "atualizar", true),
    ("C", "comentar linha", true),
    ("?", "perguntar sobre o bloco", true),
    ("⇧R", "revisar com Claude", true),
    ("Ctrl+⏎", "publicar", true),
    ("Ctrl+Z", "desfazer descarte", true),
    ("Esc", "voltar", true),
];

/// As dicas do modo Vault (`HINTS.vault` do protótipo).
/// As dicas da auditoria (`HINTS.audit` do protótipo).
const AUDIT_HINTS: &[(&str, &str, bool)] = &[
    ("Nova auditoria…", "incremental ou completa", true),
    ("espaço", "marcar achado", true),
    ("?", "atalhos", true),
];

const DB_HINTS: &[(&str, &str, bool)] = &[
    ("Alt+3", "Bancos", true),
    ("j k", "navegar", true),
    ("Enter", "liga/desliga", true),
    ("/", "filtrar", true),
    ("Ctrl+Shift+D", "ligar de qualquer tela", true),
];

const NEWS_HINTS: &[(&str, &str, bool)] = &[
    ("Alt+5", "News", true),
    ("título", "abrir", true),
    ("Não relevante", "oculta só aqui", true),
    ("?", "atalhos", true),
];

const VAULT_HINTS: &[(&str, &str, bool)] = &[
    ("clique", "abrir nota", true),
    ("[[link]]", "navegar", true),
    ("◎ Grafo", "visão geral", true),
    ("Hoje", "daily", true),
    ("?", "atalhos", true),
];

#[derive(Clone, Copy)]
enum Resize {
    Left,
    Right,
}

enum Glyph {
    Min,
    /// já maximizada: o ícone de restaurar
    Max(bool),
    Close,
}

/// O símbolo do logo em branco (`maestro-symbol-small`, para menos de 48 px).
fn logo() -> std::sync::Arc<Image> {
    static LOGO: std::sync::OnceLock<std::sync::Arc<Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| {
        std::sync::Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../assets/icons/maestro-symbol-white-64.png").to_vec(),
        ))
    })
    .clone()
}

/// Os passos do "Experimente" da ajuda, com o app de verdade.
const HELP_STEPS: &[&str] = &[
    "Alt+N cria uma feature: worktree, .env copiado, setup do repo e o Claude com o seu prompt.",
    "Quando um Claude pede permissão ou termina, o mascote aparece; clique nele para ir ao pane.",
    "Ctrl+Shift+E abre as mudanças da feature: comente linhas e clique em Enviar ao Claude.",
    "Ctrl+Shift+Espaço leva ao próximo da fila de atenção; Fila lista tudo.",
    "Ctrl+Shift+\\ divide claude | zsh lado a lado; Ctrl+A c abre outra aba.",
    "Em Terminais ficam os workspaces sem Claude; ~ é o terminal da home.",
    "Abas de cima = trabalhos. Alt+T abre outra (o mesmo repo ou outro), Alt+[ Alt+] trocam, Alt+W fecha. A lateral é sempre da aba em foco.",
];

/// As heurísticas do `openHelp` do protótipo.
const NIELSEN: &[(&str, &str)] = &[
    (
        "Status visível",
        "Cada feature mostra o estado do agente (precisa de você, trabalhando, pronto, parado); a barra de status conta quantos esperam por você.",
    ),
    (
        "Linguagem do mundo real",
        "“Feature” em vez de “worktree”; termos do GitLab nos MRs.",
    ),
    (
        "Controle e liberdade",
        "Esc interrompe o Claude; comentários se descartam e se desfazem; nada vai ao Claude ou ao GitLab sem o seu clique.",
    ),
    (
        "Consistência",
        "Terminal com o mesmo visual e os mesmos atalhos do seu Ghostty; o mesmo diff para revisar o Claude e MRs; nenhum atalho do Maestro rouba Ctrl+tecla do shell.",
    ),
    (
        "Prevenção de erros",
        "Remover feature avisa sobre arquivos não commitados; nome de branch repetido é barrado no formulário.",
    ),
    (
        "Reconhecer, não lembrar",
        "A lateral substitui decorar qual aba do herdr é qual feature; a paleta acha feature pelo nome.",
    ),
    (
        "Eficiência",
        "Ctrl+Shift+1…9 troca de feature, Ctrl+Shift+L manda a seleção, os binds de split do seu Ghostty funcionam igual, Alt+N cria worktree com .env e npm ci prontos.",
    ),
    (
        "Minimalismo",
        "O terminal ocupa o centro; o editor só aparece quando você pede para revisar.",
    ),
    (
        "Recuperação de erros",
        "Erros do formulário dizem o que fazer; comentário que falhou no GitLab mostra o motivo e uma saída.",
    ),
    (
        "Ajuda",
        "Esta tela; dicas da faixa inferior mudam por modo.",
    ),
];

/// Dicas do modo Features do protótipo; `false` = ainda não existe e aparece apagada.
const HINTS: &[(&str, &str, bool)] = &[
    ("Alt+T", "nova aba", true),
    ("Alt+[ ]", "trocar aba", true),
    ("Ctrl+Shift+Espaço", "próximo da fila", true),
    ("Ctrl+Shift+1…9", "feature", true),
    ("Ctrl+Shift+E", "revisar mudanças", true),
    ("Ctrl+Shift+\\", "dividir", true),
    ("Ctrl+Shift+Enter", "empilhar", true),
    ("Ctrl+Shift+←→", "trocar pane", true),
    ("Ctrl+Shift+K", "comando por descrição", true),
    ("Ctrl+A", "herdr", true),
    ("Ctrl+Shift+L", "seleção → Claude", true),
    ("Alt+N", "nova feature", true),
    ("?", "atalhos", true),
];

impl Shell {
    pub fn new(
        sidebar: Option<Entity<Sidebar>>,
        terminal: Entity<TerminalView>,
        theme: Theme,
        font: String,
        anchors: Anchors,
        cx: &mut Context<Self>,
    ) -> Self {
        let sidebar_present = sidebar.is_some();
        let news = cx.new(|cx| NewsView::new(theme.clone(), anchors.clone(), cx));
        let tunnels = cx.new(|cx| TunnelsView::new(theme.clone(), cx));
        cx.subscribe(
            &tunnels,
            |this: &mut Self, _, e: &tunnels_view::Event, cx| match e {
                tunnels_view::Event::Toast(t) => this.toast(t.clone(), cx),
            },
        )
        .detach();
        cx.subscribe(
            &news,
            |this: &mut Self, _, e: &news_view::Event, cx| match e {
                news_view::Event::Toast(t) => this.toast(t.clone(), cx),
                news_view::Event::Vault(w) => this.toast_undo(
                    format!("vault: {}", w.desc),
                    Some(Redo::Vault(w.clone())),
                    cx,
                ),
                news_view::Event::Hidden(m, rule) => {
                    this.toast_undo(m.clone(), Some(Redo::News(rule.clone())), cx)
                }
                news_view::Event::Ready => {
                    // a primeira abertura do dia: o Resumo, se houver o que mostrar
                    let today = crate::vault::today();
                    if crate::news::has_token()
                        && !this.news.read(cx).brief().is_empty()
                        && crate::news::brief_due(&today)
                    {
                        crate::news::brief_shown(&today);
                        this.brief = true;
                        cx.notify();
                    }
                }
            },
        )
        .detach();
        cx.observe(&news, |_, _, cx| cx.notify()).detach();
        if let Some(s) = &sidebar {
            cx.observe(s, |this: &mut Self, s, cx| {
                // ir para a feature (por qualquer caminho) dispensa o mascote dela, como o `selectFeat`
                let focused = s
                    .read(cx)
                    .agents
                    .iter()
                    .find(|a| a.focused)
                    .map(|a| a.pane_id.clone());
                if this.window_active
                    && this
                        .mascot
                        .as_ref()
                        .is_some_and(|(n, _, out)| !out && Some(&n.pane) == focused.as_ref())
                {
                    this.dismiss_mascot(cx);
                }
                cx.notify();
            })
            .detach();
            // o botão da lateral não tem a janela; o diálogo abre no próximo render
            cx.subscribe(s, |this: &mut Self, _, e: &SidebarEvent, cx| {
                match e {
                    SidebarEvent::NewFeature => this.want_dialog = true,
                    SidebarEvent::OpenReview(p) => this.want_review = Some(Some(p.clone())),
                    SidebarEvent::OpenFile(p) => this.want_file = Some(p.clone()),
                    SidebarEvent::Merged(wt, iid, branch) => {
                        this.want_merged = Some((wt.clone(), *iid, branch.clone()))
                    }
                    SidebarEvent::Notify(n) => this.notify_note(n.clone(), cx),
                    SidebarEvent::Learn(s) => this.learn.push(s.clone()),
                    SidebarEvent::Vault(r) => match r {
                        Ok(w) => this.toast_undo(
                            format!("vault: {}", w.desc),
                            Some(Redo::Vault(w.clone())),
                            cx,
                        ),
                        Err(e) => this.toast(format!("vault: não deu para escrever ({e})"), cx),
                    },
                    SidebarEvent::Audit => this.want_audit = true,
                    SidebarEvent::Picked => this.leave_modes(),
                    SidebarEvent::OpenReviewAt(p) => this.want_review_at = Some(p.clone()),
                    SidebarEvent::Stack(wt, st) => this.want_stack = Some((wt.clone(), *st)),
                    SidebarEvent::Fs(op) => this.want_fs = Some(op.clone()),
                    SidebarEvent::Toast(m) => this.toast(m.clone(), cx),
                }
                cx.notify();
            })
            .detach();
        }
        // o terminal notifica a cada saída do PTY; o chrome só redesenha quando o modo PREFIX muda
        cx.observe(&terminal, |this: &mut Self, t, cx| {
            let prefix = t.read(cx).prefix_pending();
            if prefix != this.prefix {
                this.prefix = prefix;
                cx.notify();
            }
        })
        .detach();
        // o vault é observado sempre, com ou sem o modo Vault aberto: o que mudar fora do Maestro
        // (o Claude seguindo o contrato, você no Obsidian) vai para a Atividade com desfazer
        cx.spawn(async move |this, cx| {
            let (mut last, mut notes) = (0u64, None::<Vec<crate::vault::Note>>);
            loop {
                let (sig, new) = cx
                    .background_executor()
                    .spawn(async move {
                        let root = crate::vault::dir();
                        let sig = vault_view::signature(&root);
                        (sig, (sig != last).then(|| crate::vault::load(&root)))
                    })
                    .await;
                last = sig;
                if let Some(new) = new {
                    let changes = notes
                        .as_ref()
                        .map(|old| crate::vault::external(old, &new))
                        .unwrap_or_default();
                    notes = Some(new);
                    let alive = this.update(cx, |this, cx| {
                        for w in changes {
                            this.toast_undo(format!("vault: {}", w.desc), Some(Redo::Vault(w)), cx);
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        })
        .detach();
        Self {
            sidebar,
            terminal,
            theme,
            font,
            prefix: false,
            // tutorial da área na primeira vez, como no protótipo
            tour: (sidebar_present && !tour::seen("feat")).then_some((tour::FEAT, "feat", 0)),
            anchors,
            palette: None,
            dialog: None,
            confirm: None,
            cleanup: None,
            compare: None,
            ask: None,
            kcmd: None,
            toasts: Vec::new(),
            toast_open: Vec::new(),
            want_merge: None,
            merge_dlg: None,
            review: None,
            want_review: None,
            want_file: None,
            want_fs: None,
            want_merged: None,
            want_review_at: None,
            want_stack: None,
            mq: Vec::new(),
            learn: Vec::new(),
            choice: None,
            vault: None,
            in_vault: false,
            news,
            in_news: false,
            tunnels,
            in_db: false,
            brief: false,
            mrs: None,
            in_mrs: false,
            audit: None,
            in_audit: false,
            want_audit: false,
            want_create: None,
            want_fix: None,
            help: false,
            want_publish: None,
            mascot: None,
            mascot_seq: 0,
            mascot_frame: 0,
            window_active: true,
            maximized: false,
            left_w: 270.,
            right_w: None,
            _blink: None,
            toast_seq: 0,
            want_dialog: false,
            left: true,
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        if m.alt && !m.control && !m.shift {
            let cmd = match k.key.as_str() {
                "n" => return self.open_new_feature(window, cx),
                "t" => Cmd::RepoMenu,
                "w" => Cmd::CloseWork(None),
                "]" => Cmd::NextWork(1),
                "[" => Cmd::NextWork(-1),
                "1" => Cmd::Features,
                "2" => Cmd::Mrs,
                "3" => Cmd::Db,
                "4" => Cmd::Vault(crate::palette_cmd::VaultGo::Open),
                "5" => Cmd::News,
                "6" => Cmd::AttentionMenu,
                _ => return,
            };
            cx.stop_propagation();
            return self.run(cmd, window, cx);
        }
        if m.control && m.shift && !m.alt {
            match k.key.as_str() {
                "p" => return self.toggle_palette(window, cx),
                "e" => return self.toggle_review(None, window, cx),
                "l" => return self.run(Cmd::SendSelection, window, cx),
                "o" => return self.run(Cmd::OpenFileMenu, window, cx),
                "b" => return self.run(Cmd::ToggleLeft, window, cx),
                "f" => return self.run(Cmd::Files, window, cx),
                "k" => return self.run(Cmd::Kcmd, window, cx),
                "d" => return self.run(Cmd::DbMenu, window, cx),
                _ => {}
            }
        }
        if m.control && m.shift && !m.alt && k.key == "space" {
            return self.run(Cmd::NextAttention, window, cx);
        }
        // `?` só fora do terminal e dos campos de texto: no shell ele é só um caractere
        let views = [
            self.mrs.as_ref().map(|v| v.read(cx).focus_handle()),
            self.vault.as_ref().map(|v| v.read(cx).focus_handle()),
            self.audit.as_ref().map(|v| v.read(cx).focus_handle()),
        ];
        if (k.key == "?" || k.key_char.as_deref() == Some("?"))
            && views.iter().flatten().any(|f| f.is_focused(window))
        {
            self.help = true;
            cx.stop_propagation();
            return cx.notify();
        }
        let Some(sidebar) = &self.sidebar else { return };
        let s = sidebar.read(cx);
        let target = if let Some(n) = keys::feature_digit(k) {
            s.order_here().into_iter().nth(n - 1)
        } else {
            return;
        };
        if let Some(pane) = target {
            s.focus(pane);
        }
        self.leave_modes();
        window.focus(&self.terminal.read(cx).focus_handle(), cx);
        cx.stop_propagation();
        cx.notify();
    }

    /// Ctrl+Shift+→ na borda direita do terminal entra no painel de revisão; Ctrl+Shift+← nele volta.
    fn focus_arrow(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &k.modifiers;
        if !(m.control && m.shift && !m.alt) {
            return false;
        }
        let Some(review) = self.review.clone() else {
            return false;
        };
        let terminal = self.terminal.read(cx).focus_handle();
        match k.key.as_str() {
            "right" if terminal.is_focused(window) => {
                let herdr = self.sidebar.as_ref().map(|s| s.read(cx).herdr());
                if herdr.is_some_and(|h| h.has_neighbor("right").unwrap_or(true)) {
                    return false;
                }
                let f = review.read(cx).focus_handle(cx);
                window.focus(&f, cx);
            }
            "left" if review.read(cx).contains_focus(window, cx) => window.focus(&terminal, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn resize_handle(&self, side: Resize) -> impl IntoElement {
        let id = match side {
            Resize::Left => "rz-left",
            Resize::Right => "rz-right",
        };
        crate::theme::resize_handle(id, side, &self.theme)
    }

    /// Volta para o terminal de Features (feature escolhida pela lateral ou por Ctrl+Shift+1…9).
    fn leave_modes(&mut self) {
        self.in_news = false;
        self.in_db = false;
        self.in_vault = false;
        self.in_mrs = false;
        self.in_audit = false;
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            window.focus(&self.terminal.read(cx).focus_handle(), cx);
        } else {
            let mut items = palette_cmd::commands();
            if let Some(s) = &self.sidebar {
                let s = s.read(cx);
                for (i, (pane, label)) in s.features().into_iter().enumerate() {
                    items.push(palette_cmd::Item {
                        title: format!("Feature: {label}"),
                        key: if i < 9 {
                            format!("Ctrl+Shift+{}", i + 1)
                        } else {
                            String::new()
                        },
                        cmd: Some(Cmd::Feature(pane)),
                        tag: None,
                    });
                }
            }
            items.extend(self.task_items(cx));
            self.open_palette(
                items,
                "Comando ou nome da feature…",
                "Nada encontrado",
                window,
                cx,
            );
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn open_palette(
        &mut self,
        items: Vec<palette_cmd::Item>,
        placeholder: &'static str,
        empty: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        {
            let theme = self.theme.clone();
            let p = cx.new(|cx| CommandPalette::new(theme, items, placeholder, empty, cx));
            cx.subscribe_in(&p, window, |this, _, e: &palette_cmd::Event, window, cx| {
                let cmd = match e {
                    palette_cmd::Event::Run(c) => Some(c.clone()),
                    palette_cmd::Event::Close => None,
                };
                this.toggle_palette(window, cx);
                if let Some(c) = cmd {
                    this.run(c, window, cx);
                }
            })
            .detach();
            window.focus(&p.read(cx).focus_handle(), cx);
            self.palette = Some(p);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn open_new_feature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // o passo 4 do tutorial termina com este clique, como no protótipo
        if matches!(self.tour, Some((_, "feat", i)) if i == tour::FEAT.len() - 1) {
            self.end_tour(cx);
        }
        let Some(sidebar) = &self.sidebar else { return };
        let s = sidebar.read(cx);
        let (Some(repo), herdr) = (s.top_repo(), s.herdr()) else {
            return;
        };
        let theme = self.theme.clone();
        let d = cx.new(|cx| NewFeature::new(herdr, repo, theme, window, cx));
        cx.subscribe_in(&d, window, |this, _, e: &new_feature::Event, window, cx| {
            let new_feature::Event::Done(warning) = e;
            if let Some(w) = warning {
                this.toast(w.clone(), cx);
            }
            this.dialog = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            cx.notify();
        })
        .detach();
        self.dialog = Some(d);
        cx.stop_propagation();
        cx.notify();
    }

    /// MR novo ou resposta: toast com "Abrir" (6 s); com a janela fora de foco, também `notify-send`.
    fn notify_mr(
        &mut self,
        text: String,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        self.toasts.push((id, text.clone(), None));
        self.toast_open.push((id, key.clone()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(6000))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|(i, _, _)| *i != id);
                this.toast_open.retain(|(i, _)| *i != id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        if self.window_active {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            // `-A` espera o clique por horas: numa thread do SO, não no pool do background executor
            let (tx, rx) = futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let _ = tx.send(
                    std::process::Command::new("notify-send")
                        .args(["-a", "Maestro", "-A", "open=Abrir", "Maestro", &text])
                        .output()
                        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "open"),
                );
            });
            let open = rx.await.unwrap_or(false);
            if open {
                let _ = this.update_in(cx, |this, window, cx| {
                    window.activate_window();
                    this.open_mr(&key, window, cx);
                });
            }
        })
        .detach();
    }

    fn open_mr(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.run(Cmd::Mrs, window, cx);
        if let Some(m) = &self.mrs {
            m.update(cx, |m, cx| m.open_key(key, cx));
        }
    }

    pub fn toast(&mut self, msg: String, cx: &mut Context<Self>) {
        self.toast_undo(msg, None, cx);
    }

    /// Toast com "Desfazer" (6 s, como no protótipo) quando vem o patch que refaz.
    fn toast_undo(&mut self, msg: String, redo: Option<Redo>, cx: &mut Context<Self>) {
        self.toast_seq += 1;
        let id = self.toast_seq;
        let ms = if redo.is_some() { 6000 } else { 2800 };
        self.toasts.push((id, msg, redo));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(ms))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.toasts.retain(|(i, _, _)| *i != id);
                this.toast_open.retain(|(i, _)| *i != id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// O `removeFeatureDlg` do protótipo, apagando o que o inventário registrou.
    fn remove_feature(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.remove_feature_at(None, None, window, cx)
    }

    /// `root`: a worktree de uma feature de fundo (o MR dela mergeou); `None`, a feature em foco.
    /// A pílula da pilha: "base mudou · Rebase" ou "mergeada · Mover p/ main", sempre com confirmação.
    fn stack_dialog(
        &mut self,
        wt: std::path::PathBuf,
        st: feature::Stack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(inv) = feature::find_by_path(&wt) else {
            return;
        };
        let Some(p) = inv.parent.clone() else {
            return;
        };
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let mr = s.mr_of(&wt).filter(|m| !m.merged && !m.closed);
        let mother_mr = feature::inventories()
            .into_iter()
            .find(|m| m.id == p.id)
            .and_then(|m| s.mr_of(&m.worktrees[0].path));
        let pane = s
            .agents
            .iter()
            .find(|a| std::path::Path::new(&a.cwd).starts_with(&wt))
            .map(|a| a.pane_id.clone());
        let (title, text, ok) = match st {
            feature::Stack::Rebase => (
                format!("Rebase de {} em {}?", inv.branch, p.branch),
                format!(
                    "{0} ganhou commits depois que esta foi criada. Roda git rebase {0} na worktree desta feature; se der conflito, para e abre o terminal dela com a lista.",
                    p.branch
                ),
                "Rebase".to_string(),
            ),
            feature::Stack::MoveToBase => (
                format!("Mover {} para {}?", inv.branch, p.base),
                format!(
                    "{}{} foi mergeado. Roda git rebase --onto {} {} nesta worktree{}.",
                    p.branch,
                    mother_mr.map_or(String::new(), |m| format!(" (!{})", m.iid)),
                    p.base,
                    p.branch,
                    mr.as_ref().map_or(String::new(), |m| format!(
                        " e muda o alvo do MR !{} para {} no GitLab",
                        m.iid, p.base
                    )),
                ),
                format!("Mover para {}", p.base),
            ),
        };
        let theme = self.theme.clone();
        let c = cx.new(|cx| Confirm::new(theme, title, text, &ok, false, window, cx));
        cx.subscribe_in(&c, window, move |this, _, a: &Answer, window, cx| {
            this.confirm = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            if matches!(a, Answer::Ok) {
                let (mut inv, mr, pane) = (inv.clone(), mr.clone(), pane.clone());
                let mother = p.branch.clone();
                cx.spawn(async move |this, cx| {
                    let branch = inv.branch.clone();
                    let r = cx
                        .background_executor()
                        .spawn(async move {
                            match st {
                                feature::Stack::Rebase => feature::rebase_on_parent(&mut inv)
                                    .map(|_| format!("{branch} rebaseada em {mother}")),
                                feature::Stack::MoveToBase => {
                                    let base = feature::move_to_base(&mut inv)?;
                                    let mut msg = format!("{branch} agora sai de {base}");
                                    if let Some(m) = &mr {
                                        crate::mr::retarget(m, &base)
                                            .map_err(|e| format!("rebase ok, mas o GitLab recusou o alvo novo: {e}"))?;
                                        msg.push_str(&format!("; MR !{} → {base}", m.iid));
                                    }
                                    Ok(msg)
                                }
                            }
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| match r {
                        Ok(msg) => this.toast(msg, cx),
                        Err(e) => {
                            // conflito: o rebase fica parado e o terminal da filha vem para a frente
                            if let (Some(pane), Some(s)) = (&pane, &this.sidebar) {
                                s.read(cx).focus(pane.clone());
                            }
                            this.leave_modes();
                            this.toast(
                                format!("Rebase parou: {e}. Resolva no terminal da feature e rode git rebase --continue."),
                                cx,
                            );
                        }
                    });
                })
                .detach();
            }
            cx.notify();
        })
        .detach();
        self.confirm = Some(c);
        cx.notify();
    }

    fn remove_feature_at(
        &mut self,
        root: Option<std::path::PathBuf>,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let wt = root.or_else(|| s.top_worktree());
        let focused = s
            .agents
            .iter()
            .find(|a| {
                wt.as_ref()
                    .is_some_and(|w| std::path::Path::new(&a.cwd).starts_with(w))
            })
            .cloned();
        let herdr = s.herdr();
        let Some(inv) = wt.as_deref().and_then(feature::find_by_path) else {
            let main = wt.as_ref().is_some_and(|w| herdr::main_repo(w) == *w);
            return self.toast(
                if main {
                    "A main é o checkout principal e não pode ser removida".into()
                } else {
                    "Este terminal não é uma feature criada pelo Maestro".into()
                },
                cx,
            );
        };
        let p = feature::pending(&inv);
        let working = focused.is_some_and(|a| a.agent_status == Status::Working);
        let multi = inv.worktrees.len() > 1;
        let warn = if p.uncommitted > 0 {
            format!(
                "{} arquivo{} não commitado{}{} {} junto com {}.",
                p.uncommitted,
                if p.uncommitted > 1 { "s" } else { "" },
                if p.uncommitted > 1 { "s" } else { "" },
                if p.dirty.is_empty() {
                    String::new()
                } else {
                    let list: Vec<String> =
                        p.dirty.iter().map(|(r, n)| format!("{r}: {n}")).collect();
                    format!(" ({})", list.join(", "))
                },
                if p.uncommitted > 1 {
                    "vão ser apagados"
                } else {
                    "vai ser apagado"
                },
                if multi { "as worktrees" } else { "a worktree" }
            )
        } else if working {
            "O Claude ainda está trabalhando nela e vai ser encerrado.".into()
        } else if p.merged {
            format!(
                "Nada pendente. A branch já foi mergeada em {} e sai junto com a pasta.",
                inv.base
            )
        } else {
            "Nada pendente. A branch local continua existindo; só a pasta da worktree sai.".into()
        };
        let dirs: Vec<String> = inv
            .worktrees
            .iter()
            .map(|w| crate::sidebar::tilde(&w.path.to_string_lossy()))
            .collect();
        let text = format!(
            "{warn} {}: {}",
            if multi { "Pastas" } else { "Pasta" },
            dirs.join(", ")
        );
        let theme = self.theme.clone();
        let title = title.unwrap_or_else(|| format!("Remover a feature {}?", inv.branch));
        let danger = p.uncommitted > 0 || working;
        let ok = if multi {
            "Remover worktrees"
        } else {
            "Remover worktree"
        };
        let c = cx.new(|cx| Confirm::new(theme, title, text, ok, danger, window, cx));
        cx.subscribe_in(&c, window, move |this, _, a: &Answer, window, cx| {
            this.confirm = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            if matches!(a, Answer::Ok) {
                let (herdr, inv) = (herdr.clone(), inv.clone());
                cx.spawn(async move |this, cx| {
                    let branch = inv.branch.clone();
                    let r = cx
                        .background_executor()
                        .spawn(async move { feature::conclude(&herdr, &inv) })
                        .await;
                    let _ = this.update(cx, |this, cx| match r {
                        Ok(_) => this.toast(format!("Worktree {branch} removida"), cx),
                        Err(e) => this.toast(format!("Não deu para remover: {e}"), cx),
                    });
                })
                .detach();
            }
            cx.notify();
        })
        .detach();
        self.confirm = Some(c);
        cx.notify();
    }

    /// Botão direito na aba Arquivos: mexe no disco e leva as abas do editor junto.
    fn file_op(&mut self, op: FsOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let Some(root) = sidebar.read(cx).top_worktree() else {
            return;
        };
        // as abas só são desta árvore se a revisão estiver na mesma worktree (multi-repo troca a dela)
        let review = self.review.clone().filter(|r| r.read(cx).root == root);
        let dirty = |p: &str, cx: &App| {
            review
                .as_ref()
                .is_some_and(|r| r.read(cx).dirty_under(p, cx))
        };
        match op {
            FsOp::Create(rel, dir) => match changes::create(&root, &rel, dir) {
                Ok(rel) => {
                    sidebar.update(cx, |s, cx| s.reload_files(Some(&rel), cx));
                    if !dir {
                        self.run(Cmd::OpenFile(rel), window, cx);
                    }
                }
                Err(e) => self.toast(e, cx),
            },
            FsOp::Copy(from, dir) => match changes::copy(&root, &from, &dir) {
                Ok(rel) => {
                    sidebar.update(cx, |s, cx| s.reload_files(Some(&rel), cx));
                    self.toast(format!("Colado: {rel}"), cx);
                }
                Err(e) => self.toast(e, cx),
            },
            FsOp::Rename(from, to) => {
                if dirty(&from, cx) {
                    return self.toast(
                        format!("{from} tem alterações não salvas: salve antes de renomear"),
                        cx,
                    );
                }
                match changes::rename(&root, &from, &to) {
                    Ok(to) => {
                        if let Some(r) = &review {
                            r.update(cx, |r, cx| r.moved(&from, &to, window, cx));
                        }
                        sidebar.update(cx, |s, cx| s.reload_files(Some(&to), cx));
                    }
                    Err(e) => self.toast(e, cx),
                }
            }
            FsOp::Delete(rel, dir) => {
                if dirty(&rel, cx) {
                    return self.toast(
                        format!("{rel} tem alterações não salvas: salve antes de excluir"),
                        cx,
                    );
                }
                let title = if dir {
                    let inner = format!("{rel}/");
                    let n = changes::files(&root)
                        .iter()
                        .filter(|f| f.starts_with(&inner) && !f.ends_with('/'))
                        .count();
                    format!(
                        "Excluir a pasta {rel} e {n} arquivo{}?",
                        if n == 1 { "" } else { "s" }
                    )
                } else {
                    format!("Excluir {rel}?")
                };
                let text = "Some do disco. O que o git já conhece volta com git restore; o resto não volta.".to_string();
                let theme = self.theme.clone();
                let c = cx.new(|cx| Confirm::new(theme, title, text, "Excluir", true, window, cx));
                cx.subscribe_in(&c, window, move |this, _, a: &Answer, window, cx| {
                    this.confirm = None;
                    window.focus(&this.terminal.read(cx).focus_handle(), cx);
                    if matches!(a, Answer::Ok) {
                        match changes::remove(&root, &rel) {
                            Ok(()) => {
                                if let Some(r) = &review {
                                    r.update(cx, |r, cx| r.forget(&rel, window, cx));
                                }
                                sidebar.update(cx, |s, cx| s.reload_files(None, cx));
                                this.toast(format!("{rel} excluído"), cx);
                            }
                            Err(e) => this.toast(format!("Não deu para excluir {rel}: {e}"), cx),
                        }
                    }
                    cx.notify();
                })
                .detach();
                self.confirm = Some(c);
                cx.notify();
            }
        }
    }

    /// O `compareDlg` do protótipo para o grupo da feature em foco.
    fn open_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let wt = s.top_worktree();
        let group = wt
            .as_deref()
            .and_then(feature::find_by_path)
            .and_then(|i| i.group);
        let Some(group) =
            group.or_else(|| feature::inventories().into_iter().find_map(|i| i.group))
        else {
            return self.toast(
                "Nenhuma feature com várias tentativas. Crie uma com Tentativas: 2.".into(),
                cx,
            );
        };
        let mut invs: Vec<_> = feature::inventories()
            .into_iter()
            .filter(|i| i.group.as_ref() == Some(&group))
            .collect();
        invs.sort_by(|a, b| a.branch.cmp(&b.branch));
        let attempts: Vec<Attempt> = invs
            .into_iter()
            .map(|inv| {
                let path = &inv.worktrees[0].path;
                let agent = s
                    .agents
                    .iter()
                    .find(|a| std::path::Path::new(&a.cwd).starts_with(path));
                let st = agent.map_or(Status::Unknown, |a| a.agent_status);
                let ch = changes::changes(path).unwrap_or_default();
                Attempt {
                    state: (
                        crate::sidebar::glyph(st).1.to_string(),
                        self.status_color(st),
                    ),
                    done: matches!(st, Status::Done | Status::Idle),
                    files: ch.files.len(),
                    added: ch.files.iter().map(|f| f.added).sum(),
                    removed: ch.files.iter().map(|f| f.removed).sum(),
                    cost: agent.and_then(|a| s.usage.get(&a.pane_id)).map(|u| u.cost),
                    inv,
                }
            })
            .collect();
        let herdr = s.herdr();
        let theme = self.theme.clone();
        let invs: Vec<_> = attempts.iter().map(|a| a.inv.clone()).collect();
        let c = cx.new(|cx| Compare::new(group, attempts, theme, window, cx));
        cx.subscribe_in(
            &c,
            window,
            move |this, _, e: &compare::Event, window, cx| {
                this.compare = None;
                window.focus(&this.terminal.read(cx).focus_handle(), cx);
                if let compare::Event::Diff(i) = e {
                    let inv = &invs[*i];
                    if let (Some(s), Some(ws)) = (&this.sidebar, inv.workspace_id.clone()) {
                        s.read(cx).focus_workspace(ws);
                    }
                    if let Some(w) = inv.worktrees.first() {
                        this.open_review_at(w.path.clone(), None, window, cx);
                    }
                }
                if let compare::Event::Keep(i) = e {
                    let keep = invs[*i].clone();
                    let rest: Vec<_> = invs.iter().filter(|x| x.id != keep.id).cloned().collect();
                    let names: Vec<_> = rest.iter().map(|x| x.branch.clone()).collect();
                    let theme = this.theme.clone();
                    let herdr = herdr.clone();
                    let title = format!("Ficar com {}?", keep.branch);
                    let text =
                        format!("Remove {} (worktrees e branches locais).", names.join(", "));
                    let d = cx.new(|cx| {
                        Confirm::new(theme, title, text, "Ficar com esta", false, window, cx)
                    });
                    cx.subscribe_in(&d, window, move |this, _, a: &Answer, window, cx| {
                        this.confirm = None;
                        window.focus(&this.terminal.read(cx).focus_handle(), cx);
                        if matches!(a, Answer::Ok) {
                            let (herdr, keep, rest) = (herdr.clone(), keep.clone(), rest.clone());
                            cx.spawn(async move |this, cx| {
                                let n = rest.len();
                                let branch = keep.branch.clone();
                                let r = cx
                                    .background_executor()
                                    .spawn(async move {
                                        rest.iter()
                                            .try_for_each(|x| feature::discard(&herdr, x))?;
                                        feature::keep(&keep)
                                    })
                                    .await;
                                let _ = this.update(cx, |this, cx| match r {
                                    Ok(()) => this.toast(
                                        format!(
                                            "Ficou {branch}; {n} tentativa{} removida{}",
                                            if n > 1 { "s" } else { "" },
                                            if n > 1 { "s" } else { "" }
                                        ),
                                        cx,
                                    ),
                                    Err(e) => this
                                        .toast(format!("Não deu para remover as outras: {e}"), cx),
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    })
                    .detach();
                    this.confirm = Some(d);
                }
                cx.notify();
            },
        )
        .detach();
        self.compare = Some(c);
        cx.notify();
    }

    /// Painel "Revisar" da feature em foco (`Ctrl+Shift+E`); com `path`, abre já nesse arquivo.
    fn toggle_review(&mut self, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self
            .sidebar
            .as_ref()
            .and_then(|s| s.read(cx).top_worktree());
        if path.is_none() && self.review.take().is_some() {
            window.focus(&self.terminal.read(cx).focus_handle(), cx);
            return cx.notify();
        }
        let Some(root) = root else {
            return self.toast("Abra um terminal num repositório para revisar".into(), cx);
        };
        self.open_review_at(root, path, window, cx)
    }

    /// O painel de uma worktree escolhida (a da fila, que pode ainda não estar em foco).
    fn open_review_at(
        &mut self,
        root: std::path::PathBuf,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .review
            .as_ref()
            .is_some_and(|r| r.read(cx).root == root);
        if !same {
            let branch = herdr::branch(&root).unwrap_or_default();
            let theme = self.theme.clone();
            let anchors = self.anchors.clone();
            let r = cx.new(|cx| Review::new(root.clone(), branch.clone(), theme, anchors, cx));
            cx.observe(&r, |this, r, cx| {
                // o arquivo aberto (e se está sujo) aparece marcado na aba Arquivos da lateral
                let open = r.read(cx).current_file(cx);
                if let Some(s) = &this.sidebar {
                    s.update(cx, |s, cx| s.set_open(open, cx));
                }
                cx.notify();
            })
            .detach();
            cx.subscribe_in(
                &r,
                window,
                move |this, r, e: &review::Event, window, cx| match e {
                    review::Event::Close => {
                        this.review = None;
                        window.focus(&this.terminal.read(cx).focus_handle(), cx);
                        cx.notify();
                    }
                    review::Event::Toast(m, redo) => this.toast_undo(m.clone(), redo.clone(), cx),
                review::Event::RunTests(cmd) => {
                    let root = r.read(cx).root.clone();
                    let Some(s) = this.sidebar.clone() else { return };
                    let s = s.read(cx);
                    let target = s
                        .agents
                        .iter()
                        .find(|a| std::path::Path::new(&a.cwd).starts_with(&root))
                        .map(|a| a.pane_id.clone());
                    let (herdr, cmd) = (s.herdr(), cmd.clone());
                    // um split embaixo do Claude da feature, como no protótipo
                    std::thread::spawn(move || {
                        let p = herdr.call(
                            "pane.split",
                            serde_json::json!({ "direction": "down", "cwd": root, "target_pane_id": target, "focus": false }),
                        )?;
                        let pane = p["pane"]["pane_id"].as_str().unwrap_or_default().to_string();
                        herdr.call("pane.send_text", serde_json::json!({ "pane_id": pane, "text": format!("{cmd}\r") }))
                    });
                    this.toast("Testes afetados rodando num split".into(), cx);
                }
                    review::Event::Send(text) => {
                        let root = r.read(cx).root.clone();
                        this.ask_claude(
                            &root,
                            text.clone(),
                            format!("Revisão enviada ao Claude de {branch}"),
                            cx,
                        )
                    }
                    review::Event::Ask(text, ok) => {
                        let root = r.read(cx).root.clone();
                        this.ask_claude(&root, text.clone(), ok.clone(), cx)
                    }
                    review::Event::Conclude => {
                        let root = r.read(cx).root.clone();
                        this.remove_feature_at(Some(root), None, window, cx)
                    }
                    review::Event::AskClaude(c) => this.open_ask(c.clone(), window, cx),
                    review::Event::MrOpened => {
                        if this.tour.is_none() && !tour::seen("mrtab") {
                            this.tour = Some((tour::MRTAB, "mrtab", 0));
                            cx.notify();
                        }
                    }
                    review::Event::OpenMr(row) => {
                        this.run(Cmd::Mrs, window, cx);
                        if let Some(m) = this.mrs.clone() {
                            m.update(cx, |m, cx| m.open_row(row.clone(), cx));
                        }
                    }
                },
            )
            .detach();
            self.review = Some(r);
            if self.tour.is_none() && !tour::seen("review") {
                self.tour = Some((tour::REVIEW, "review", 0));
            }
        }
        if let (Some(p), Some(r)) = (path, &self.review) {
            r.update(cx, |r, cx| r.show_file(p, cx));
        }
        if let Some(r) = &self.review {
            let f = r.read(cx).focus_handle(cx);
            window.focus(&f, cx);
        }
        cx.notify();
    }

    fn mrs_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<MrsView> {
        if let Some(m) = &self.mrs {
            return m.clone();
        }
        let m = cx.new(|cx| MrsView::new(self.theme.clone(), self.anchors.clone(), cx));
        cx.subscribe_in(
            &m,
            window,
            |this, _, e: &mrs_view::Event, window, cx| match e {
                mrs_view::Event::Toast(t) => this.toast(t.clone(), cx),
                mrs_view::Event::Discarded(t) => {
                    this.toast_undo(t.clone(), Some(Redo::MrDraft), cx)
                }
                mrs_view::Event::AskClaude(c) => this.open_ask(c.clone(), window, cx),
                mrs_view::Event::AskPublish(a, b) => {
                    this.want_publish = Some((a.clone(), b.clone()));
                    cx.notify();
                }
                mrs_view::Event::AskMerge(a, b, o, running) => {
                    this.want_merge = Some((a.clone(), b.clone(), o.clone(), *running));
                    cx.notify();
                }
                mrs_view::Event::Notify(text, key) => {
                    this.notify_mr(text.clone(), key.clone(), window, cx)
                }
            },
        )
        .detach();
        cx.observe(&m, |_, _, cx| cx.notify()).detach();
        self.mrs = Some(m.clone());
        m
    }

    fn audit_view(&mut self, cx: &mut Context<Self>) -> Entity<AuditView> {
        let repo = self.sidebar.as_ref().and_then(|s| s.read(cx).top_repo());
        if let Some(a) = &self.audit {
            a.update(cx, |a, cx| a.set_repo(repo, cx));
            return a.clone();
        }
        let a = cx.new(|cx| {
            let mut a = AuditView::new(self.theme.clone(), self.anchors.clone(), cx);
            a.set_repo(repo, cx);
            a
        });
        cx.subscribe(&a, |this, _, e: &audit_view::Event, cx| {
            match e {
                audit_view::Event::Toast(t) => this.toast(t.clone(), cx),
                audit_view::Event::Ignored(ig) => this.toast_undo(
                    "Ignorado: não volta nas próximas auditorias (.maestro/audit-ignore.json)"
                        .into(),
                    Some(Redo::Audit(ig.clone())),
                    cx,
                ),
                audit_view::Event::AskCreate(a, b) => {
                    this.want_create = Some((a.clone(), b.clone()))
                }
                audit_view::Event::AskFix(a, b, c, d) => {
                    this.want_fix = Some((a.clone(), b.clone(), c.clone(), d.clone()))
                }
            }
            cx.notify();
        })
        .detach();
        cx.observe(&a, |_, _, cx| cx.notify()).detach();
        self.audit = Some(a.clone());
        a
    }

    /// "Corrigir em feature": a worktree `fix/<iid>-slug` com o Claude e o work item como tarefa.
    fn fix_in_feature(&mut self, branch: String, prompt: String, cx: &mut Context<Self>) {
        let Some(repo) = self.audit.as_ref().and_then(|a| a.read(cx).repo()) else {
            return;
        };
        let Some(base) = feature::bases(&repo).into_iter().next() else {
            return self.toast(
                "O repositório não tem branch para servir de base.".into(),
                cx,
            );
        };
        let req = feature::Request {
            repo,
            branch,
            base,
            prompt,
            copy_env: true,
            setup: true,
            claude: true,
            model: None,
            group: None,
            others: Vec::new(),
            parent: None,
        };
        let used: Vec<u16> = feature::inventories().iter().map(|i| i.port).collect();
        let port = feature::next_port(&used);
        let Some(herdr) = self.sidebar.as_ref().map(|s| s.read(cx).herdr()) else {
            return;
        };
        self.in_audit = false;
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { feature::create(&herdr, &req, port) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Err(e) => this.toast(format!("Não deu para criar a feature: {e}"), cx),
                    Ok(Some(w)) => this.toast(w, cx),
                    Ok(None) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn vault_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<VaultView> {
        if let Some(v) = &self.vault {
            return v.clone();
        }
        let (sidebar, theme, anchors) = (
            self.sidebar.clone(),
            self.theme.clone(),
            self.anchors.clone(),
        );
        let v = cx.new(|cx| VaultView::new(sidebar, theme, anchors, cx));
        cx.subscribe_in(
            &v,
            window,
            |this, _, e: &vault_view::Event, window, cx| match e {
                vault_view::Event::Toast(m, redo) => this.toast_undo(m.clone(), redo.clone(), cx),
                vault_view::Event::Focus(p) => {
                    this.in_vault = false;
                    this.in_news = false;
                    this.in_db = false;
                    if let Some(s) = &this.sidebar {
                        s.read(cx).focus(p.clone());
                    }
                    window.focus(&this.terminal.read(cx).focus_handle(), cx);
                    cx.notify();
                }
            },
        )
        .detach();
        self.vault = Some(v.clone());
        v
    }

    /// O `learnPerm` do protótipo: sempre permitir, só para você ou para o time.
    fn ask_learn(
        &mut self,
        s: crate::perms::Suggestion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo = s
            .repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = s
            .rule
            .strip_prefix("Bash(")
            .and_then(|r| r.strip_suffix(":*)"))
            .unwrap_or(&s.rule)
            .to_string();
        let title = format!("Você aprovou `{key}` {} vezes em {repo}", s.count);
        let text = format!(
            "Em {} feature{} nesta semana. Sempre permitir `{}` neste repo?",
            s.features,
            if s.features > 1 { "s" } else { "" },
            s.rule
        );
        let options = vec![
            ("Agora não".to_string(), false),
            (
                "Só pra mim (.claude/settings.local.json)".to_string(),
                false,
            ),
            (
                "Para o time (.claude/settings.json, vai no git)".to_string(),
                true,
            ),
        ];
        let theme = self.theme.clone();
        let c = cx.new(|cx| Choice::new(theme, title, text, options, window, cx));
        cx.subscribe_in(&c, window, move |this, _, Chose(i), window, cx| {
            this.choice = None;
            window.focus(&this.terminal.read(cx).focus_handle(), cx);
            crate::perms::mark(&s);
            let r = match i {
                Some(1) => crate::perms::allow_personal(&s)
                    .map(|_| format!("Permitido para você: {}", s.rule)),
                Some(2) => crate::perms::allow_team(&s).map(|_| {
                    format!(
                        "{} adicionado ao .claude/settings.json. Entra no próximo commit.",
                        s.rule
                    )
                }),
                _ => return cx.notify(),
            };
            match r {
                Ok(m) => this.toast(m, cx),
                Err(e) => this.toast(format!("Não deu para gravar a regra: {e}"), cx),
            }
        })
        .detach();
        self.choice = Some(c);
        cx.notify();
    }

    /// O `notify(f)` do protótipo: quem está olhando a feature não precisa do mascote.
    fn notify_note(&mut self, n: Note, cx: &mut Context<Self>) {
        let focused = self.sidebar.as_ref().and_then(|s| {
            s.read(cx)
                .agents
                .iter()
                .find(|a| a.focused)
                .map(|a| a.pane_id.clone())
        });
        if self.window_active && focused.as_ref() == Some(&n.pane) {
            return;
        }
        let shown = self
            .mascot
            .as_ref()
            .is_some_and(|(m, _, _)| m.pane == n.pane);
        if !shown && !self.mq.iter().any(|m| m.pane == n.pane) {
            self.mq.push(n);
        }
        if self.mascot.is_none() {
            self.next_mascot(cx);
        }
        cx.notify();
    }

    fn next_mascot(&mut self, cx: &mut Context<Self>) {
        if self.mq.is_empty() {
            self.mascot = None;
            self._blink = None;
            return cx.notify();
        }
        let n = self.mq.remove(0);
        mascot::play(n.kind);
        self.mascot_seq += 1;
        self.mascot = Some((n, self.mascot_seq, false));
        // o piscar do protótipo: um quadro a cada 450 ms
        self._blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(450))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.mascot_frame += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn dismiss_mascot(&mut self, cx: &mut Context<Self>) {
        let Some((n, seq, false)) = self.mascot.clone() else {
            return;
        };
        self.mq.retain(|m| m.pane != n.pane);
        self.mascot = Some((n, seq, true));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.mascot.as_ref().is_some_and(|m| m.1 == seq) {
                    this.mascot = None;
                    this.next_mascot(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// `agent.prompt` no Claude da worktree; o toast diz se chegou.
    fn ask_claude(
        &mut self,
        root: &std::path::Path,
        text: String,
        ok: String,
        cx: &mut Context<Self>,
    ) {
        let (Some(pane), Some(s)) = (self.claude_pane(root, cx), self.sidebar.clone()) else {
            return self.toast("Nenhum Claude rodando nesta worktree".into(), cx);
        };
        let herdr = s.read(cx).herdr();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    herdr.call(
                        "agent.prompt",
                        serde_json::json!({ "target": pane, "text": text }),
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(_) => this.toast(ok, cx),
                Err(e) => this.toast(format!("O Claude não recebeu: {e}"), cx),
            });
        })
        .detach();
    }

    /// O pane do Claude que roda na worktree `root`.
    fn claude_pane(&self, root: &std::path::Path, cx: &App) -> Option<String> {
        let s = self.sidebar.as_ref()?.read(cx);
        s.agents
            .iter()
            .find(|a| std::path::Path::new(&a.cwd).starts_with(root))
            .map(|a| a.pane_id.clone())
    }

    /// O `sendSel` do protótipo: `@arquivo:linhas` digitado no Claude da feature, sem Enter.
    fn send_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.review.clone() else {
            return self.toast("Selecione linhas no editor ou no diff".into(), cx);
        };
        let (root, sel) = {
            let r = r.read(cx);
            (r.root.clone(), r.selection_ref(cx))
        };
        let Some(sel) = sel else {
            return self.toast("Selecione linhas no editor ou no diff".into(), cx);
        };
        let (Some(pane), Some(s)) = (self.claude_pane(&root, cx), self.sidebar.clone()) else {
            return self.toast(
                "Este terminal não tem Claude; escolha uma feature".into(),
                cx,
            );
        };
        let s = s.read(cx);
        let herdr = s.herdr();
        s.focus(pane.clone());
        std::thread::spawn(move || {
            herdr.call(
                "pane.send_text",
                serde_json::json!({ "pane_id": pane, "text": format!("{sel} ") }),
            )
        });
        window.focus(&self.terminal.read(cx).focus_handle(), cx);
        cx.stop_propagation();
        cx.notify();
    }

    /// "Tarefa: <nome>" para cada script/alvo da worktree em foco.
    fn task_items(&self, cx: &App) -> Vec<palette_cmd::Item> {
        let root = self
            .sidebar
            .as_ref()
            .and_then(|s| s.read(cx).top_worktree());
        root.map(|r| crate::tasks::detect(&r))
            .unwrap_or_default()
            .into_iter()
            .map(|t| palette_cmd::Item {
                title: format!("Tarefa: {}", t.name),
                key: t.cmd.clone(),
                cmd: Some(Cmd::Task(t.name, t.cmd)),
                tag: None,
            })
            .collect()
    }

    /// Roda a tarefa num split do herdr e, quando ela termina, joga os erros na aba Problemas.
    fn run_task(&mut self, name: String, cmd: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let s = sidebar.read(cx);
        let Some(root) = s.top_worktree() else {
            return self.toast("Este terminal não está num repo: sem tarefas".into(), cx);
        };
        let target = s
            .agents
            .iter()
            .find(|a| std::path::Path::new(&a.cwd).starts_with(&root))
            .map(|a| a.pane_id.clone());
        let herdr = s.herdr();
        let (tx, rx) = futures::channel::oneshot::channel();
        let r = root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::tasks::run_in_split(&herdr, &r, target, &cmd));
        });
        self.toast(format!("{name}: rodando num split"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(res) = rx.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                let (code, probs) = match res {
                    Ok(x) => x,
                    Err(e) => return this.toast(format!("{name}: {e}"), cx),
                };
                let n = probs.len();
                if n > 0 || this.review.is_some() {
                    if this.review.as_ref().is_none_or(|r| r.read(cx).root != root) {
                        this.open_review_at(root.clone(), None, window, cx);
                    }
                    if let Some(rv) = &this.review {
                        rv.update(cx, |rv, cx| {
                            rv.set_task_problems(name.clone(), probs, cx);
                            if n > 0 {
                                rv.show_problems(cx);
                            }
                        });
                    }
                }
                let msg = match (n, code) {
                    (0, 0) => format!("{name}: passou, nenhum problema"),
                    (0, c) => format!("{name}: saiu com {c}, sem erros com arquivo e linha"),
                    (1, _) => format!("{name}: 1 problema, veja em Problemas"),
                    (n, _) => format!("{name}: {n} problemas, veja em Problemas"),
                };
                this.toast(msg, cx);
            });
        })
        .detach();
    }

    fn open_ask(&mut self, ctx: crate::ask::Ctx, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let a = cx.new(|cx| AskView::new(ctx, theme, cx));
        cx.subscribe_in(&a, window, |this, _, e: &AskEvent, window, cx| match e {
            AskEvent::Close => {
                this.ask = None;
                // o foco volta para quem abriu a gaveta: o MR ou o painel de revisão
                let back = match (&this.mrs, &this.review) {
                    (Some(m), _) if this.in_mrs => m.read(cx).focus_handle(),
                    (_, Some(r)) => r.read(cx).focus_handle(cx),
                    _ => this.terminal.read(cx).focus_handle(),
                };
                window.focus(&back, cx);
                cx.notify();
            }
        })
        .detach();
        window.focus(&a.read(cx).focus_handle(cx), cx);
        self.ask = Some(a);
        cx.notify();
    }

    fn open_kcmd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_terminal = self.terminal.read(cx).focus_handle().is_focused(window)
            && !self.in_mrs
            && !self.in_vault
            && !self.in_audit;
        let herdr = self.sidebar.as_ref().map(|s| s.read(cx).herdr());
        let (true, Some(herdr)) = (on_terminal, herdr) else {
            return self.toast("Ctrl+Shift+K funciona com o terminal em foco".into(), cx);
        };
        let theme = self.theme.clone();
        let k = cx.new(|cx| KcmdView::new(herdr, theme, cx));
        cx.subscribe_in(&k, window, |this, _, e: &KEvent, window, cx| {
            this.kcmd = None;
            if matches!(e, KEvent::Typed) {
                this.toast(
                    "Comando digitado no zsh. Enter roda; nada foi executado.".into(),
                    cx,
                );
            }
            // o Enter que confirmou ainda vira texto depois deste evento: com o terminal já em foco, ele rodaria o comando
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    window.focus(&this.terminal.read(cx).focus_handle(), cx);
                });
            })
            .detach();
            cx.notify();
        })
        .detach();
        window.focus(&k.read(cx).focus_handle(cx), cx);
        self.kcmd = Some(k);
        cx.stop_propagation();
        cx.notify();
    }

    /// "Arquivo: Abrir…": os arquivos da worktree em foco, na mesma paleta.
    fn open_file_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self
            .sidebar
            .as_ref()
            .and_then(|s| s.read(cx).top_worktree())
        else {
            return self.toast(
                "Abra um terminal num repositório para abrir arquivos".into(),
                cx,
            );
        };
        let items = crate::changes::files(&root)
            .into_iter()
            .filter(|p| !p.ends_with('/'))
            .map(|p| palette_cmd::Item {
                title: p.clone(),
                key: String::new(),
                cmd: Some(Cmd::OpenFile(p)),
                tag: None,
            })
            .collect();
        self.open_palette(items, "Nome do arquivo…", "Nada encontrado", window, cx);
    }

    fn run(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.sidebar.clone();
        let focus = |pane: Option<String>, cx: &mut Context<Self>| {
            if let (Some(s), Some(p)) = (&sidebar, pane) {
                s.read(cx).focus(p);
            }
        };
        match cmd {
            // o `nextAttention` do protótipo
            Cmd::NextAttention => {
                let q = sidebar
                    .as_ref()
                    .map(|s| s.read(cx).attention())
                    .unwrap_or_default();
                let Some(first) = q.first() else {
                    return self.toast("Fila vazia: nada esperando você".into(), cx);
                };
                let rest = if q.len() > 1 {
                    format!(" · faltam {}", q.len() - 1)
                } else {
                    String::new()
                };
                self.toast(format!("Fila: {}{rest}", first.text), cx);
                return self.run(Cmd::Go(first.go.clone()), window, cx);
            }
            Cmd::AttentionMenu => {
                let t = self.theme.clone();
                let items = sidebar
                    .as_ref()
                    .map(|s| s.read(cx).attention())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|i| palette_cmd::Item {
                        title: i.text,
                        key: String::new(),
                        tag: Some((
                            i.tag.into(),
                            match i.p {
                                0 => t.orange,
                                1 => rgb(0xff5555).into(),
                                2 => t.ok,
                                3 => t.accent,
                                _ => t.muted,
                            },
                        )),
                        cmd: Some(Cmd::Go(i.go)),
                    })
                    .collect();
                return self.open_palette(
                    items,
                    "Fila de atenção: filtrar…",
                    "Fila vazia. Nada esperando você.",
                    window,
                    cx,
                );
            }
            Cmd::Go(go) => {
                let pane = match &go {
                    Go::Pane(p) | Go::Mr(p, _) | Go::Review(p, _) | Go::Compare(p) => {
                        Some(p.clone())
                    }
                    Go::Audit => None,
                };
                if pane.is_some() {
                    self.leave_modes();
                }
                focus(pane, cx);
                match go {
                    Go::Audit => return self.run(Cmd::Audit(false), window, cx),
                    Go::Pane(_) => {}
                    Go::Mr(_, wt) => {
                        self.open_review_at(wt, None, window, cx);
                        if let Some(r) = &self.review {
                            r.update(cx, |r, cx| r.open_mr(cx));
                        }
                        return cx.notify();
                    }
                    Go::Review(_, wt) => return self.open_review_at(wt, None, window, cx),
                    Go::Compare(_) => return self.open_compare(window, cx),
                }
            }
            Cmd::Blocked => focus(
                sidebar.as_ref().and_then(|s| s.read(cx).first_blocked()),
                cx,
            ),
            Cmd::Feature(p) => {
                self.leave_modes();
                focus(Some(p), cx)
            }
            Cmd::Help => self.help = true,
            Cmd::TaskMenu => {
                let items = self.task_items(cx);
                if items.is_empty() {
                    return self.toast(
                        "Nenhuma tarefa: sem package.json, Makefile, Cargo.toml ou go.mod aqui"
                            .into(),
                        cx,
                    );
                }
                return self.open_palette(items, "Tarefa…", "Nenhuma tarefa", window, cx);
            }
            Cmd::Task(name, cmd) => return self.run_task(name, cmd, window, cx),
            Cmd::Kcmd => return self.open_kcmd(window, cx),
            Cmd::Problems => {
                let Some(root) = self
                    .sidebar
                    .as_ref()
                    .and_then(|s| s.read(cx).top_worktree())
                else {
                    return self.toast(
                        "Abra um terminal num repositório para ver os problemas".into(),
                        cx,
                    );
                };
                self.leave_modes();
                if self.review.as_ref().is_none_or(|r| r.read(cx).root != root) {
                    self.open_review_at(root, None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.show_problems(cx));
                }
            }
            Cmd::Files => {
                self.left = true;
                self.in_vault = false;
                self.in_news = false;
                self.in_db = false;
                self.in_mrs = false;
                self.in_audit = false;
                if let Some(s) = &self.sidebar {
                    s.update(cx, |s, cx| s.show_files(cx));
                }
                if let Some(r) = &self.review
                    && r.update(cx, |r, cx| r.file_tab(cx))
                {
                    let f = r.read(cx).focus_handle(cx);
                    window.focus(&f, cx);
                } else {
                    window.focus(&self.terminal.read(cx).focus_handle(), cx);
                }
            }
            Cmd::RepoMenu => {
                let top = self.sidebar.as_ref().and_then(|s| s.read(cx).top_repo());
                let mut repos: Vec<std::path::PathBuf> =
                    std::fs::read_dir(crate::mrs::repos_root())
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.join(".git").is_dir())
                        .collect();
                if let Some(t) = &top {
                    repos.extend(feature::sibling_repos(t));
                }
                repos.sort();
                repos.dedup();
                repos.retain(|r| Some(r) != top.as_ref());
                let name = |r: &std::path::Path| {
                    r.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                let dir = |r: &std::path::Path| {
                    crate::sidebar::tilde(&r.parent().unwrap_or(r).to_string_lossy())
                };
                let mut items: Vec<palette_cmd::Item> = top
                    .iter()
                    .map(|t| palette_cmd::Item {
                        title: format!("{} · outro trabalho no mesmo repo", name(t)),
                        key: dir(t),
                        cmd: Some(Cmd::OpenWork(t.clone())),
                        tag: None,
                    })
                    .collect();
                items.extend(repos.iter().map(|r| palette_cmd::Item {
                    title: name(r),
                    key: dir(r),
                    cmd: Some(Cmd::OpenWork(r.clone())),
                    tag: None,
                }));
                items.push(palette_cmd::Item {
                    title: "~ (só terminal, sem repo)".into(),
                    key: String::new(),
                    cmd: Some(Cmd::OpenWork(std::path::PathBuf::from(
                        std::env::var("HOME").unwrap_or_default(),
                    ))),
                    tag: None,
                });
                return self.open_palette(
                    items,
                    "Nova aba em… (Enter = mesmo repo, ou digite outro)",
                    "Nenhum repositório com esse nome",
                    window,
                    cx,
                );
            }
            Cmd::OpenWork(dir) => {
                let Some(s) = self.sidebar.clone() else {
                    return;
                };
                let herdr = s.update(cx, |s, _| {
                    s.fresh = true;
                    s.herdr()
                });
                let home = std::env::var("HOME").unwrap_or_default();
                let label = if dir.as_os_str() == home.as_str() {
                    "~".to_string()
                } else {
                    dir.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                std::thread::spawn(move || {
                    herdr.call(
                        "workspace.create",
                        serde_json::json!({ "cwd": dir, "label": label, "focus": true }),
                    )
                });
                self.leave_modes();
                window.focus(&self.terminal.read(cx).focus_handle(), cx);
            }
            Cmd::Work(i) => {
                if let Some(s) = &self.sidebar {
                    s.update(cx, |s, cx| s.go_work(i, cx));
                }
                self.leave_modes();
                window.focus(&self.terminal.read(cx).focus_handle(), cx);
            }
            Cmd::NextWork(d) => {
                let Some(s) = &self.sidebar else { return };
                s.update(cx, |s, cx| {
                    let n = s.works.len() as i32;
                    if n > 1 {
                        s.go_work((s.cur as i32 + d).rem_euclid(n) as usize, cx);
                    }
                });
                self.leave_modes();
                window.focus(&self.terminal.read(cx).focus_handle(), cx);
            }
            Cmd::CloseWork(i) => {
                let Some(s) = &self.sidebar else { return };
                let s = s.read(cx);
                if let Err(e) = s.close_work(i.unwrap_or(s.cur)) {
                    return self.toast(e.into(), cx);
                }
            }
            Cmd::Home => {
                let Some(s) = &self.sidebar else { return };
                let s = s.read(cx);
                if let Some(w) = s.workspaces.iter().find(|w| w.label == "~") {
                    s.focus_workspace(w.workspace_id.clone());
                } else {
                    let herdr = s.herdr();
                    let home = std::env::var("HOME").unwrap_or_default();
                    std::thread::spawn(move || {
                        herdr.call(
                            "workspace.create",
                            serde_json::json!({ "cwd": home, "label": "~", "focus": true }),
                        )
                    });
                }
                self.in_vault = false;
                self.in_news = false;
                self.in_db = false;
                self.in_mrs = false;
                self.in_audit = false;
                window.focus(&self.terminal.read(cx).focus_handle(), cx);
            }
            Cmd::Tutorial => {
                let on_mr = self.review.as_ref().is_some_and(|r| r.read(cx).on_mr());
                let mrs_open = self.in_mrs;
                self.tour = Some(if self.in_audit {
                    (tour::AUDIT, "audit", 0)
                } else if self.in_vault {
                    (tour::VAULT, "vault", 0)
                } else if mrs_open {
                    (tour::MRS, "mrs", 0)
                } else if on_mr {
                    (tour::MRTAB, "mrtab", 0)
                } else if self.review.is_some() {
                    (tour::REVIEW, "review", 0)
                } else {
                    (tour::FEAT, "feat", 0)
                })
            }
            Cmd::Herdr(a) => self.terminal.update(cx, |t, _| t.herdr_action(a)),
            Cmd::Interrupt => self.terminal.update(cx, |t, _| t.send(b"\x1b")),
            Cmd::ToggleLeft => self.left = !self.left,
            Cmd::Features => {
                self.in_vault = false;
                self.in_news = false;
                self.in_db = false;
                self.in_mrs = false;
                self.in_audit = false;
            }
            Cmd::Audit(new) => {
                let a = self.audit_view(cx);
                a.update(cx, |a, cx| a.refresh(cx));
                if new {
                    a.update(cx, |a, cx| a.new_audit(true, cx));
                }
                self.in_audit = true;
                self.in_vault = false;
                self.in_news = false;
                self.in_db = false;
                self.in_mrs = false;
                let f = a.read(cx).focus_handle();
                window.focus(&f, cx);
                if self.tour.is_none() && !tour::seen("audit") {
                    self.tour = Some((tour::AUDIT, "audit", 0));
                }
                return cx.notify();
            }
            Cmd::Mrs | Cmd::MrReview => {
                let m = self.mrs_view(window, cx);
                self.in_mrs = true;
                self.in_vault = false;
                self.in_news = false;
                self.in_db = false;
                self.in_audit = false;
                if cmd == Cmd::MrReview {
                    m.update(cx, |m, cx| m.start_review(cx));
                }
                let f = m.read(cx).focus_handle();
                window.focus(&f, cx);
                if self.tour.is_none() && !tour::seen("mrs") {
                    self.tour = Some((tour::MRS, "mrs", 0));
                }
                return cx.notify();
            }
            Cmd::Vault(to) => {
                let repo = self
                    .sidebar
                    .as_ref()
                    .and_then(|s| s.read(cx).top_repo())
                    .and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned()));
                let v = self.vault_view(window, cx);
                v.update(cx, |v, cx| v.go(&to, repo, cx));
                self.in_vault = true;
                self.in_news = false;
                self.in_db = false;
                self.in_mrs = false;
                self.in_audit = false;
                let f = v.read(cx).focus_handle();
                window.focus(&f, cx);
                if self.tour.is_none() && !tour::seen("vault") {
                    self.tour = Some((tour::VAULT, "vault", 0));
                }
                return cx.notify();
            }
            Cmd::Db => {
                self.in_db = true;
                self.in_news = false;
                self.in_vault = false;
                self.in_mrs = false;
                self.in_audit = false;
                let f = self.tunnels.read(cx).focus_handle();
                window.focus(&f, cx);
                return cx.notify();
            }
            Cmd::DbMenu => {
                let red: Hsla = rgb(0xff5555).into();
                let items = self
                    .tunnels
                    .read(cx)
                    .items()
                    .into_iter()
                    .map(|(name, port, up, prd)| palette_cmd::Item {
                        title: format!(
                            "{} {name}{}",
                            if up { "●" } else { "○" },
                            if up { " · desligar" } else { "" }
                        ),
                        key: format!(":{port}"),
                        cmd: Some(Cmd::Tunnel(name)),
                        tag: prd.then(|| ("prd".to_string(), red)),
                    })
                    .collect();
                return self.open_palette(
                    items,
                    "Banco… (Enter liga ou desliga · os aliases do zsh)",
                    "Nenhum alias de túnel no zsh",
                    window,
                    cx,
                );
            }
            Cmd::Tunnel(name) => {
                self.tunnels.update(cx, |t, cx| t.toggle(&name, cx));
                return cx.notify();
            }
            Cmd::TunnelsOff => {
                self.tunnels.update(cx, |t, cx| t.stop_all(cx));
                return cx.notify();
            }
            Cmd::News => {
                self.brief = false;
                self.in_news = true;
                self.in_db = false;
                self.in_vault = false;
                self.in_mrs = false;
                self.in_audit = false;
                let f = self.news.read(cx).focus_handle();
                window.focus(&f, cx);
                if self.tour.is_none() && !tour::seen("news") {
                    self.tour = Some((tour::NEWS, "news", 0));
                }
                return cx.notify();
            }
            Cmd::NewsBrief => {
                if crate::news::has_token() {
                    self.brief = true;
                } else {
                    self.toast("Resumo do dia precisa do token do daily.dev".into(), cx);
                }
            }
            Cmd::NewFeature => return self.open_new_feature(window, cx),
            Cmd::RemoveFeature => return self.remove_feature(window, cx),
            Cmd::Compare => return self.open_compare(window, cx),
            Cmd::Review => return self.toggle_review(None, window, cx),
            Cmd::SendSelection => return self.send_selection(window, cx),
            Cmd::OpenFileMenu => return self.open_file_menu(window, cx),
            Cmd::OpenFile(p) => {
                if self.review.is_none() {
                    self.toggle_review(None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.open_file(p, window, cx));
                }
                return cx.notify();
            }
            Cmd::OpenMr => {
                if self.review.is_none() {
                    self.toggle_review(None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.open_mr(cx));
                }
                return cx.notify();
            }
            Cmd::Save => {
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| r.save(cx));
                }
                return;
            }
            Cmd::Affected | Cmd::SendReview => {
                if self.review.is_none() {
                    self.toggle_review(None, window, cx);
                }
                if let Some(r) = &self.review {
                    r.update(cx, |r, cx| match cmd {
                        Cmd::Affected => r.run_tests(cx),
                        _ => r.send(cx),
                    });
                }
                return;
            }
            Cmd::AddRepoMenu => {
                let wt = self
                    .sidebar
                    .as_ref()
                    .and_then(|s| s.read(cx).top_worktree());
                let Some(inv) = wt.as_deref().and_then(feature::find_by_path) else {
                    return self.toast(
                        "Abra uma feature criada pelo Maestro para adicionar um repo".into(),
                        cx,
                    );
                };
                let items = feature::sibling_repos(&inv.repo)
                    .into_iter()
                    .filter(|r| !inv.worktrees.iter().any(|w| w.repo == *r))
                    .map(|r| palette_cmd::Item {
                        title: r
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        key: String::new(),
                        cmd: Some(Cmd::AddRepo(r)),
                        tag: None,
                    })
                    .collect();
                return self.open_palette(
                    items,
                    "Repositório para adicionar à feature…",
                    "Nenhum repositório irmão para adicionar",
                    window,
                    cx,
                );
            }
            Cmd::AddRepo(repo) => {
                let wt = self
                    .sidebar
                    .as_ref()
                    .and_then(|s| s.read(cx).top_worktree());
                let (Some(mut inv), Some(herdr)) = (
                    wt.as_deref().and_then(feature::find_by_path),
                    self.sidebar.as_ref().map(|s| s.read(cx).herdr()),
                ) else {
                    return;
                };
                let name = repo
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                cx.spawn(async move |this, cx| {
                    let r = cx
                        .background_executor()
                        .spawn(async move { feature::add_repo(&herdr, &mut inv, &repo) })
                        .await;
                    let _ = this.update(cx, |this, cx| match r {
                        Ok(_) => this.toast(
                            format!("{name} entrou na feature; /add-dir enviado ao Claude"),
                            cx,
                        ),
                        Err(e) => this.toast(format!("Não deu para adicionar {name}: {e}"), cx),
                    });
                })
                .detach();
            }
            Cmd::Cleanup => {
                let repos = self
                    .sidebar
                    .as_ref()
                    .map(|s| s.read(cx).repo_paths())
                    .unwrap_or_default();
                let theme = self.theme.clone();
                let c = cx.new(|cx| Cleanup::new(repos, theme, window, cx));
                cx.subscribe_in(&c, window, |this, _, e: &cleanup::Event, window, cx| {
                    let cleanup::Event::Done(msg) = e;
                    this.cleanup = None;
                    if let Some(m) = msg.clone() {
                        this.toast(m, cx);
                    }
                    window.focus(&this.terminal.read(cx).focus_handle(), cx);
                    cx.notify();
                })
                .detach();
                self.cleanup = Some(c);
                cx.notify();
                return;
            }
        }
        window.focus(&self.terminal.read(cx).focus_handle(), cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn advance_tour(&mut self, cx: &mut Context<Self>) {
        match self.tour {
            Some((steps, id, i)) if i + 1 < steps.len() => self.tour = Some((steps, id, i + 1)),
            _ => return self.end_tour(cx),
        }
        cx.notify();
    }

    fn end_tour(&mut self, cx: &mut Context<Self>) {
        if let Some((_, id, _)) = self.tour.take() {
            tour::mark(id);
        }
        cx.notify();
    }

    /// Posições dos alvos relativas à raiz (a janela sem decoração tem margem de sombra).
    fn relative_anchors(&self, origin: Point<Pixels>) -> tour::Anchors {
        let map = self
            .anchors
            .borrow()
            .iter()
            .map(|(k, b)| (*k, Bounds::new(b.origin - origin, b.size)))
            .collect();
        std::rc::Rc::new(std::cell::RefCell::new(map))
    }

    fn kbd(&self, k: &str) -> Div {
        let t = &self.theme;
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
            .child(k.to_string())
    }

    /// Minimizar, maximizar e fechar à direita, como nas janelas do Linux (a decoração é nossa).
    fn win_btn(&self, id: &'static str, g: Glyph, action: fn(&mut Window)) -> impl IntoElement {
        let t = &self.theme;
        let close = matches!(g, Glyph::Close);
        let icon = match g {
            Glyph::Min => div().w(px(10.)).h(px(1.5)).bg(t.fg),
            Glyph::Max(false) => div().size(px(10.)).border_1().border_color(t.fg),
            Glyph::Max(true) => div()
                .size(px(10.))
                .relative()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .size(px(8.))
                        .border_1()
                        .border_color(t.muted),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .size(px(8.))
                        .border_1()
                        .border_color(t.fg)
                        .bg(t.panel),
                ),
            Glyph::Close => div().text_size(px(17.)).line_height(px(17.)).child("×"),
        };
        div()
            .id(id)
            .w(px(40.))
            .h(px(40.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.fg)
            .hover(move |d| {
                if close {
                    d.bg(rgb(0xc42b1c))
                } else {
                    d.bg(t.raise)
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, _| action(window))
            .child(icon)
    }

    fn title(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let (blocked, attention) = self.sidebar.as_ref().map_or((0, 0), |s| {
            let s = s.read(cx);
            let n = s
                .agents
                .iter()
                .filter(|x| x.agent_status == Status::Blocked)
                .count();
            (n, s.attention().len())
        });
        let mode = |label: &str, on: bool, count: Option<String>| {
            div()
                .flex()
                .gap(px(6.))
                .items_center()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(5.))
                .text_color(if on { t.fg } else { t.muted })
                .when(on, |d| d.bg(t.raise))
                .child(label.to_string())
                .when_some(count, |d, c| {
                    d.child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(t.orange)
                            .child(c),
                    )
                })
        };
        div()
            .h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.line)
            // decoração própria (a do protótipo): arrastar move, dois cliques maximizam
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count == 2 {
                    window.zoom_window();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                div()
                    .flex()
                    .gap(px(7.))
                    .items_center()
                    .child(img(logo()).size(px(18.)).flex_none())
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("maestro")),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .ml(px(8.))
                    .child(
                        mode(
                            "Features",
                            !self.in_vault && !self.in_mrs && !self.in_news && !self.in_db,
                            (blocked > 0).then(|| format!("{blocked} !")),
                        )
                        .id("m-feat")
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::Features, w, cx))),
                    )
                    .child(
                        mode(
                            "MRs",
                            self.in_mrs,
                            self.mrs
                                .as_ref()
                                .map(|m| m.read(cx).waiting())
                                .filter(|n| *n > 0)
                                .map(|n| n.to_string()),
                        )
                        .id("m-mrs")
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::Mrs, w, cx))),
                    )
                    .child({
                        let n = self.tunnels.read(cx).up();
                        div()
                            .id("m-db")
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .px(px(10.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .text_color(if self.in_db { t.fg } else { t.muted })
                            .when(self.in_db, |d| d.bg(t.raise))
                            .child("Bancos")
                            .when(n > 0, |d| {
                                d.child(
                                    div()
                                        .font_family(t.mono.clone())
                                        .text_size(px(12.))
                                        .text_color(t.ok)
                                        .child(n.to_string()),
                                )
                            })
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::Db, w, cx)))
                    })
                    .child(
                        mode("Vault", self.in_vault, None)
                            .id("m-vault")
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.run(Cmd::Vault(crate::palette_cmd::VaultGo::Open), w, cx)
                            })),
                    )
                    .child({
                        // sem token o modo existe, apagado, com o passo a passo dentro
                        let n = self.news.read(cx).count();
                        div()
                            .id("m-news")
                            .flex()
                            .gap(px(6.))
                            .items_center()
                            .px(px(10.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .text_color(if self.in_news { t.fg } else { t.muted })
                            .when(self.in_news, |d| d.bg(t.raise))
                            .when(!crate::news::has_token(), |d| d.opacity(0.45))
                            .child("News")
                            .when(n > 0, |d| {
                                d.child(
                                    div()
                                        .font_family(t.mono.clone())
                                        .text_size(px(12.))
                                        .text_color(t.cyan)
                                        .child(n.to_string()),
                                )
                            })
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::News, w, cx)))
                    }),
            )
            .child(
                div()
                    .id("fila")
                    .relative()
                    .child(tour::anchor(&self.anchors, "fila"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .flex()
                    .gap(px(6.))
                    .items_center()
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(t.line)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.raise))
                    .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::AttentionMenu, w, cx)))
                    .child("Fila")
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.orange)
                            .child(attention.to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .items_center()
                    .text_color(t.muted)
                    .child(
                        div()
                            .id("files")
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| this.open_file_menu(w, cx)))
                            .child(self.kbd("Ctrl+Shift+O"))
                            .child("arquivo"),
                    )
                    .child(
                        div()
                            .id("cmds")
                            .flex()
                            .gap(px(6.))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_palette(w, cx)))
                            .child(self.kbd("Ctrl+Shift+P"))
                            .child("comandos"),
                    )
                    .child(
                        div()
                            .id("help-btn")
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.help = true;
                                cx.notify();
                            }))
                            .child(self.kbd("?")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .ml(px(4.))
                    .child(self.win_btn("w-min", Glyph::Min, |w| w.minimize_window()))
                    .child(self.win_btn("w-max", Glyph::Max(self.maximized), |w| w.zoom_window()))
                    .child(self.win_btn("w-close", Glyph::Close, |w| w.remove_window())),
            )
    }

    /// As abas de trabalho, acima da lateral e do terminal (o `.wtabs` do protótipo); dentro de cada uma, as abas são do herdr.
    fn works_bar(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let mut bar = div()
            .flex()
            .flex_none()
            .items_end()
            .gap(px(2.))
            .h(px(32.))
            .px(px(8.))
            .bg(rgb(0x191a21))
            .border_b_1()
            .border_color(t.line)
            .text_size(px(13.))
            .whitespace_nowrap()
            .overflow_hidden();
        let Some(sidebar) = &self.sidebar else {
            return bar;
        };
        let s = sidebar.read(cx);
        for i in 0..s.works.len() {
            let on = i == s.cur;
            let (repo, branch) = s.work_label(i);
            let mark = s.work_status(i).map(|st| {
                div()
                    .text_color(self.status_color(st))
                    .font_weight(FontWeight::BOLD)
                    .child(crate::sidebar::glyph(st).0)
            });
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("work:{i}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .max_w(px(300.))
                    .rounded_t(px(6.))
                    .cursor_pointer()
                    .text_color(t.muted)
                    .when(on, |d| d.bg(t.panel).text_color(t.fg))
                    .when(!on, |d| d.hover(|d| d.bg(t.panel)))
                    .on_click(cx.listener(move |this, _, w, cx| this.run(Cmd::Work(i), w, cx)))
                    .children(mark)
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .min_w_0()
                            .overflow_hidden()
                            .child(div().text_color(t.fg).child(repo))
                            .children(branch.map(|b| {
                                div()
                                    .truncate()
                                    .font_family(t.mono.clone())
                                    .text_size(px(12.))
                                    .text_color(if on { t.accent } else { t.comment })
                                    .child(b)
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("workx:{i}")))
                            .px(px(3.))
                            .rounded(px(3.))
                            .text_color(t.comment)
                            .hover(|d| d.text_color(t.fg).bg(t.raise))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.run(Cmd::CloseWork(Some(i)), w, cx)
                            }))
                            .child("×"),
                    ),
            );
        }
        bar = bar.child(
            div()
                .id("work-new")
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(t.comment)
                .hover(|d| d.text_color(t.fg))
                .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::RepoMenu, w, cx)))
                .child("+"),
        );
        bar = bar.child(div().flex_1());
        let side = div()
            .h(px(28.))
            .px(px(10.))
            .flex()
            .items_center()
            .font_family(t.mono.clone())
            .text_size(px(12.));
        if self.prefix {
            bar = bar.child(
                side.bg(rgb(0xf1fa8c))
                    .text_color(rgb(0x282a36))
                    .font_weight(FontWeight::BOLD)
                    .child("PREFIX: v divide · - empilha · c aba · x fecha · z zoom"),
            );
        } else if s.top_worktree().is_some()
            && !self.in_mrs
            && !self.in_vault
            && !self.in_news
            && !self.in_db
        {
            bar = bar.child(
                side.id("run-task")
                    .cursor_pointer()
                    .text_color(t.comment)
                    .hover(|d| d.text_color(t.fg))
                    .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::TaskMenu, w, cx)))
                    .child("▶ Rodar tarefa…"),
            );
        }
        bar
    }

    fn status_color(&self, s: Status) -> Hsla {
        let t = &self.theme;
        match s {
            Status::Blocked => t.orange,
            Status::Working => t.cyan,
            Status::Done => t.ok,
            Status::Idle | Status::Unknown => t.comment,
        }
    }

    /// O Resumo do dia (`paintBrief` do protótipo): no canto, sem bloquear o terminal.
    fn brief_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        const MESES: [&str; 12] = [
            "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
        ];
        let today = crate::vault::today();
        let date = match (
            today.get(8..10),
            today.get(5..7).and_then(|m| m.parse::<usize>().ok()),
        ) {
            (Some(d), Some(m)) if (1..=12).contains(&m) => {
                format!("{} {}", d.trim_start_matches('0'), MESES[m - 1])
            }
            _ => today.clone(),
        };
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(12.))
            .py(px(8.))
            .text_size(px(12.5));
        for (i, (title, why)) in self.news.read(cx).brief().into_iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_color(t.muted)
                            .child(format!("{}.", i + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(title)
                            .child(div().text_size(px(11.5)).text_color(t.muted).child(why)),
                    ),
            );
        }
        div()
            .absolute()
            .top(px(46.))
            .right(px(14.))
            .w(px(400.))
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .shadow_lg()
            .flex()
            .flex_col()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Resumo do dia"),
                    )
                    .child(
                        div()
                            .text_color(t.muted)
                            .child(format!("{date} · daily.dev")),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("br-x")
                            .cursor_pointer()
                            .text_color(t.muted)
                            .hover(|d| d.text_color(t.fg))
                            .child("×")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.brief = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(t.muted)
                            .child("aparece na primeira abertura do dia"),
                    )
                    .child(div().flex_1())
                    .child(button(&t, "br-c", "Fechar", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.brief = false;
                            cx.notify();
                        },
                    )))
                    .child(
                        button(&t, "br-o", "Abrir News", true)
                            .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::News, w, cx))),
                    ),
            )
    }

    /// As dicas da tela atual (o `HINTS[S.mode]` do protótipo).
    fn screen_hints(&self, cx: &App) -> &'static [(&'static str, &'static str, bool)] {
        let mr_open = self.mrs.as_ref().is_some_and(|m| m.read(cx).in_mr());
        if self.in_db {
            DB_HINTS
        } else if self.in_news {
            NEWS_HINTS
        } else if self.in_audit {
            AUDIT_HINTS
        } else if self.in_vault {
            VAULT_HINTS
        } else if self.in_mrs && mr_open {
            MR_HINTS
        } else if self.in_mrs {
            MRS_HINTS
        } else {
            HINTS
        }
    }

    /// O `openHelp` do protótipo. "Experimente" traz os passos do app, não os dados de exemplo do protótipo.
    fn help_overlay(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let h4 = |s: &str| {
            div()
                .text_size(px(12.))
                .text_color(t.muted)
                .font_weight(FontWeight::MEDIUM)
                .child(s.to_uppercase())
        };
        let row = |w: f32, a: Div, b: Div| {
            div()
                .flex()
                .gap(px(16.))
                .py(px(4.))
                .border_b_1()
                .border_color(t.line)
                .child(a.w(px(w)).flex_none().whitespace_nowrap())
                .child(b.flex_1().min_w_0())
        };
        let mut steps = div().flex().flex_col().text_size(px(13.5));
        for (i, s) in HELP_STEPS.iter().enumerate() {
            steps = steps.child(row(10., div().child((i + 1).to_string()), div().child(*s)));
        }
        let mut keys = div().flex().flex_col().text_size(px(13.5));
        for (k, v, _) in self.screen_hints(cx).iter().chain(
            [
                ("Ctrl+Shift+O", "abrir arquivo", true),
                ("Ctrl+Shift+F", "árvore de arquivos", true),
                ("Alt+1…6", "Features, MRs, Bancos, Vault, News, Fila", true),
                ("Ctrl+Shift+P", "comandos", true),
                ("Ctrl+Shift+K", "comando por descrição", true),
            ]
            .iter(),
        ) {
            keys = keys.child(row(130., div().flex().child(self.kbd(k)), div().child(*v)));
        }
        let mut nielsen = div().flex().flex_col().text_size(px(13.5));
        for (i, (a, b)) in NIELSEN.iter().enumerate() {
            nielsen = nielsen.child(row(
                170.,
                div().child(format!("{}. {a}", i + 1)),
                div().child(*b),
            ));
        }
        let p = |s: &str| {
            div()
                .text_color(t.muted)
                .text_size(px(13.5))
                .child(s.to_string())
        };
        div()
            .id("help")
            .w(px(780.))
            .max_w_full()
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .bg(t.raise)
            .border_1()
            .border_color(t.line)
            .rounded(px(8.))
            .px(px(20.))
            .py(px(18.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Ajuda"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(18.))
                    .child(div().flex_1().min_w(px(260.)).flex().flex_col().gap(px(6.)).child(h4("Experimente")).child(steps))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(260.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(h4("Atalhos desta tela"))
                            .child(keys)
                            .child(p("Regra dos atalhos: com o foco no terminal, Ctrl+tecla vai para o terminal (Ctrl+A herdr, Ctrl+C, Ctrl+L, Ctrl+R, Ctrl+E de readline). Ctrl+Shift+tecla é do Maestro, como no seu Ghostty, e os binds de split do seu ghostty/config viram ações do herdr."))
                            .child(p("Visual: fonte, paleta, padding, opacidade e cursor vêm do ghostty/config; o prompt vem do starship.toml; ls/cat seguem seus aliases eza/bat.")),
                    ),
            )
            .child(h4("Heurísticas de Nielsen aplicadas"))
            .child(nielsen)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .justify_end()
                    .child(button(&t, "hT", "Rever tutorial desta tela", false).on_click(
                        cx.listener(|this, _, w, cx| {
                            this.help = false;
                            this.run(Cmd::Tutorial, w, cx);
                        }),
                    ))
                    .child(
                        button(&t, "hC", "Fechar", false)
                            .child(self.kbd("Esc"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.help = false;
                                cx.notify();
                            })),
                    ),
            )
    }

    fn hints(&self, cx: &App) -> Div {
        let t = &self.theme;
        let mut bar = div()
            .h(px(25.))
            .flex_none()
            .flex()
            .gap(px(14.))
            .items_center()
            .px(px(12.))
            .text_size(px(12.5))
            .text_color(t.muted)
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .overflow_hidden()
            .whitespace_nowrap();
        for (k, label, ready) in self.screen_hints(cx) {
            bar = bar.child(
                div()
                    .flex()
                    .gap(px(4.))
                    .items_center()
                    .when(!ready, |d| d.opacity(0.45))
                    .child(self.kbd(k))
                    .child(label.to_string()),
            );
        }
        bar
    }

    fn status(&self, cx: &mut Context<Self>) -> Div {
        let t = &self.theme;
        let cell = || {
            div()
                .h_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .border_r_1()
                .border_color(t.line)
        };
        let mut bar = div()
            .h(px(27.))
            .flex_none()
            .flex()
            .items_center()
            .text_size(px(13.))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.line)
            .overflow_hidden()
            .whitespace_nowrap();
        match &self.sidebar {
            None => bar = bar.child(cell().child("herdr: desligado")),
            Some(s) => {
                let s = s.read(cx);
                let blocked = s
                    .agents
                    .iter()
                    .filter(|a| a.agent_status == Status::Blocked)
                    .count();
                let working = s
                    .agents
                    .iter()
                    .filter(|a| a.agent_status == Status::Working)
                    .count();
                bar = bar.child(cell().child(if s.offline {
                    "herdr: fora do ar"
                } else {
                    "herdr: conectado"
                }));
                if blocked > 0 {
                    let sidebar = self.sidebar.clone().unwrap();
                    bar = bar.child(
                        cell()
                            .id("att")
                            .text_color(t.orange)
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                let s = sidebar.read(cx);
                                if let Some(p) = s.first_blocked() {
                                    s.focus(p);
                                }
                            })
                            .child(format!(
                                "! {blocked} precisa{} de você",
                                if blocked > 1 { "m" } else { "" }
                            )),
                    );
                }
                bar = bar.child(cell().child(format!("{working} trabalhando")));
                let (up, down) = {
                    let d = self.tunnels.read(cx);
                    (d.up(), d.down())
                };
                if up + down > 0 {
                    bar = bar.child(
                        cell()
                            .id("st-db")
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_color(if down > 0 { rgb(0xff5555).into() } else { t.ok })
                                    .child("●"),
                            )
                            .child(format!(
                                "{up} banco{}{}",
                                if up == 1 { "" } else { "s" },
                                if down > 0 {
                                    format!(" · {down} caiu")
                                } else {
                                    String::new()
                                }
                            ))
                            .on_click(cx.listener(|this, _, w, cx| this.run(Cmd::Db, w, cx))),
                    );
                }
                bar = bar.child(
                    cell().child(format!("hoje: US$ {}", crate::sidebar::brl(s.today_cost()))),
                );
                if let Some(a) = self
                    .audit
                    .as_ref()
                    .map(|a| a.read(cx))
                    .filter(|a| a.running())
                {
                    bar = bar.child(
                        cell().child(format!("auditoria: US$ {}", crate::audit::brl(a.spent()))),
                    );
                }
                let focused = s.agents.iter().find(|a| a.focused);
                if let Some(b) = focused.and_then(|a| {
                    herdr::repo_root(std::path::Path::new(&a.cwd)).and_then(herdr::branch)
                }) {
                    bar = bar.child(cell().child(format!("⎇ {b}")));
                }
            }
        }
        let cursor = self.review.as_ref().and_then(|r| r.read(cx).cursor(cx));
        let lsp = self.review.as_ref().and_then(|r| r.read(cx).lsp_status(cx));
        let red: Hsla = rgb(0xff5555).into();
        let yellow: Hsla = rgb(0xf1fa8c).into();
        bar.child(div().flex_1())
            .children(lsp.map(|(counts, label)| {
                let s = |n: usize| if n == 1 { "" } else { "s" };
                cell()
                    .gap(px(4.))
                    .when_some(counts, |d, (e, w)| {
                        d.child(
                            div()
                                .when(e > 0, |d| d.text_color(red))
                                .child(format!("{e} erro{}", s(e))),
                        )
                        .child("·")
                        .child(
                            div()
                                .when(w > 0, |d| d.text_color(yellow))
                                .child(format!("{w} aviso{}", s(w))),
                        )
                        .child(div().w(px(8.)))
                    })
                    .child(div().text_color(t.muted).child(label))
            }))
            .children(cursor.map(|(l, c)| cell().child(format!("Ln {l}, Col {c}"))))
            .child(cell().border_r_0().child(format!("ghostty: {}", self.font)))
    }
}

/// Bordas de redimensionar da janela sem decoração do sistema.
fn resize_edges() -> impl Iterator<Item = Div> {
    use ResizeEdge::*;
    const E: f32 = 5.;
    [
        Top,
        Bottom,
        Left,
        Right,
        TopLeft,
        TopRight,
        BottomLeft,
        BottomRight,
    ]
    .into_iter()
    .map(|edge| {
        let d = div().absolute();
        let d = match edge {
            Top => d
                .top_0()
                .left(px(E))
                .right(px(E))
                .h(px(E))
                .cursor_ns_resize(),
            Bottom => d
                .bottom_0()
                .left(px(E))
                .right(px(E))
                .h(px(E))
                .cursor_ns_resize(),
            Left => d
                .left_0()
                .top(px(E))
                .bottom(px(E))
                .w(px(E))
                .cursor_ew_resize(),
            Right => d
                .right_0()
                .top(px(E))
                .bottom(px(E))
                .w(px(E))
                .cursor_ew_resize(),
            TopLeft => d.top_0().left_0().size(px(E)).cursor_nwse_resize(),
            BottomRight => d.bottom_0().right_0().size(px(E)).cursor_nwse_resize(),
            TopRight => d.top_0().right_0().size(px(E)).cursor_nesw_resize(),
            BottomLeft => d.bottom_0().left_0().size(px(E)).cursor_nesw_resize(),
        };
        d.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.start_window_resize(edge);
        })
    })
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        self.window_active = window.is_window_active();
        self.maximized = window.is_maximized();
        if std::mem::take(&mut self.want_dialog) {
            self.open_new_feature(window, cx);
        }
        if self.review.is_none()
            && let Some(s) = &self.sidebar
        {
            s.update(cx, |s, cx| s.set_open(None, cx));
        }
        if self.confirm.is_none()
            && self.choice.is_none()
            && self.dialog.is_none()
            && !self.learn.is_empty()
        {
            let s = self.learn.remove(0);
            self.ask_learn(s, window, cx);
        }
        // o polling da inbox roda desde o início: é ele que avisa de MR novo e de resposta
        if self.mrs.is_none() {
            self.mrs_view(window, cx);
        }
        if self.merge_dlg.is_none()
            && let Some((title, text, opts, running)) = self.want_merge.take()
        {
            let theme = self.theme.clone();
            let d =
                cx.new(|cx| confirm::MergeDlg::new(theme, title, text, opts, running, window, cx));
            cx.subscribe_in(&d, window, |this, _, a: &confirm::Merged, window, cx| {
                this.merge_dlg = None;
                if let Some(m) = &this.mrs {
                    if let Some(o) = a.0.clone() {
                        m.update(cx, |m, cx| m.merge(o, cx));
                    }
                    let f = m.read(cx).focus_handle();
                    window.focus(&f, cx);
                }
                cx.notify();
            })
            .detach();
            self.merge_dlg = Some(d);
        }
        if self.confirm.is_none()
            && let Some((title, text)) = self.want_publish.take()
        {
            let theme = self.theme.clone();
            let c = cx.new(|cx| Confirm::new(theme, title, text, "Publicar", false, window, cx));
            cx.subscribe_in(&c, window, |this, _, a: &Answer, window, cx| {
                this.confirm = None;
                if matches!(a, Answer::Ok)
                    && let Some(m) = &this.mrs
                {
                    m.update(cx, |m, cx| m.publish(cx));
                    let f = m.read(cx).focus_handle();
                    window.focus(&f, cx);
                }
                cx.notify();
            })
            .detach();
            self.confirm = Some(c);
        }
        if self.confirm.is_none()
            && let Some((wt, iid, branch)) = self.want_merged.take()
        {
            let title = format!("!{iid} foi mergeado. Remover a worktree {branch}?");
            self.remove_feature_at(Some(wt), Some(title), window, cx);
        }
        // o card da lateral lê só o relatório salvo; abrir o projeto nunca roda auditoria
        if self
            .sidebar
            .as_ref()
            .is_some_and(|s| s.read(cx).top_repo().is_some())
        {
            let a = self.audit_view(cx).read(cx);
            let (run, k, m) = a.card();
            let v = (run, k, m, self.in_audit, a.ready());
            if let Some(s) = &self.sidebar {
                s.update(cx, |s, cx| {
                    if s.audit != v {
                        s.audit = v;
                        cx.notify();
                    }
                });
            }
        }
        if std::mem::take(&mut self.want_audit) {
            self.run(Cmd::Audit(false), window, cx);
        }
        if self.confirm.is_none()
            && let Some((title, text)) = self.want_create.take()
        {
            let theme = self.theme.clone();
            let c = cx
                .new(|cx| Confirm::new(theme, title, text, "Abrir work items", false, window, cx));
            cx.subscribe_in(&c, window, |this, _, a: &Answer, window, cx| {
                this.confirm = None;
                if let Some(au) = &this.audit {
                    if matches!(a, Answer::Ok) {
                        au.update(cx, |au, cx| au.create(cx));
                    }
                    let f = au.read(cx).focus_handle();
                    window.focus(&f, cx);
                }
                cx.notify();
            })
            .detach();
            self.confirm = Some(c);
        }
        if self.confirm.is_none()
            && let Some((title, text, branch, prompt)) = self.want_fix.take()
        {
            let theme = self.theme.clone();
            let c =
                cx.new(|cx| Confirm::new(theme, title, text, "Criar feature", false, window, cx));
            cx.subscribe_in(&c, window, move |this, _, a: &Answer, window, cx| {
                this.confirm = None;
                if matches!(a, Answer::Ok) {
                    this.fix_in_feature(branch.clone(), prompt.clone(), cx);
                    window.focus(&this.terminal.read(cx).focus_handle(), cx);
                }
                cx.notify();
            })
            .detach();
            self.confirm = Some(c);
        }
        if let Some(p) = self.want_file.take() {
            self.run(Cmd::OpenFile(p), window, cx);
        }
        if let Some(op) = self.want_fs.take() {
            self.file_op(op, window, cx);
        }
        if let Some(p) = self.want_review.take() {
            self.toggle_review(p, window, cx);
        }
        if let Some(p) = self.want_review_at.take() {
            self.leave_modes();
            self.open_review_at(p, None, window, cx);
        }
        if self.confirm.is_none()
            && let Some((wt, st)) = self.want_stack.take()
        {
            self.stack_dialog(wt, st, window, cx);
        }
        let overlay = self.tour.and_then(|(steps, _, i)| {
            let root = self.anchors.borrow().get("root").copied()?;
            let next = cx.listener(|this: &mut Self, _: &(), _, cx| this.advance_tour(cx));
            let skip = cx.listener(|this: &mut Self, _: &(), _, cx| this.end_tour(cx));
            let o = tour::overlay(
                steps,
                i,
                &self.relative_anchors(root.origin),
                root.size,
                &t,
                move |w, cx| next(&(), w, cx),
                move |w, cx| skip(&(), w, cx),
            );
            if o.is_none() {
                // o alvo só tem posição depois do primeiro frame
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
            o
        });
        div()
            .relative()
            .child(tour::anchor(&self.anchors, "root"))
            .on_action(cx.listener(|this, _: &editor::LeaveEditor, window, cx| {
                window.focus(&this.terminal.read(cx).focus_handle(), cx);
                cx.notify();
            }))
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if this.focus_arrow(&e.keystroke, window, cx) {
                    return cx.stop_propagation();
                }
                if e.keystroke.key != "escape" {
                    return;
                }
                // a ordem do Esc do protótipo: tutorial, diálogos, e só então o mascote
                let overlay = this.palette.is_some()
                    || this.dialog.is_some()
                    || this.confirm.is_some()
                    || this.cleanup.is_some()
                    || this.compare.is_some()
                    || this.choice.is_some()
                    || this
                        .review
                        .as_ref()
                        .is_some_and(|r| r.read(cx).editing_open());
                if this.help {
                    this.help = false;
                    cx.notify();
                } else if this.tour.is_some() {
                    this.end_tour(cx);
                } else if !overlay && this.mascot.as_ref().is_some_and(|m| !m.2) {
                    this.dismiss_mascot(cx);
                } else {
                    return;
                }
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .text_color(t.fg)
            .font_family(t.sans)
            .text_size(px(14.))
            .child(self.title(cx))
            .child(self.works_bar(cx))
            .child(
                div()
                    .id("columns")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .on_drag_move(cx.listener(|this, e: &DragMoveEvent<Resize>, _, cx| {
                        let b = e.bounds;
                        let x = e.event.position.x;
                        match e.drag(cx) {
                            Resize::Left => {
                                // o terminal nunca fica com menos de 300 px
                                let right = this.right_w.unwrap_or(0.);
                                let max = (f32::from(b.size.width) * 0.45)
                                    .min(f32::from(b.size.width) - right - 300.)
                                    .max(180.);
                                this.left_w = f32::from(x - b.left()).clamp(180., max)
                            }
                            Resize::Right => {
                                this.right_w = Some(f32::from(b.right() - x).clamp(
                                    320.,
                                    (f32::from(b.size.width) - this.left_w - 300.).max(320.),
                                ))
                            }
                        }
                        cx.notify();
                    }))
                    .when_some(self.vault.clone().filter(|_| self.in_vault), |d, v| {
                        d.child(v)
                    })
                    .when_some(self.mrs.clone().filter(|_| self.in_mrs), |d, m| d.child(m))
                    .children(
                        self.sidebar
                            .clone()
                            .filter(|_| {
                                self.left
                                    && !self.in_vault
                                    && !self.in_mrs
                                    && !self.in_news
                                    && !self.in_db
                            })
                            .map(|s| {
                                div()
                                    .flex()
                                    .flex_none()
                                    .h_full()
                                    .child(div().w(px(self.left_w)).h_full().child(s))
                                    .child(self.resize_handle(Resize::Left))
                            }),
                    )
                    .when_some(self.audit.clone().filter(|_| self.in_audit), |d, a| {
                        d.child(a)
                    })
                    .when_some(self.in_news.then(|| self.news.clone()), |d, n| d.child(n))
                    .when_some(self.in_db.then(|| self.tunnels.clone()), |d, n| d.child(n))
                    .when(
                        !self.in_vault
                            && !self.in_mrs
                            && !self.in_audit
                            && !self.in_news
                            && !self.in_db,
                        |d| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .bg(t.win)
                                    .relative()
                                    .child(tour::anchor(&self.anchors, "term"))
                                    .child(div().flex_1().min_h_0().child(self.terminal.clone())),
                            )
                        },
                    )
                    .children(
                        self.review
                            .clone()
                            .filter(|_| {
                                !self.in_vault
                                    && !self.in_mrs
                                    && !self.in_audit
                                    && !self.in_news
                                    && !self.in_db
                            })
                            .map(|r| {
                                div()
                                    .map(|d| match self.right_w {
                                        Some(w) => d.w(px(w)),
                                        None => d.w(relative(0.46)).max_w(px(640.)),
                                    })
                                    .flex()
                                    .flex_none()
                                    .h_full()
                                    .child(self.resize_handle(Resize::Right))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .h_full()
                                            .border_l_1()
                                            .border_color(t.line)
                                            .child(r),
                                    )
                            }),
                    )
                    .children(self.ask.clone()),
            )
            .child(self.hints(cx))
            .child(self.status(cx))
            .children(overlay)
            .children(self.kcmd.clone().map(|k| {
                // o `.kpop` do protótipo: acima do terminal, sem escurecer a tela
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(76.))
                    .px(px(14.))
                    .flex()
                    .justify_center()
                    .child(k)
            }))
            .when(self.help, |d| {
                d.child(
                    div()
                        .id("help-scrim")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .justify_center()
                        .items_center()
                        .px(px(16.))
                        .bg(rgba(0x0a0a1080))
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.help = false;
                            cx.notify();
                        }))
                        .child(
                            self.help_overlay(cx)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                        ),
                )
            })
            .children(self.compare.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(24.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .children(self.cleanup.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .children(self.choice.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    .occlude()
                    .child(d)
            }))
            .children(self.merge_dlg.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    .occlude()
                    .child(d)
            }))
            .children(self.confirm.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .when_some(self.mascot.clone(), |d, (n, seq, leaving)| {
                let pane = n.pane.clone();
                d.child(
                    div()
                        .id("mascot")
                        .absolute()
                        .right(px(22.))
                        .bottom(px(58.))
                        .occlude()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = &this.sidebar {
                                s.read(cx).focus(pane.clone());
                            }
                            this.dismiss_mascot(cx);
                        }))
                        .child(mascot::view(
                            &t,
                            &n,
                            self.mq.len(),
                            self.mascot_frame,
                            seq,
                            leaving,
                        )),
                )
            })
            .when(self.brief && self.tour.is_none() && !self.help, |d| {
                d.child(self.brief_panel(cx))
            })
            .when(!self.toasts.is_empty(), |d| {
                let mut col = div()
                    .absolute()
                    .bottom(px(56.))
                    .left_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.));
                for (id, m, redo) in &self.toasts {
                    let mut toast = confirm::toast(&t, m).flex().gap(px(12.)).items_center();
                    if let Some((_, key)) = self.toast_open.iter().find(|(i, _)| i == id) {
                        let (id, key) = (*id, key.clone());
                        toast = toast.child(
                            div()
                                .id(("open", id))
                                .underline()
                                .font_weight(FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.toasts.retain(|(i, _, _)| *i != id);
                                    this.open_mr(&key, window, cx);
                                    cx.notify();
                                }))
                                .child("Abrir"),
                        );
                    }
                    if let Some(redo) = redo.clone() {
                        let id = *id;
                        toast = toast.child(
                            div()
                                .id(("undo", id))
                                .underline()
                                .font_weight(FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toasts.retain(|(i, _, _)| *i != id);
                                    let r = match &redo {
                                        Redo::Patch(root, patch) => {
                                            crate::diff::apply(root, patch, false)
                                        }
                                        Redo::Restore(root, sha) => feature::restore(root, sha),
                                        Redo::Vault(w) => crate::vault::undo(w),
                                        Redo::MrDraft => {
                                            if let Some(m) = &this.mrs {
                                                m.update(cx, |m, cx| m.undo(cx));
                                            }
                                            Ok(())
                                        }
                                        Redo::News(rule) => {
                                            this.news.update(cx, |n, cx| n.unhide(rule, cx));
                                            Ok(())
                                        }
                                        Redo::Audit(ig) => {
                                            if let Some(a) = &this.audit {
                                                a.update(cx, |a, cx| a.unignore(ig, cx));
                                            }
                                            Ok(())
                                        }
                                    };
                                    if let Err(e) = r {
                                        this.toast(format!("Não deu para refazer: {e}"), cx);
                                    }
                                    cx.notify();
                                }))
                                .child("Desfazer"),
                        );
                    }
                    col = col.child(toast);
                }
                d.child(col)
            })
            .children(self.dialog.clone().map(|d| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .child(d)
            }))
            .children(self.palette.clone().map(|p| {
                // o `.scrim` do protótipo: clique fora fecha
                div()
                    .id("scrim")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(60.))
                    .px(px(16.))
                    .bg(rgba(0x0a0a1080))
                    // sem isto o clique atravessa e o terminal embaixo toma o foco
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, w, cx| this.toggle_palette(w, cx)),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(p),
                    )
            }))
            // por último, para ficar por cima do conteúdo
            .children(resize_edges())
    }
}
