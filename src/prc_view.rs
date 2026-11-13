//! A tela PRCs (`renderPrc`/`renderPrcLeft` do protótipo): o board de chamados do Jira,
//! a coluna Análise (IA) e o detalhe do card com a sessão do Claude espelhada.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use gpui_kit::{prelude::FluentBuilder, *};
use serde_json::json;

use crate::{
    form::{FieldEvent, TextField, button},
    herdr::{Herdr, Status},
    jira::{self, Board, PrcCard},
    prc::{self, Analysis, Config},
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

/// A aba `<KEY> · análise` no herdr e o que a análise já gravou.
#[derive(Clone, Default)]
struct Session {
    ws: String,
    pane: Option<String>,
    status: Option<Status>,
    result: Option<Result<Analysis, String>>,
    /// a contingência já foi pedida (arquivo `exec` na pasta da sessão: sobrevive a reabrir o Maestro)
    running: bool,
}

/// O que o card em análise oferece, conforme o resultado.
#[derive(Debug, PartialEq)]
pub struct Actions {
    pub exec: bool,
    pub acr: bool,
}

pub fn actions(a: Option<&Analysis>, linked_acr: bool) -> Actions {
    let Some(a) = a else {
        return Actions {
            exec: false,
            acr: false,
        };
    };
    Actions {
        exec: a.cont.as_ref().is_some_and(|c| a.done < c.len()),
        acr: a.acr.is_some() && !linked_acr,
    }
}

/// Estado de cada passo: feito, pedindo aprovação (o agente está bloqueado nele) ou pendente.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Step {
    Done,
    Asking,
    Pending,
}

pub fn steps(a: &Analysis, running: bool, blocked: bool) -> Vec<Step> {
    let n = a.cont.as_ref().map_or(0, Vec::len);
    (0..n)
        .map(|i| match i.cmp(&a.done) {
            std::cmp::Ordering::Less => Step::Done,
            std::cmp::Ordering::Equal if running && blocked => Step::Asking,
            _ => Step::Pending,
        })
        .collect()
}

pub struct PrcView {
    cfg: Result<Option<Config>, String>,
    board: Option<Board>,
    loading: bool,
    err: Option<String>,
    upd: Option<String>,
    /// 0 é a coluna Análise (IA); `i + 1` é a coluna `i` do board
    col: usize,
    sel: usize,
    moving: HashSet<String>,
    herdr: Option<Arc<Herdr>>,
    sessions: HashMap<String, Session>,
    open: Option<String>,
    mirror: String,
    input: Entity<TextField>,
    left_w: f32,
    focus: FocusHandle,
    theme: Theme,
    _poll: Task<()>,
}

impl EventEmitter<Event> for PrcView {}

impl PrcView {
    pub fn new(theme: Theme, herdr: Option<Arc<Herdr>>, cx: &mut Context<Self>) -> Self {
        let input = cx
            .new(|cx| TextField::new(theme.clone(), "", "escreva para o Claude", false, false, cx));
        cx.subscribe(&input, |this, _, e: &FieldEvent, cx| {
            if matches!(e, FieldEvent::Submit) {
                let text = this.input.read(cx).value.trim().to_string();
                if !text.is_empty() {
                    this.input.update(cx, |f, cx| f.set(String::new(), cx));
                    if let Some(k) = this.open.clone() {
                        this.send_text(&k, &text, cx);
                    }
                }
            }
        })
        .detach();
        // ponytail: herdr a cada 1 s (socket local); o pane só é lido com o detalhe aberto
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok((h, open)) = this.read_with(cx, |this, _| {
                    (
                        this.herdr.clone(),
                        this.open
                            .as_ref()
                            .and_then(|k| this.sessions.get(k))
                            .and_then(|s| s.pane.clone()),
                    )
                }) else {
                    break;
                };
                if let Some(h) = h {
                    let (sessions, mirror) = cx
                        .background_executor()
                        .spawn(async move { (scan(&h), open.map(|p| read_pane(&h, &p))) })
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            this.sessions = sessions;
                            if let Some(m) = mirror {
                                this.mirror = m;
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        });
        Self {
            cfg: prc::load(),
            board: None,
            loading: false,
            err: None,
            upd: None,
            col: 1,
            sel: 0,
            moving: HashSet::new(),
            herdr,
            sessions: HashMap::new(),
            open: None,
            mirror: String::new(),
            input,
            left_w: 250.,
            focus: cx.focus_handle(),
            theme,
            _poll: poll,
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
                    (b, hhmm())
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
        self.open_cols()
            .iter()
            .find(|&&c| c > 0)
            .map_or(0, |&c| self.cards_in(c).len())
    }

    fn in_analysis(&self, key: &str) -> bool {
        self.sessions.contains_key(key)
    }

    fn open_cols(&self) -> Vec<usize> {
        let mut xs = vec![0];
        if let Some(b) = &self.board {
            xs.extend(
                (0..b.cols.len())
                    .filter(|&i| !b.cols[i].done)
                    .map(|i| i + 1),
            );
        }
        xs
    }

    fn cards_in(&self, col: usize) -> Vec<&PrcCard> {
        let Some(b) = &self.board else {
            return Vec::new();
        };
        if col == 0 {
            return b
                .cards
                .iter()
                .filter(|c| self.in_analysis(&c.key))
                .collect();
        }
        prc::columns(b)
            .into_iter()
            .nth(col - 1)
            .map(|(_, xs)| {
                xs.into_iter()
                    .filter(|c| !self.in_analysis(&c.key))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn clamp(&mut self) {
        if !self.open_cols().contains(&self.col) {
            self.col = 1;
        }
        self.sel = self
            .sel
            .min(self.cards_in(self.col).len().saturating_sub(1));
    }

    fn current(&self) -> Option<PrcCard> {
        self.cards_in(self.col).get(self.sel).map(|c| (*c).clone())
    }

    fn card(&self, key: &str) -> Option<PrcCard> {
        self.board
            .as_ref()?
            .cards
            .iter()
            .find(|c| c.key == key)
            .cloned()
    }

    fn select(&mut self, key: &str) {
        for col in self.open_cols() {
            if let Some(i) = self.cards_in(col).iter().position(|c| c.key == key) {
                self.col = col;
                self.sel = i;
                return;
            }
        }
    }

    fn browse(&self, key: &str) {
        if let Some(b) = &self.board {
            crate::spawn_reaped(
                std::process::Command::new("xdg-open").arg(format!("{}/browse/{key}", b.site)),
            );
        }
    }

    fn running(&self, key: &str) -> bool {
        self.sessions.get(key).is_some_and(|s| s.running)
    }

    fn analysis(&self, key: &str) -> Option<&Analysis> {
        self.sessions.get(key)?.result.as_ref()?.as_ref().ok()
    }

    /// Abre a aba `<KEY> · análise` com o Claude isolado, ou foca a que já existe.
    fn analyze(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(h) = self.herdr.clone() else {
            return cx.emit(Event::Toast(
                "herdr desligado: a análise roda numa aba dele".into(),
            ));
        };
        if let Some(s) = self.sessions.get(key) {
            let _ = h.focus_workspace(&s.ws);
            return cx.emit(Event::Toast(format!(
                "{key} já está em análise: aba focada"
            )));
        }
        let (Some(card), Ok(Some(cfg))) = (self.card(key), self.cfg.clone()) else {
            return;
        };
        let (session, repos, vault) =
            (prc::session_dir(key), prc::expand(&cfg.repos), vault::dir());
        let cmd = match prc::prepare(&card, &session, &repos, &vault) {
            Ok(c) => c,
            Err(e) => return cx.emit(Event::Toast(format!("{key}: {e}"))),
        };
        let r = h.call(
            "workspace.create",
            json!({ "cwd": session, "label": prc::label(key), "focus": false }),
        );
        let pane = r
            .as_ref()
            .ok()
            .and_then(|v| v["root_pane"]["pane_id"].as_str().map(String::from));
        match (r, pane) {
            (Ok(v), Some(p)) => {
                let _ = h.call(
                    "pane.send_text",
                    json!({ "pane_id": p, "text": format!("{cmd}\r") }),
                );
                self.sessions.insert(
                    key.to_string(),
                    Session {
                        ws: v["workspace"]["workspace_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        pane: Some(p),
                        status: None,
                        result: None,
                        running: false,
                    },
                );
                self.col = 0;
                self.select(key);
                cx.emit(Event::Toast(format!(
                    "Abrindo {key} · análise numa aba do herdr: o Claude só lê até o resultado"
                )));
            }
            (Err(e), _) => cx.emit(Event::Toast(format!("herdr: {e}"))),
            _ => cx.emit(Event::Toast("herdr: a aba abriu sem pane".into())),
        }
        cx.notify();
    }

    fn send_text(&mut self, key: &str, text: &str, cx: &mut Context<Self>) {
        let (Some(h), Some(p)) = (
            self.herdr.clone(),
            self.sessions.get(key).and_then(|s| s.pane.clone()),
        ) else {
            return;
        };
        let _ = h.call(
            "pane.send_text",
            json!({ "pane_id": p, "text": format!("{text}\r") }),
        );
        cx.notify();
    }

    fn send_keys(&mut self, key: &str, keys: &[&str], cx: &mut Context<Self>) {
        let (Some(h), Some(p)) = (
            self.herdr.clone(),
            self.sessions.get(key).and_then(|s| s.pane.clone()),
        ) else {
            return;
        };
        let _ = h.call("pane.send_keys", json!({ "pane_id": p, "keys": keys }));
        cx.notify();
    }

    fn exec(&mut self, key: &str, cx: &mut Context<Self>) {
        if !actions(self.analysis(key), false).exec {
            return;
        }
        if !self.running(key) {
            let _ = std::fs::write(prc::session_dir(key).join("exec"), "");
            if let Some(s) = self.sessions.get_mut(key) {
                s.running = true;
            }
            self.send_text(key, prc::EXEC_MSG, cx);
            cx.emit(Event::Toast(format!(
                "{key}: o Claude começa a contingência; cada passo pede sua aprovação"
            )));
        } else {
            self.open = Some(key.to_string());
        }
    }

    fn create_acr(&mut self, key: &str, cx: &mut Context<Self>) {
        let linked = self.card(key).is_some_and(|c| c.acr.is_some());
        if actions(self.analysis(key), linked).acr {
            self.send_text(key, prc::ACR_MSG, cx);
            cx.emit(Event::Toast(format!(
                "{key}: o Claude monta a ACR e pede aprovação antes de criar"
            )));
        }
    }

    /// Fecha a aba e apaga o resultado: o card volta para a coluna do Jira.
    fn end(&mut self, key: &str, cx: &mut Context<Self>) {
        if let (Some(h), Some(s)) = (self.herdr.clone(), self.sessions.remove(key)) {
            let _ = h.close_workspace(&s.ws);
        }
        for f in ["result.json", "exec"] {
            let _ = std::fs::remove_file(prc::session_dir(key).join(f));
        }
        if self.open.as_deref() == Some(key) {
            self.open = None;
        }
        self.clamp();
        cx.emit(Event::Toast(format!(
            "Análise de {key} encerrada: o card volta para a coluna do Jira"
        )));
        cx.notify();
    }

    /// Soltar o card numa coluna: leva a issue ao primeiro status dela no Jira.
    fn drop_on(&mut self, key: &str, col: usize, cx: &mut Context<Self>) {
        if col == 0 {
            return self.analyze(key, cx);
        }
        let Some(b) = &self.board else { return };
        let Some(c) = b.cols.get(col - 1) else { return };
        let Some(to) = prc::target(c).map(String::from) else {
            return;
        };
        let Some(card) = b.cards.iter().find(|x| x.key == key) else {
            return;
        };
        if c.statuses.contains(&card.status) {
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
        if k.key == "escape" && self.open.is_some() {
            self.open = None;
            window.focus(&self.focus, cx);
            cx.stop_propagation();
            return cx.notify();
        }
        if k.modifiers.control || k.modifiers.alt || !self.focus.is_focused(window) {
            return;
        }
        let open = self.open_cols();
        let pos = open.iter().position(|&c| c == self.col).unwrap_or(0);
        let cur = self.current().map(|c| c.key);
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
                if let Some(c) = cur {
                    self.browse(&c);
                }
            }
            "a" => {
                if let Some(c) = cur {
                    self.analyze(&c, cx);
                }
            }
            "e" => {
                if let Some(c) = cur {
                    self.exec(&c, cx);
                }
            }
            "c" => {
                if let Some(c) = cur {
                    self.create_acr(&c, cx);
                }
            }
            "enter" => self.open = cur,
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
        let mut cols: Vec<(String, bool, usize)> =
            vec![("Análise (IA)".into(), false, self.cards_in(0).len())];
        if let Some(b) = &self.board {
            for (i, c) in b.cols.iter().enumerate() {
                cols.push((c.name.clone(), c.done, self.cards_in(i + 1).len()));
            }
        }
        for (i, (name, done, n)) in cols.into_iter().enumerate() {
            let on = i == self.col && !done;
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("pcol{i}")))
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .when(done, |d| d.opacity(0.45))
                    .when(on, |d| d.bg(t.accent_soft))
                    .when(!done, |d| {
                        d.cursor_pointer()
                            .when(!on, |d| d.hover(|d| d.bg(t.raise)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.col = i;
                                this.sel = 0;
                                cx.notify();
                            }))
                    })
                    .child(div().w(px(16.)).text_color(t.comment).child(if done {
                        "✓"
                    } else {
                        "·"
                    }))
                    .child(
                        div()
                            .flex_1()
                            .when(i == 0, |d| d.text_color(t.cyan))
                            .child(name),
                    )
                    .when(!done, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .child(n.to_string()),
                        )
                    }),
            );
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
                d.child(self.note(
                    "Nenhuma receita ainda. Peça para registrar um procedimento de PRC (skill prc-base).",
                ))
            })
            .child(self.note(
                "Uma nota por tipo de PRC, com o nome da label do card. A IA recebe a receita do card e, no fim de cada sessão, propõe atualizar a nota (com sua aprovação).",
            ))
    }

    /// A linha de estado da sessão no card e no detalhe.
    fn run_line(&self, key: &str) -> (Hsla, String) {
        let t = &self.theme;
        let s = self.sessions.get(key);
        let blocked = s.and_then(|s| s.status) == Some(Status::Blocked);
        match s.and_then(|s| s.result.clone()) {
            None if blocked => (t.orange, "! pede sua aprovação".into()),
            None => (t.cyan, "◐ Claude analisando: card, receita e repos…".into()),
            Some(Err(e)) => (rgb(0xff5555).into(), e),
            Some(Ok(a)) => {
                let n = a.cont.as_ref().map_or(0, Vec::len);
                if blocked {
                    let what = if self.running(key) && a.done < n {
                        format!(": passo {} de {n}", a.done + 1)
                    } else {
                        String::new()
                    };
                    (t.orange, format!("! pede sua aprovação{what}"))
                } else if n > 0 && a.done >= n {
                    (t.ok, "✓ contingência aplicada".into())
                } else if self.running(key) {
                    (t.cyan, "◐ executando a contingência…".into())
                } else {
                    (t.ok, "✓ análise pronta".into())
                }
            }
        }
    }

    fn pill(&self, l: &str) -> Div {
        let t = &self.theme;
        div()
            .px(px(6.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.line)
            .font_family(t.mono.clone())
            .text_size(px(10.5))
            .text_color(t.muted)
            .child(l.to_string())
    }

    fn card_view(&self, c: &PrcCard, on: bool, ia: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let rc = prc::recipes(&c.labels, &vault::dir());
        let moving = self.moving.contains(&c.key);
        let (key, theme, chip) = (c.key.clone(), t.clone(), c.key.clone());
        let a = self.analysis(&c.key).cloned();
        let acts = actions(a.as_ref(), c.acr.is_some());
        let mut d = div()
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
                let (theme, chip) = (theme.clone(), chip.clone());
                cx.new(|_| DragChip(chip, theme))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select(&key);
                this.open = Some(key.clone());
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
            .when(ia, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.muted)
                        .child(format!("no Jira: {}", c.status)),
                )
            })
            .when(!c.labels.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .children(c.labels.iter().map(|l| self.pill(l))),
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
            });
        if ia {
            if let Some(a) = &a {
                let diag: String = a.diag.chars().take(160).collect();
                d = d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .pt(px(6.))
                        .border_t_1()
                        .border_color(t.line)
                        .text_size(px(12.))
                        .child(diag)
                        .child(kv(
                            &t,
                            "CONTINGÊNCIA",
                            t.cyan,
                            match &a.cont {
                                Some(c) => format!(
                                    "{} passo{} sem código",
                                    c.len(),
                                    if c.len() == 1 { "" } else { "s" }
                                ),
                                None => "nenhuma sem código".into(),
                            },
                        ))
                        .child(kv(
                            &t,
                            "ACR",
                            t.orange,
                            match (&a.acr, &c.acr) {
                                (_, Some(l)) => format!("{} criada", l.key),
                                (Some(x), None) => format!("{} · a criar", x.repo),
                                (None, None) => "não precisa".into(),
                            },
                        )),
                );
            }
            let (color, line) = self.run_line(&c.key);
            d = d.child(div().text_size(px(11.5)).text_color(color).child(line));
        }
        if on {
            let k = c.key.clone();
            let mut row = div().flex().flex_wrap().gap(px(4.)).pt(px(2.));
            if !ia {
                let k2 = k.clone();
                row = row.child(
                    button(&t, "pc-an", "Analisar  a", true)
                        .on_click(cx.listener(move |this, _, _, cx| this.analyze(&k2, cx))),
                );
            } else {
                if acts.exec {
                    let k2 = k.clone();
                    let label = if self.running(&k) {
                        "Ver execução  e"
                    } else {
                        "Executar contingência  e"
                    };
                    row = row.child(
                        button(&t, "pc-ex", label, true)
                            .on_click(cx.listener(move |this, _, _, cx| this.exec(&k2, cx))),
                    );
                }
                if acts.acr {
                    let k2 = k.clone();
                    row = row.child(
                        button(&t, "pc-acr", "Criar ACR  c", false)
                            .on_click(cx.listener(move |this, _, _, cx| this.create_acr(&k2, cx))),
                    );
                }
                let k2 = k.clone();
                row = row.child(button(&t, "pc-det", "Detalhe e sessão", false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.open = Some(k2.clone());
                        cx.notify();
                    }),
                ));
                let k2 = k.clone();
                row = row.child(
                    button(&t, "pc-end", "Encerrar", false)
                        .text_color(rgb(0xff5555))
                        .on_click(cx.listener(move |this, _, _, cx| this.end(&k2, cx))),
                );
            }
            row = row.child(
                button(&t, "pc-jira", "Jira ↗", false)
                    .on_click(cx.listener(move |this, _, _, _| this.browse(&k))),
            );
            d = d.child(row);
        }
        d
    }

    fn column(&self, col: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let (name, done, ia) = if col == 0 {
            ("ANÁLISE (IA)".to_string(), false, true)
        } else {
            let Some(c) = self.board.as_ref().and_then(|b| b.cols.get(col - 1)) else {
                return div().id("pcol-none");
            };
            (c.name.to_uppercase(), c.done, false)
        };
        let xs: Vec<PrcCard> = self.cards_in(col).into_iter().cloned().collect();
        let soft = t.accent_soft;
        let mut body = div()
            .id(SharedString::from(format!("pcards{col}")))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(6.))
            .overflow_y_scroll();
        if done {
            body = body.child(dashed(&t, "solte aqui para concluir no Jira"));
        } else if xs.is_empty() && ia {
            body = body.child(dashed(&t, "arraste um card aqui  ·  a"));
        } else if xs.is_empty() {
            body = body.child(
                div()
                    .p(px(4.))
                    .text_size(px(12.))
                    .text_color(t.comment)
                    .child("vazio"),
            );
        }
        for (k, x) in xs.iter().enumerate() {
            body = body.child(self.card_view(x, col == self.col && k == self.sel, ia, cx));
        }
        div()
            .id(SharedString::from(format!("pcol-{col}")))
            .flex()
            .flex_col()
            .flex_none()
            .w(px(if done {
                170.
            } else if ia {
                320.
            } else {
                250.
            }))
            .max_h_full()
            .when(ia, |d| d.border_dashed())
            .when(!ia, |d| d.bg(t.panel))
            .border_1()
            .border_color(t.line)
            .rounded(px(6.))
            .drag_over::<DragCard>(move |s, _, _, _| s.bg(soft))
            .on_drop(
                cx.listener(move |this, d: &DragCard, _, cx| this.drop_on(&d.0.clone(), col, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(t.line)
                    .text_size(px(11.))
                    .child(
                        div()
                            .flex()
                            .text_color(if ia {
                                t.cyan
                            } else if col == self.col {
                                t.fg
                            } else {
                                t.muted
                            })
                            .child(div().flex_1().child(if ia {
                                format!("● {name}")
                            } else {
                                name
                            }))
                            .when(!done, |d| {
                                d.child(
                                    div()
                                        .font_family(t.mono.clone())
                                        .child(xs.len().to_string()),
                                )
                            }),
                    )
                    .when(ia, |d| {
                        d.child(
                            div()
                                .text_color(t.muted)
                                .child("diagnóstico, contingência sem código e ACR se precisar"),
                        )
                    }),
            )
            .child(body)
    }

    fn detail(&self, key: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = self.theme.clone();
        let card = self.card(key);
        let s = self.sessions.get(key).cloned();
        let a = self.analysis(key).cloned();
        let blocked = s.as_ref().and_then(|s| s.status) == Some(Status::Blocked);
        let rc = card
            .as_ref()
            .map(|c| prc::recipes(&c.labels, &vault::dir()))
            .unwrap_or_default();
        let head =
            |txt: &'static str, color: Hsla| div().text_size(px(11.)).text_color(color).child(txt);
        let mut body = div()
            .id("prc-det-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(14.));
        if let Some(c) = &card {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(head("JIRA", t.muted))
                    .child(format!("{} · no Jira: {}", c.title, c.status))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(4.))
                            .children(c.labels.iter().map(|l| self.pill(l)))
                            .children(rc.iter().map(|r| {
                                div()
                                    .px(px(6.))
                                    .rounded(px(10.))
                                    .border_1()
                                    .border_color(t.ok)
                                    .text_color(t.ok)
                                    .font_family(t.mono.clone())
                                    .text_size(px(10.5))
                                    .child(format!("◆ prcs/{r}.md"))
                            })),
                    ),
            );
        }
        match &a {
            None if s.is_none() => {
                body = body.child(
                    self.note("Ainda não analisado. Arraste para Análise (IA) ou use Analisar."),
                );
            }
            None => {
                let (color, line) = self.run_line(key);
                body = body.child(div().text_color(color).child(line));
            }
            Some(a) => {
                let mut ev = div()
                    .flex()
                    .flex_col()
                    .font_family(t.mono.clone())
                    .text_size(px(12.))
                    .text_color(t.muted);
                for e in &a.ev {
                    ev = ev.child(format!("· {e}"));
                }
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(head("DIAGNÓSTICO", t.muted))
                        .child(a.diag.clone())
                        .child(ev),
                );
                let mut cont = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(head("CONTINGÊNCIA SEM CÓDIGO", t.cyan));
                match &a.cont {
                    None => {
                        cont = cont.child(
                            self.note("Nenhuma: o cliente só destrava com mudança de código."),
                        )
                    }
                    Some(xs) => {
                        let st = steps(a, self.running(key), blocked);
                        for (x, st) in xs.iter().zip(st) {
                            let (g, color, tail) = match st {
                                Step::Done => ("✓", t.ok, ""),
                                Step::Asking => ("!", t.orange, " · pede sua aprovação"),
                                Step::Pending => ("·", t.comment, ""),
                            };
                            cont = cont.child(
                                div()
                                    .flex()
                                    .gap(px(6.))
                                    .text_size(px(12.5))
                                    .child(div().w(px(14.)).text_color(color).child(g))
                                    .child(div().flex_1().child(format!("{x}{tail}"))),
                            );
                        }
                    }
                }
                body = body.child(cont);
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(head("ACR", t.orange))
                        .child(match (&a.acr, card.as_ref().and_then(|c| c.acr.clone())) {
                            (_, Some(l)) => format!("{} criada · {}", l.key, l.status),
                            (Some(x), None) => format!("{} · {} · {}", x.causa, x.repo, x.loc),
                            (None, None) => {
                                format!("Não precisa: {}", a.no_acr.clone().unwrap_or_default())
                            }
                        }),
                );
            }
        }
        if s.is_some() {
            let k1 = key.to_string();
            let k2 = key.to_string();
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(head("SESSÃO DO CLAUDE", t.muted))
                    .child(
                        div()
                            .id("prc-mirror")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .p(px(8.))
                            .bg(t.win)
                            .border_1()
                            .border_color(t.line)
                            .rounded(px(6.))
                            .font_family(t.mono.clone())
                            .text_size(px(12.))
                            .child(if self.mirror.is_empty() {
                                "…".to_string()
                            } else {
                                self.mirror.clone()
                            }),
                    )
                    .when(blocked, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(button(&t, "pd-yes", "1. Yes", true).on_click(cx.listener(
                                    move |this, _, _, cx| this.send_keys(&k1, &["1"], cx),
                                )))
                                .child(button(&t, "pd-no", "2. No (esc)", false).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.send_keys(&k2, &["esc"], cx)
                                    }),
                                )),
                        )
                    })
                    .child(self.input.clone()),
            );
        }
        let k = key.to_string();
        let mut foot = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .p(px(10.))
            .border_t_1()
            .border_color(t.line);
        if s.is_none() {
            let k2 = k.clone();
            foot = foot.child(
                button(&t, "pd-an", "Analisar  a", true)
                    .on_click(cx.listener(move |this, _, _, cx| this.analyze(&k2, cx))),
            );
        } else {
            let acts = actions(a.as_ref(), card.as_ref().is_some_and(|c| c.acr.is_some()));
            if acts.exec && !self.running(&k) {
                let k2 = k.clone();
                foot = foot.child(
                    button(&t, "pd-ex", "Executar contingência", true)
                        .on_click(cx.listener(move |this, _, _, cx| this.exec(&k2, cx))),
                );
            }
            if acts.acr {
                let k2 = k.clone();
                foot = foot.child(
                    button(&t, "pd-acr", "Criar ACR", false)
                        .on_click(cx.listener(move |this, _, _, cx| this.create_acr(&k2, cx))),
                );
            }
            let k2 = k.clone();
            foot = foot.child(
                button(&t, "pd-full", "Abrir em tela cheia", false).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let (Some(h), Some(s)) = (this.herdr.clone(), this.sessions.get(&k2)) {
                            let _ = h.focus_workspace(&s.ws);
                        }
                        cx.notify();
                    },
                )),
            );
            let k2 = k.clone();
            foot = foot.child(
                button(&t, "pd-end", "Encerrar", false)
                    .text_color(rgb(0xff5555))
                    .on_click(cx.listener(move |this, _, _, cx| this.end(&k2, cx))),
            );
        }
        foot = foot.child(div().flex_1()).child(
            button(&t, "pd-jira", "Jira ↗", false)
                .on_click(cx.listener(move |this, _, _, _| this.browse(&k))),
        );
        div()
            .id("prc-det")
            .w(px(560.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.raise)
            .border_l_1()
            .border_color(t.line)
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
                            .font_family(t.mono.clone())
                            .text_color(t.muted)
                            .child(key.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child(card.map(|c| c.title).unwrap_or_default()),
                    )
                    .child(button(&t, "pd-x", "Esc", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.open = None;
                            cx.notify();
                        },
                    ))),
            )
            .child(body)
            .child(foot)
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
        kb = kb.child(self.column(0, cx));
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
            kb = kb.child(self.column(i + 1, cx));
        }
        base.child(bar).child(kb)
    }
}

fn kv(t: &Theme, k: &'static str, color: Hsla, v: String) -> Div {
    div()
        .flex()
        .gap(px(8.))
        .child(
            div()
                .w(px(90.))
                .flex_none()
                .text_size(px(10.5))
                .text_color(color)
                .child(k),
        )
        .child(div().flex_1().text_color(t.fg).child(v))
}

fn dashed(t: &Theme, text: &'static str) -> Div {
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

/// As abas `<KEY> · análise` vivas, o status do agente nelas e o resultado gravado.
fn scan(h: &Herdr) -> HashMap<String, Session> {
    let Ok(ws) = h.workspaces() else {
        return HashMap::new();
    };
    let agents = h.agents().unwrap_or_default();
    ws.into_iter()
        .filter_map(|w| {
            let key = w.label.strip_suffix(" · análise")?.to_string();
            let ag = agents.iter().find(|a| a.workspace_id == w.workspace_id);
            let pane = ag
                .map(|a| a.pane_id.clone())
                .or_else(|| root_pane(h, &w.workspace_id));
            let dir: PathBuf = prc::session_dir(&key);
            Some((
                key,
                Session {
                    ws: w.workspace_id,
                    pane,
                    status: ag.map(|a| a.agent_status),
                    result: prc::result(&dir),
                    running: dir.join("exec").exists(),
                },
            ))
        })
        .collect()
}

fn root_pane(h: &Herdr, ws: &str) -> Option<String> {
    let panes = h.panes().ok()?;
    panes["panes"]
        .as_array()?
        .iter()
        .find(|p| p["workspace_id"] == ws)
        .and_then(|p| p["pane_id"].as_str().map(String::from))
}

fn read_pane(h: &Herdr, pane: &str) -> String {
    h.call(
        "pane.read",
        json!({ "pane_id": pane, "source": "recent", "lines": 25 }),
    )
    .ok()
    .and_then(|v| {
        v["text"]
            .as_str()
            .or_else(|| v["read"]["text"].as_str())
            .map(String::from)
    })
    .unwrap_or_default()
}

fn hhmm() -> String {
    std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

impl Render for PrcView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let open = self.open.clone();
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
            .when_some(open, |d, k| d.child(self.detail(&k, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::{Actions, Step, actions, steps};
    use crate::prc::{Acr, Analysis};

    fn an(cont: Option<usize>, done: usize, acr: bool) -> Analysis {
        Analysis {
            diag: "d".into(),
            ev: vec![],
            cont: cont.map(|n| (0..n).map(|i| format!("p{i}")).collect()),
            done,
            acr: acr.then(|| Acr {
                causa: "c".into(),
                repo: "loja-api".into(),
                loc: "a.ts:1".into(),
            }),
            no_acr: None,
        }
    }

    #[test]
    fn acoes_conforme_o_resultado() {
        let none = Actions {
            exec: false,
            acr: false,
        };
        assert_eq!(actions(None, false), none);
        assert_eq!(
            actions(Some(&an(Some(2), 0, true)), false),
            Actions {
                exec: true,
                acr: true
            }
        );
        assert_eq!(
            actions(Some(&an(None, 0, true)), false),
            Actions {
                exec: false,
                acr: true
            }
        );
        assert_eq!(actions(Some(&an(Some(2), 2, false)), false), none);
        assert_eq!(
            actions(Some(&an(Some(1), 0, true)), true),
            Actions {
                exec: true,
                acr: false
            }
        );
    }

    #[test]
    fn estado_dos_passos() {
        let a = an(Some(3), 1, false);
        assert_eq!(
            steps(&a, true, true),
            [Step::Done, Step::Asking, Step::Pending]
        );
        assert_eq!(
            steps(&a, true, false),
            [Step::Done, Step::Pending, Step::Pending]
        );
        assert_eq!(
            steps(&a, false, true),
            [Step::Done, Step::Pending, Step::Pending]
        );
        assert!(steps(&an(None, 0, true), true, true).is_empty());
    }
}
