//! A tela PRCs (`renderPrc`/`renderPrcLeft` do protótipo): o board de chamados do Jira.

use std::collections::HashSet;

use gpui_kit::{prelude::FluentBuilder, *};

use crate::{
    form::button,
    jira::{self, Board, PrcCard},
    prc::{self, Config},
    theme::{self, Theme},
    vault,
};

pub enum Event {
    Toast(String),
    /// o card mudou de status no Jira: o toast oferece desfazer
    Moved {
        key: String,
        from: String,
        to: String,
    },
}

#[derive(Clone)]
pub struct DragCard(String);

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

#[derive(Clone, Copy)]
struct LeftDrag;

pub struct PrcView {
    cfg: Result<Option<Config>, String>,
    board: Option<Board>,
    loading: bool,
    err: Option<String>,
    upd: Option<String>,
    col: usize,
    sel: usize,
    moving: HashSet<String>,
    left_w: f32,
    focus: FocusHandle,
    theme: Theme,
}

impl EventEmitter<Event> for PrcView {}

impl PrcView {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            cfg: prc::load(),
            board: None,
            loading: false,
            err: None,
            upd: None,
            col: 0,
            sel: 0,
            moving: HashSet::new(),
            left_w: 250.,
            focus: cx.focus_handle(),
            theme,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Na primeira entrada na tela; depois só pelo "Atualizar".
    pub fn ensure(&mut self, cx: &mut Context<Self>) {
        if self.board.is_none() && self.err.is_none() {
            self.refresh(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.cfg = prc::load();
        let Ok(Some(cfg)) = self.cfg.clone() else {
            return cx.notify();
        };
        self.loading = true;
        self.err = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let b = jira::board(cfg.board, cfg.acr_link.as_deref());
                    (b, vault_hhmm())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match r.0 {
                    Ok(b) => {
                        this.board = Some(b);
                        this.upd = Some(r.1);
                        this.clamp();
                    }
                    Err(e) => this.err = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Os cards da primeira coluna aberta: o que espera triagem, para o título.
    pub fn waiting(&self) -> usize {
        let Some(b) = &self.board else { return 0 };
        prc::columns(b)
            .into_iter()
            .find(|(c, _)| !c.done)
            .map_or(0, |(_, xs)| xs.len())
    }

    fn open_cols(&self) -> Vec<usize> {
        self.board
            .as_ref()
            .map(|b| (0..b.cols.len()).filter(|&i| !b.cols[i].done).collect())
            .unwrap_or_default()
    }

    fn cards_in(&self, col: usize) -> Vec<&PrcCard> {
        let Some(b) = &self.board else {
            return Vec::new();
        };
        prc::columns(b)
            .into_iter()
            .nth(col)
            .map(|(_, xs)| xs)
            .unwrap_or_default()
    }

    fn clamp(&mut self) {
        let open = self.open_cols();
        if !open.contains(&self.col) {
            self.col = open.first().copied().unwrap_or(0);
        }
        self.sel = self
            .sel
            .min(self.cards_in(self.col).len().saturating_sub(1));
    }

    fn current(&self) -> Option<&PrcCard> {
        self.cards_in(self.col).get(self.sel).copied()
    }

    fn browse(&self, key: &str) {
        if let Some(b) = &self.board {
            crate::spawn_reaped(
                std::process::Command::new("xdg-open").arg(format!("{}/browse/{key}", b.site)),
            );
        }
    }

    /// Soltar o card numa coluna: leva a issue ao primeiro status dela no Jira.
    fn drop_on(&mut self, key: &str, col: usize, cx: &mut Context<Self>) {
        let Some(b) = &self.board else { return };
        let Some(to) = b
            .cols
            .get(col)
            .and_then(|c| prc::target(c))
            .map(String::from)
        else {
            return;
        };
        let Some(card) = b.cards.iter().find(|c| c.key == key) else {
            return;
        };
        if b.cols[col].statuses.contains(&card.status) {
            return;
        }
        let from = card.status.clone();
        self.move_to(key, &from, &to, cx);
    }

    /// Também é o desfazer: leva de volta ao status anterior.
    pub fn move_to(&mut self, key: &str, from: &str, to: &str, cx: &mut Context<Self>) {
        if !self.moving.insert(key.to_string()) {
            return;
        }
        cx.emit(Event::Toast(format!("Movendo {key} para {to} no Jira…")));
        cx.notify();
        let (k, f, t) = (key.to_string(), from.to_string(), to.to_string());
        cx.spawn(async move |this, cx| {
            let (k2, t2) = (k.clone(), t.clone());
            let r = cx
                .background_executor()
                .spawn(async move { jira::transition(&k2, &t2) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.moving.remove(&k);
                match r {
                    Ok(()) => {
                        if let Some(b) = &mut this.board {
                            if let Some(c) = b.cards.iter_mut().find(|c| c.key == k) {
                                c.status = t.clone();
                            }
                            // concluído sai do board: só os abertos são carregados
                            let done: Vec<String> = b
                                .cols
                                .iter()
                                .filter(|c| c.done)
                                .flat_map(|c| c.statuses.clone())
                                .collect();
                            b.cards.retain(|c| !done.contains(&c.status));
                        }
                        this.clamp();
                        cx.emit(Event::Moved {
                            key: k,
                            from: f,
                            to: t,
                        });
                    }
                    Err(e) => cx.emit(Event::Toast(format!("{k}: {e}. O card fica onde estava."))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.alt || !self.focus.is_focused(window) {
            return;
        }
        let open = self.open_cols();
        let pos = open.iter().position(|&c| c == self.col).unwrap_or(0);
        match k.key.as_str() {
            "h" | "left" if pos > 0 => {
                self.col = open[pos - 1];
                self.sel = 0;
            }
            "l" | "right" if pos + 1 < open.len() => {
                self.col = open[pos + 1];
                self.sel = 0;
            }
            "j" | "down" => {
                self.sel = (self.sel + 1).min(self.cards_in(self.col).len().saturating_sub(1))
            }
            "k" | "up" => self.sel = self.sel.saturating_sub(1),
            "o" => {
                if let Some(c) = self.current() {
                    self.browse(&c.key.clone());
                }
            }
            "r" => self.refresh(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn note(&self, text: impl Into<SharedString>) -> Div {
        div()
            .px(px(12.))
            .py(px(6.))
            .text_size(px(12.))
            .text_color(self.theme.muted)
            .child(text.into())
    }

    fn left(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(6.))
            .pt(px(2.))
            .pb(px(6.));
        rows = rows.child(
            div()
                .flex()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .opacity(0.45)
                .child(div().w(px(16.)).text_color(t.comment).child("·"))
                .child(div().flex_1().text_color(t.cyan).child("Análise (IA)")),
        );
        if let Some(b) = &self.board {
            for (i, (c, xs)) in prc::columns(b).into_iter().enumerate() {
                let on = i == self.col && !c.done;
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("pcol{i}")))
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .when(c.done, |d| d.opacity(0.45))
                        .when(on, |d| d.bg(t.accent_soft))
                        .when(!c.done, |d| {
                            d.cursor_pointer()
                                .when(!on, |d| d.hover(|d| d.bg(t.raise)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.col = i;
                                    this.sel = 0;
                                    cx.notify();
                                }))
                        })
                        .child(div().w(px(16.)).text_color(t.comment).child(if c.done {
                            "✓"
                        } else {
                            "·"
                        }))
                        .child(div().flex_1().child(c.name.clone()))
                        .when(!c.done, |d| {
                            d.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.muted)
                                    .child(xs.len().to_string()),
                            )
                        }),
                );
            }
        }
        let notes = prc::notes(&vault::dir());
        let mut kb = div().flex().flex_col().py(px(4.));
        for n in &notes {
            let uses = self.board.as_ref().map_or(0, |b| {
                b.cards.iter().filter(|c| c.labels.contains(n)).count()
            });
            kb = kb.child(
                div()
                    .flex()
                    .px(px(12.))
                    .py(px(3.))
                    .font_family(t.mono.clone())
                    .text_size(px(12.))
                    .child(div().flex_1().child(format!("{n}.md")))
                    .when(uses > 0, |d| {
                        d.child(div().text_color(t.comment).child(uses.to_string()))
                    }),
            );
        }
        div()
            .id("prc-left")
            .w(px(self.left_w))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.line)
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .items_baseline()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(div().font_weight(FontWeight::MEDIUM).child("PRCs"))
                    .child(
                        div()
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(match &self.cfg {
                                Ok(Some(c)) => format!("board {}", c.board),
                                _ => "sem config".into(),
                            }),
                    ),
            )
            .child(rows)
            .child(
                div()
                    .flex()
                    .px(px(10.))
                    .py(px(6.))
                    .border_t_1()
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child(div().flex_1().child("BASE DE CONHECIMENTO"))
                    .child(div().font_family(t.mono.clone()).child("vault/prcs/")),
            )
            .child(kb)
            .when(notes.is_empty(), |d| {
                d.child(self.note("Nenhuma receita ainda. Peça para registrar um procedimento de PRC (skill prc-base)."))
            })
            .child(self.note(
                "Uma nota por tipo de PRC, com o nome da label do card. A IA recebe a receita do card e, no fim de cada sessão, propõe atualizar a nota (com sua aprovação).",
            ))
    }

    fn card(&self, c: &PrcCard, on: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let vault = vault::dir();
        let rc = prc::recipes(&c.labels, &vault);
        let moving = self.moving.contains(&c.key);
        let key = c.key.clone();
        let (theme, chip) = (t.clone(), c.key.clone());
        div()
            .id(SharedString::from(format!("pc:{}", c.key)))
            .flex()
            .flex_col()
            .gap(px(5.))
            .p(px(8.))
            .bg(t.raise)
            .border_1()
            .border_color(if on { t.accent } else { t.line })
            .rounded(px(6.))
            .cursor_pointer()
            .when(moving, |d| d.opacity(0.5))
            .on_drag(DragCard(c.key.clone()), move |_: &DragCard, _, _, cx| {
                let theme = theme.clone();
                let chip = chip.clone();
                cx.new(|_| DragChip(chip, theme))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(b) = &this.board
                    && let Some(i) = prc::columns(b)
                        .iter()
                        .position(|(_, xs)| xs.iter().any(|x| x.key == key))
                {
                    let sel = prc::columns(b)[i].1.iter().position(|x| x.key == key);
                    this.col = i;
                    this.sel = sel.unwrap_or(0);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .font_family(t.mono.clone())
                    .text_size(px(11.5))
                    .text_color(t.comment)
                    .child(div().flex_1().child(c.key.clone()))
                    .when(moving, |d| {
                        d.child(div().text_color(t.muted).child("movendo…"))
                    })
                    .when(!rc.is_empty() && !moving, |d| {
                        d.child(div().text_color(t.ok).child("◆ receita"))
                    }),
            )
            .child(div().text_size(px(13.)).child(c.title.clone()))
            .when(!c.labels.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(c.labels.iter().map(|l| {
                            div()
                                .px(px(6.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(t.line)
                                .font_family(t.mono.clone())
                                .text_size(px(10.5))
                                .text_color(t.muted)
                                .child(l.clone())
                        })),
                )
            })
            .when_some(c.acr.clone(), |d, a| {
                d.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .font_family(t.mono.clone())
                        .text_size(px(11.5))
                        .text_color(t.accent)
                        .child(a.key)
                        .child(
                            div()
                                .text_color(t.muted)
                                .child(format!("· ACR · {}", a.status)),
                        ),
                )
            })
            .when(on, |d| {
                let k = c.key.clone();
                d.child(
                    div().flex().gap(px(4.)).pt(px(2.)).child(
                        button(&t, "pc-jira", "Jira ↗", false)
                            .on_click(cx.listener(move |this, _, _, _| this.browse(&k))),
                    ),
                )
            })
    }

    fn column(&self, i: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let Some(b) = &self.board else {
            return div().id("pcol-none");
        };
        let c = &b.cols[i];
        let xs = self.cards_in(i);
        let soft = t.accent_soft;
        let mut body = div()
            .id(SharedString::from(format!("pcards{i}")))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(6.))
            .overflow_y_scroll();
        if c.done {
            body = body.child(self.dashed("solte aqui para concluir no Jira"));
        } else if xs.is_empty() {
            body = body.child(
                div()
                    .p(px(4.))
                    .text_size(px(12.))
                    .text_color(t.comment)
                    .child("vazio"),
            );
        }
        let cards: Vec<PrcCard> = xs.into_iter().cloned().collect();
        for (k, x) in cards.iter().enumerate() {
            body = body.child(self.card(x, i == self.col && k == self.sel, cx));
        }
        div()
            .id(SharedString::from(format!("pcol-{i}")))
            .flex()
            .flex_col()
            .flex_none()
            .w(px(if c.done { 170. } else { 250. }))
            .max_h_full()
            .bg(t.panel)
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .drag_over::<DragCard>(move |s, _, _, _| s.bg(soft))
            .on_drop(
                cx.listener(move |this, d: &DragCard, _, cx| this.drop_on(&d.0.clone(), i, cx)),
            )
            .child(
                div()
                    .flex()
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(11.))
                    .text_color(if i == self.col { t.fg } else { t.muted })
                    .child(div().flex_1().child(c.name.to_uppercase()))
                    .when(!c.done, |d| {
                        d.child(
                            div()
                                .font_family(t.mono.clone())
                                .child(self.cards_in(i).len().to_string()),
                        )
                    }),
            )
            .child(body)
    }

    fn dashed(&self, text: &'static str) -> Div {
        let t = &self.theme;
        div()
            .p(px(14.))
            .border_1()
            .border_dashed()
            .border_color(t.line)
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(t.comment)
            .text_center()
            .child(text)
    }

    fn center(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let base = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        match &self.cfg {
            Ok(Some(_)) => {}
            Ok(None) => {
                return base.items_center().justify_center().child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .max_w(px(420.))
                        .items_center()
                        .text_color(t.muted)
                        .child(div().text_color(t.fg).font_weight(FontWeight::MEDIUM).child("Nenhum board configurado"))
                        .child(format!(
                            "Crie {} com o id do board de PRCs no Jira e a pasta dos seus repos. Fica fora do repo do Maestro.",
                            prc::config_path().display()
                        ))
                        .child(
                            div()
                                .p(px(10.))
                                .bg(t.panel)
                                .rounded(px(6.))
                                .font_family(t.mono.clone())
                                .text_color(t.fg)
                                .child(prc::EXAMPLE),
                        )
                        .child("Depois, r ou \"PRCs: atualizar\"."),
                );
            }
            Err(e) => return base.p(px(24.)).text_color(rgb(0xff5555)).child(e.clone()),
        }
        let total = self.board.as_ref().map_or(0, |b| b.cards.len());
        let status: SharedString = match (&self.err, self.loading, &self.upd) {
            (Some(e), _, Some(u)) => {
                format!("Falha ao carregar: {e} · mostrando a carga das {u}").into()
            }
            (Some(e), _, None) => format!("Falha ao carregar: {e}").into(),
            (None, true, _) => "atualizando…".into(),
            (None, false, Some(u)) => format!("atualizado às {u}").into(),
            _ => "".into(),
        };
        let bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(t.line)
            .text_size(px(12.))
            .child(
                div()
                    .text_color(t.muted)
                    .child(format!("{total} abertos · só os não concluídos")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_color(if self.err.is_some() {
                        rgb(0xff5555).into()
                    } else {
                        t.muted
                    })
                    .child(status),
            )
            .child(
                button(&t, "prc-up", "↻ Atualizar", false)
                    .when(self.loading, |d| d.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            );
        let mut kb = div()
            .id("prc-board")
            .flex_1()
            .min_h_0()
            .flex()
            .items_start()
            .gap(px(10.))
            .p(px(12.))
            .overflow_x_scroll();
        // ponytail: a coluna da IA aparece apagada até a etapa da análise entrar
        kb = kb.child(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(px(250.))
                .opacity(0.45)
                .border_1()
                .border_dashed()
                .border_color(t.line)
                .rounded(px(6.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(10.))
                        .py(px(8.))
                        .text_size(px(11.))
                        .child(div().text_color(t.cyan).child("● ANÁLISE (IA)"))
                        .child(
                            div()
                                .text_color(t.muted)
                                .child("diagnóstico, contingência sem código e ACR se precisar"),
                        ),
                )
                .child(div().p(px(6.)).child(self.dashed("arraste um card aqui"))),
        );
        if self.board.is_none() && self.loading {
            kb = kb.child(
                div()
                    .p(px(12.))
                    .text_color(t.muted)
                    .child("Lendo o board no Jira…"),
            );
        }
        let n = self.board.as_ref().map_or(0, |b| b.cols.len());
        for i in 0..n {
            kb = kb.child(self.column(i, cx));
        }
        base.child(bar).child(kb)
    }
}

fn vault_hhmm() -> String {
    std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

impl Render for PrcView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
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
            .child(theme::resize_handle("rz-prc", LeftDrag, &t))
            .child(self.center(cx))
    }
}
