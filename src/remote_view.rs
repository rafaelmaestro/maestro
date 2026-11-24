//! O diálogo do acesso remoto (`remoteDlg` do protótipo): QR, URL sem o token, tempo restante e os botões.

use std::time::Duration;

use gpui_kit::{prelude::FluentBuilder, *};

use crate::theme::Theme;

/// "2 h 00" ou "58 min", como o `remFmt` do protótipo.
pub fn left(d: Duration) -> String {
    let m = d.as_secs().div_ceil(60);
    if m >= 60 {
        format!("{} h {:02}", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

/// Cada linha vira faixas de módulos iguais: bem menos elementos que um `div` por módulo.
fn qr(m: &[Vec<bool>], size: f32) -> Div {
    let cell = size / m.len().max(1) as f32;
    let mut col = div()
        .flex()
        .flex_col()
        .size(px(size))
        .bg(rgb(0xffffff))
        .rounded(px(6.));
    for row in m {
        let mut line = div().flex().h(px(cell));
        let mut i = 0;
        while i < row.len() {
            let dark = row[i];
            let run = row[i..].iter().take_while(|&&x| x == dark).count();
            line = line.child(
                div()
                    .w(px(cell * run as f32))
                    .h_full()
                    .when(dark, |d| d.bg(rgb(0x000000))),
            );
            i += run;
        }
        col = col.child(line);
    }
    col
}

pub fn dialog(t: &Theme, url: &str, code: &[Vec<bool>], remaining: Duration, acts: Div) -> Div {
    div()
        .w(px(480.))
        .max_w_full()
        .bg(t.raise)
        .border_1()
        .border_color(t.line)
        .rounded(px(8.))
        .p(px(16.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Acesso remoto (celular)"),
        )
        .child(
            div()
                .flex()
                .gap(px(14.))
                .items_center()
                .child(div().p(px(8.)).bg(rgb(0xffffff)).rounded(px(6.)).child(qr(code, 168.)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child("Leia com a câmera do celular. O link pareia um aparelho só.")
                        .child(div().font_family(t.mono.clone()).text_color(t.fg).child(url.to_string()))
                        .child(format!("desliga em {} sem uso · no máximo 8 h", left(remaining))),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(t.orange)
                .border_l_2()
                .border_color(t.orange)
                .pl(px(8.))
                .child("Quem estiver com este celular manda prompts aos seus Claudes, e prompt vira comando nesta máquina. O ngrok vê o tráfego."),
        )
        .child(acts)
}

#[cfg(test)]
mod tests {
    use super::left;
    use std::time::Duration;

    #[test]
    fn tempo_restante() {
        assert_eq!(left(Duration::from_secs(7200)), "2 h 00");
        assert_eq!(left(Duration::from_secs(3599)), "1 h 00");
        assert_eq!(left(Duration::from_secs(61)), "2 min");
        assert_eq!(left(Duration::from_secs(4000)), "1 h 07");
    }
}
