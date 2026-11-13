//! SVG export of the current view.

use crate::draw::{plan_labels, Highlight, Style, LABEL_PX};
use crate::scene::{screen_radius, Scene};
use crate::view::View;
use std::fmt::Write;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn to_svg(scene: &Scene, view: &View, hl: &Highlight, style: &Style) -> String {
    let (w, h) = (view.width, view.height);
    let mut s = String::new();
    let _ = writeln!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">"
    );
    let _ = writeln!(
        s,
        "<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>",
        hex(style.background())
    );
    let ec = hex(style.edge_color());
    let _ = writeln!(
        s,
        "<g fill=\"none\" stroke=\"{ec}\" stroke-opacity=\"0.45\" stroke-width=\"1\">"
    );
    for e in &scene.edges {
        if e.optional && !style.show_optional {
            continue;
        }
        let a = view.to_screen(scene.nodes[e.from as usize].pos);
        let b = view.to_screen(scene.nodes[e.to as usize].pos);
        if (a[0] < 0.0 && b[0] < 0.0)
            || (a[0] > w && b[0] > w)
            || (a[1] < 0.0 && b[1] < 0.0)
            || (a[1] > h && b[1] > h)
        {
            continue;
        }
        let dash = if e.optional {
            " stroke-dasharray=\"5 4\""
        } else {
            ""
        };
        let _ = writeln!(
            s,
            "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\"{dash}/>",
            a[0], a[1], b[0], b[1]
        );
    }
    s.push_str("</g>\n<g>\n");
    for n in &scene.nodes {
        let p = view.to_screen(n.pos);
        if p[0] < -30.0 || p[0] > w + 30.0 || p[1] < -30.0 || p[1] > h + 30.0 {
            continue;
        }
        let _ =
            writeln!(
            s,
            "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{}\"><title>{}</title></circle>",
            p[0], p[1], screen_radius(n.radius, view.zoom), hex(n.color), esc(&n.label)
        );
    }
    s.push_str("</g>\n");
    let _ = writeln!(
        s,
        "<g font-family=\"sans-serif\" font-size=\"{LABEL_PX}\" fill=\"{}\">",
        hex(style.label_color())
    );
    for (i, x, y) in plan_labels(scene, view, hl, style) {
        let _ = writeln!(
            s,
            "<text x=\"{x:.1}\" y=\"{:.1}\">{}</text>",
            y + LABEL_PX * 0.35,
            esc(&scene.nodes[i].label)
        );
    }
    s.push_str("</g>\n</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Edge, Node};

    #[test]
    fn svg_is_wellformed_enough() {
        let nodes = vec![
            Node {
                id: 0,
                pos: [0.0, 0.0],
                radius: 4.0,
                color: [1, 2, 3],
                label: "a<b>&\"".into(),
            },
            Node {
                id: 1,
                pos: [50.0, 0.0],
                radius: 4.0,
                color: [1, 2, 3],
                label: "c".into(),
            },
        ];
        let sc = Scene::new(
            nodes,
            vec![Edge {
                from: 0,
                to: 1,
                optional: true,
            }],
        );
        let v = sc.fit_view(200.0, 100.0);
        let hl = Highlight {
            hovered: Some(0),
            ..Default::default()
        };
        let out = to_svg(&sc, &v, &hl, &Style::default());
        assert!(out.starts_with("<svg") && out.trim_end().ends_with("</svg>"));
        assert!(out.contains("a&lt;b&gt;&amp;&quot;"));
        assert!(out.contains("stroke-dasharray"));
        assert_eq!(out.matches("<circle").count(), 2);
    }
}
