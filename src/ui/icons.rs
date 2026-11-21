//! 矢量图标绘制器（纯 egui::Painter 抗锯齿路径，无 Emoji、无位图模糊）。

use std::f32::consts::PI;

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Shield,
    Dashboard,
    Sliders,
    Terminal,
    Wifi,
    Router,
    Globe,
    Key,
    Refresh,
    Play,
    Pause,
    Sun,
    Moon,
    Eye,
    EyeOff,
    Search,
    Copy,
    ExternalLink,
    Check,
    Minimize,
    Close,
    ChevronRight,
    Bell,
    PanelLeft,
    Activity,
    Info,
    Settings,
    ArrowLeft,
    User,
    ArrowUpDown,
    ChevronDown,
    Folder,
}

/// 在给定矩形内按比例居中绘制矢量图标。
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let stroke_w = (s * 0.105).clamp(1.25_f32, 2.0_f32);
    let st = Stroke::new(stroke_w, color);

    let pt = |nx: f32, ny: f32| -> Pos2 { Pos2::new(c.x + nx * s * 0.5, c.y + ny * s * 0.5) };

    match icon {
        Icon::Shield => {
            // 盾牌轮廓 + 内部信号勾
            let outline = vec![
                pt(0.0, -0.82),
                pt(0.70, -0.52),
                pt(0.66, 0.10),
                pt(0.36, 0.58),
                pt(0.0, 0.84),
                pt(-0.36, 0.58),
                pt(-0.66, 0.10),
                pt(-0.70, -0.52),
            ];
            painter.add(egui::Shape::closed_line(outline, st));
            painter.line_segment([pt(-0.28, 0.02), pt(-0.06, 0.26)], st);
            painter.line_segment([pt(-0.06, 0.26), pt(0.34, -0.22)], st);
        }
        Icon::Dashboard => {
            // 2×2 现代 Bento 格（左上略高，右下略高）
            let g = s * 0.08;
            let half = s * 0.40;
            let r = egui::CornerRadius::same(2);
            let tl = Rect::from_min_max(
                Pos2::new(c.x - half, c.y - half),
                Pos2::new(c.x - g * 0.6, c.y + g * 0.4),
            );
            let tr = Rect::from_min_max(
                Pos2::new(c.x + g * 0.6, c.y - half),
                Pos2::new(c.x + half, c.y - g * 0.8),
            );
            let bl = Rect::from_min_max(
                Pos2::new(c.x - half, c.y + g * 1.6),
                Pos2::new(c.x - g * 0.6, c.y + half),
            );
            let br = Rect::from_min_max(
                Pos2::new(c.x + g * 0.6, c.y + g * 0.4),
                Pos2::new(c.x + half, c.y + half),
            );
            for cell in [tl, tr, bl, br] {
                painter.rect_stroke(cell, r, st, egui::StrokeKind::Middle);
            }
        }
        Icon::Sliders => {
            // 两条舒展的水平滑轨 + 清晰圆钮（15px 小尺寸下极度干净）
            let rows = [(-0.34_f32, 0.24_f32), (0.34_f32, -0.24_f32)];
            let kr = s * 0.145;
            for (y, kx) in rows {
                let py = c.y + y * s * 0.5;
                let left = c.x - s * 0.40;
                let right = c.x + s * 0.40;
                let knob_x = c.x + kx * s * 0.5;
                if knob_x - kr > left {
                    painter.line_segment([Pos2::new(left, py), Pos2::new(knob_x - kr, py)], st);
                }
                if knob_x + kr < right {
                    painter.line_segment([Pos2::new(knob_x + kr, py), Pos2::new(right, py)], st);
                }
                painter.circle_stroke(Pos2::new(knob_x, py), kr, st);
            }
        }
        Icon::Terminal => {
            let box_rect = Rect::from_center_size(c, Vec2::splat(s * 0.82));
            painter.rect_stroke(
                box_rect,
                egui::CornerRadius::same(3),
                st,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([pt(-0.44, -0.22), pt(-0.14, 0.02)], st);
            painter.line_segment([pt(-0.14, 0.02), pt(-0.44, 0.26)], st);
            painter.line_segment([pt(0.06, 0.24), pt(0.44, 0.24)], st);
        }
        Icon::Wifi => {
            let base = pt(0.0, 0.52);
            painter.circle_filled(base, (s * 0.095).max(1.4_f32), color);
            paint_arc(painter, base, s * 0.28, -PI * 0.78, -PI * 0.22, st);
            paint_arc(painter, base, s * 0.48, -PI * 0.78, -PI * 0.22, st);
            paint_arc(painter, base, s * 0.68, -PI * 0.78, -PI * 0.22, st);
        }
        Icon::Router => {
            // 路由器机身 + 双天线 + 指示灯
            let body = Rect::from_min_max(pt(-0.76, 0.08), pt(0.76, 0.64));
            painter.rect_stroke(
                body,
                egui::CornerRadius::same(3),
                st,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([pt(-0.36, 0.08), pt(-0.46, -0.58)], st);
            painter.line_segment([pt(0.36, 0.08), pt(0.46, -0.58)], st);
            painter.circle_filled(pt(-0.36, 0.36), (s * 0.07).max(1.2_f32), color);
            painter.circle_filled(pt(0.0, 0.36), (s * 0.07).max(1.2_f32), color);
            painter.line_segment([pt(0.28, 0.36), pt(0.50, 0.36)], st);
        }
        Icon::Globe => {
            let r = s * 0.41;
            painter.circle_stroke(c, r, st);
            // 水平纬线
            painter.line_segment([Pos2::new(c.x - r, c.y), Pos2::new(c.x + r, c.y)], st);
            // 纵向椭圆经线
            paint_ellipse(painter, c, r * 0.46, r, st);
        }
        Icon::Key => {
            let head = pt(-0.32, 0.0);
            let r = s * 0.23;
            painter.circle_stroke(head, r, st);
            painter.line_segment([Pos2::new(head.x + r, head.y), pt(0.76, 0.0)], st);
            painter.line_segment([pt(0.42, 0.0), pt(0.42, 0.28)], st);
            painter.line_segment([pt(0.68, 0.0), pt(0.68, 0.22)], st);
        }
        Icon::Refresh => {
            let r = s * 0.35;
            // 顺时针缺口圆弧（右上缺口）+ 清晰指向右下的小箭头
            paint_arc(painter, c, r, -PI * 0.12, PI * 1.48, st);
            let a_end = -PI * 0.12;
            let tip = Pos2::new(c.x + r * a_end.cos(), c.y + r * a_end.sin());
            painter.line_segment([tip, tip + Vec2::new(-s * 0.24, -s * 0.04)], st);
            painter.line_segment([tip, tip + Vec2::new(s * 0.04, -s * 0.24)], st);
        }
        Icon::Play => {
            let tri = vec![pt(-0.34, -0.56), pt(0.56, 0.0), pt(-0.34, 0.56)];
            painter.add(egui::Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        Icon::Pause => {
            let bw = s * 0.16;
            let bh = s * 0.56;
            let left = Rect::from_center_size(Pos2::new(c.x - s * 0.17, c.y), Vec2::new(bw, bh));
            let right = Rect::from_center_size(Pos2::new(c.x + s * 0.17, c.y), Vec2::new(bw, bh));
            painter.rect_filled(left, egui::CornerRadius::same(1), color);
            painter.rect_filled(right, egui::CornerRadius::same(1), color);
        }
        Icon::Sun => {
            let r = s * 0.20;
            painter.circle_stroke(c, r, st);
            let inner = s * 0.31;
            let outer = s * 0.43;
            for i in 0..8 {
                let a = (i as f32) * PI / 4.0;
                let dir = Vec2::new(a.cos(), a.sin());
                painter.line_segment([c + dir * inner, c + dir * outer], st);
            }
        }
        Icon::Moon => {
            // 精确两圆求交新月轮廓（零交叉、两端尖角完美闭合）
            let r1 = s * 0.38;
            let r2 = s * 0.34;
            let c2 = Pos2::new(c.x + s * 0.22, c.y - s * 0.18);
            let d = (c2 - c).length();
            let base_angle = (c2.y - c.y).atan2(c2.x - c.x);
            let cos_a1 = ((r1 * r1 + d * d - r2 * r2) / (2.0 * r1 * d)).clamp(-1.0, 1.0);
            let half_a1 = cos_a1.acos();
            let p_start = base_angle + half_a1;
            let p_end = base_angle + 2.0 * PI - half_a1;

            let mut pts = Vec::with_capacity(28);
            let steps_out = 16;
            for i in 0..=steps_out {
                let a = p_start + (p_end - p_start) * (i as f32 / steps_out as f32);
                pts.push(Pos2::new(c.x + r1 * a.cos(), c.y + r1 * a.sin()));
            }
            // 从外弧终点沿内圆弧回到外弧起点
            let tip_end = *pts.last().unwrap();
            let tip_start = pts[0];
            let in_start = (tip_end.y - c2.y).atan2(tip_end.x - c2.x);
            let mut in_end = (tip_start.y - c2.y).atan2(tip_start.x - c2.x);
            if in_end > in_start {
                in_end -= 2.0 * PI;
            }
            let steps_in = 12;
            for i in 1..steps_in {
                let a = in_start + (in_end - in_start) * (i as f32 / steps_in as f32);
                pts.push(Pos2::new(c2.x + r2 * a.cos(), c2.y + r2 * a.sin()));
            }
            painter.add(egui::Shape::closed_line(pts, st));
        }
        Icon::Eye => {
            let mut top = Vec::with_capacity(11);
            let mut bot = Vec::with_capacity(11);
            for i in 0..=10 {
                let t = (i as f32 / 10.0) * 2.0 - 1.0;
                let arch = (1.0 - t * t) * 0.42;
                top.push(pt(t * 0.78, -arch));
                bot.push(pt(t * 0.78, arch));
            }
            painter.add(egui::Shape::line(top, st));
            painter.add(egui::Shape::line(bot, st));
            painter.circle_stroke(c, s * 0.15, st);
        }
        Icon::EyeOff => {
            paint(painter, rect, Icon::Eye, color);
            painter.line_segment([pt(-0.62, 0.56), pt(0.62, -0.56)], st);
        }
        Icon::Search => {
            let lens = pt(-0.14, -0.14);
            let r = s * 0.28;
            painter.circle_stroke(lens, r, st);
            let dir = Vec2::new(0.707, 0.707);
            painter.line_segment([lens + dir * r, pt(0.66, 0.66)], st);
        }
        Icon::Copy => {
            let front = Rect::from_min_max(pt(-0.18, -0.18), pt(0.64, 0.68));
            painter.rect_stroke(
                front,
                egui::CornerRadius::same(2),
                st,
                egui::StrokeKind::Middle,
            );
            let back = vec![
                pt(-0.18, 0.24),
                pt(-0.60, 0.24),
                pt(-0.60, -0.64),
                pt(0.24, -0.64),
                pt(0.24, -0.18),
            ];
            painter.add(egui::Shape::line(back, st));
        }
        Icon::ExternalLink => {
            let box_pts = vec![
                pt(0.05, -0.56),
                pt(-0.62, -0.56),
                pt(-0.62, 0.62),
                pt(0.56, 0.62),
                pt(0.56, -0.05),
            ];
            painter.add(egui::Shape::line(box_pts, st));
            painter.line_segment([pt(-0.08, 0.08), pt(0.64, -0.64)], st);
            painter.line_segment([pt(0.24, -0.64), pt(0.64, -0.64)], st);
            painter.line_segment([pt(0.64, -0.64), pt(0.64, -0.24)], st);
        }
        Icon::Check => {
            painter.line_segment([pt(-0.56, 0.04), pt(-0.16, 0.44)], st);
            painter.line_segment([pt(-0.16, 0.44), pt(0.60, -0.40)], st);
        }
        Icon::Minimize => {
            painter.line_segment([pt(-0.52, 0.04), pt(0.52, 0.04)], st);
        }
        Icon::Close => {
            painter.line_segment([pt(-0.46, -0.46), pt(0.46, 0.46)], st);
            painter.line_segment([pt(0.46, -0.46), pt(-0.46, 0.46)], st);
        }
        Icon::ChevronRight => {
            painter.line_segment([pt(-0.22, -0.44), pt(0.26, 0.0)], st);
            painter.line_segment([pt(0.26, 0.0), pt(-0.22, 0.44)], st);
        }
        Icon::Bell => {
            let pts = vec![
                pt(-0.56, 0.36),
                pt(-0.44, 0.16),
                pt(-0.44, -0.18),
                pt(-0.28, -0.52),
                pt(0.0, -0.64),
                pt(0.28, -0.52),
                pt(0.44, -0.18),
                pt(0.44, 0.16),
                pt(0.56, 0.36),
            ];
            painter.add(egui::Shape::closed_line(pts, st));
            painter.line_segment([pt(-0.18, 0.56), pt(0.18, 0.56)], st);
        }
        Icon::PanelLeft => {
            let outer = Rect::from_center_size(c, Vec2::splat(s * 0.80));
            painter.rect_stroke(
                outer,
                egui::CornerRadius::same(3),
                st,
                egui::StrokeKind::Middle,
            );
            let split_x = outer.left() + outer.width() * 0.34;
            painter.line_segment(
                [
                    Pos2::new(split_x, outer.top()),
                    Pos2::new(split_x, outer.bottom()),
                ],
                st,
            );
        }
        Icon::Activity => {
            let pts = vec![
                pt(-0.78, 0.04),
                pt(-0.42, 0.04),
                pt(-0.22, -0.58),
                pt(0.14, 0.58),
                pt(0.36, -0.12),
                pt(0.50, 0.04),
                pt(0.78, 0.04),
            ];
            painter.add(egui::Shape::line(pts, st));
        }
        Icon::Info => {
            painter.circle_stroke(c, s * 0.42, st);
            painter.circle_filled(pt(0.0, -0.34), (s * 0.075).max(1.0), color);
            painter.line_segment([pt(0.0, -0.06), pt(0.0, 0.42)], st);
        }
        Icon::Settings => {
            painter.circle_stroke(c, s * 0.16, st);
            painter.circle_stroke(c, s * 0.31, st);
            for i in 0..8 {
                let a = (i as f32) * (PI * 0.25);
                let p0 = Pos2::new(c.x + s * 0.32 * a.cos(), c.y + s * 0.32 * a.sin());
                let p1 = Pos2::new(c.x + s * 0.44 * a.cos(), c.y + s * 0.44 * a.sin());
                painter.line_segment([p0, p1], st);
            }
        }
        Icon::ArrowLeft => {
            painter.line_segment([pt(0.54, 0.0), pt(-0.54, 0.0)], st);
            painter.line_segment([pt(-0.10, -0.42), pt(-0.54, 0.0)], st);
            painter.line_segment([pt(-0.10, 0.42), pt(-0.54, 0.0)], st);
        }
        Icon::User => {
            painter.circle_stroke(pt(0.0, -0.28), s * 0.20, st);
            paint_arc(painter, pt(0.0, 0.62), s * 0.38, PI * 1.15, PI * 1.85, st);
        }
        Icon::ArrowUpDown => {
            // Left up arrow
            painter.line_segment([pt(-0.30, 0.52), pt(-0.30, -0.52)], st);
            painter.line_segment([pt(-0.56, -0.24), pt(-0.30, -0.52)], st);
            painter.line_segment([pt(-0.04, -0.24), pt(-0.30, -0.52)], st);
            // Right down arrow
            painter.line_segment([pt(0.30, -0.52), pt(0.30, 0.52)], st);
            painter.line_segment([pt(0.04, 0.24), pt(0.30, 0.52)], st);
            painter.line_segment([pt(0.56, 0.24), pt(0.30, 0.52)], st);
        }
        Icon::ChevronDown => {
            painter.line_segment([pt(-0.42, -0.18), pt(0.0, 0.26)], st);
            painter.line_segment([pt(0.0, 0.26), pt(0.42, -0.18)], st);
        }
        Icon::Folder => {
            let pts = vec![
                pt(-0.66, -0.44),
                pt(-0.18, -0.44),
                pt(-0.02, -0.24),
                pt(0.66, -0.24),
                pt(0.66, 0.48),
                pt(-0.66, 0.48),
            ];
            painter.add(egui::Shape::closed_line(pts, st));
        }
    }
}

fn paint_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    start_angle: f32,
    end_angle: f32,
    stroke: Stroke,
) {
    let steps = 16;
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = start_angle + (end_angle - start_angle) * (i as f32 / steps as f32);
            Pos2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(pts, stroke));
}

fn paint_ellipse(painter: &egui::Painter, center: Pos2, rx: f32, ry: f32, stroke: Stroke) {
    let steps = 24;
    let pts: Vec<Pos2> = (0..steps)
        .map(|i| {
            let a = (i as f32 / steps as f32) * 2.0 * PI;
            Pos2::new(center.x + rx * a.cos(), center.y + ry * a.sin())
        })
        .collect();
    painter.add(egui::Shape::closed_line(pts, stroke));
}
