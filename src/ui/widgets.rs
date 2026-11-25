//! 自绘现代控件库：卡片、状态徽章、滑动开关、分段控件、步进数字框、按钮与可拖拽分隔条。

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::{
    bold_text, lerp_color, with_alpha, Palette, ThemeMode, RADIUS_MD, RADIUS_SM, SPACE_MD,
};

/// 标准卡片容器（浅色下带极轻底阴影，深色下带干净发丝描边）。
pub fn card(ui: &mut egui::Ui, p: &Palette, body: impl FnOnce(&mut egui::Ui)) {
    card_padded(ui, p, SPACE_MD as i8, body);
}

pub fn card_padded(ui: &mut egui::Ui, p: &Palette, pad: i8, body: impl FnOnce(&mut egui::Ui)) {
    let shadow = match p.mode {
        ThemeMode::Light => egui::Shadow {
            offset: [0, 1],
            blur: 4,
            spread: 0,
            color: p.shadow,
        },
        ThemeMode::Dark => egui::Shadow::NONE,
    };
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin::same(pad))
        .shadow(shadow)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
}

/// 带左侧竖向状态色条的高亮横幅卡（顶部主状态栏）。
pub fn accent_card(
    ui: &mut egui::Ui,
    p: &Palette,
    stripe_color: Color32,
    body: impl FnOnce(&mut egui::Ui),
) {
    let shadow = match p.mode {
        ThemeMode::Light => egui::Shadow {
            offset: [0, 1],
            blur: 5,
            spread: 0,
            color: p.shadow,
        },
        ThemeMode::Dark => egui::Shadow::NONE,
    };
    let response = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(egui::Margin {
            left: 15,
            right: 12,
            top: 9,
            bottom: 9,
        })
        .shadow(shadow)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
    let rect = response.response.rect;
    let bar = Rect::from_min_max(
        Pos2::new(rect.left() + 1.0, rect.top() + 7.0),
        Pos2::new(rect.left() + 4.5, rect.bottom() - 7.0),
    );
    ui.painter()
        .rect_filled(bar, CornerRadius::same(2), stripe_color);
}

/// 呼吸光晕状态圆点。
pub fn pulsing_dot(ui: &mut egui::Ui, color: Color32, size: f32, pulse: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let center = rect.center();
    let t = if pulse {
        let time = ui.input(|i| i.time) as f32;
        0.5 + 0.5 * (time * 2.6).sin()
    } else {
        0.35
    };
    let halo_r = size * (0.34 + 0.14 * t);
    let halo_alpha = if pulse {
        (26.0 + 30.0 * (1.0 - t)) as u8
    } else {
        32
    };
    ui.painter()
        .circle_filled(center, halo_r, with_alpha(color, halo_alpha));
    ui.painter().circle_filled(center, size * 0.24, color);
}

/// 带状态圆点的胶囊徽章。
pub fn status_pill(
    ui: &mut egui::Ui,
    color: Color32,
    soft_bg: Color32,
    border: Color32,
    text: &str,
) {
    egui::Frame::new()
        .fill(soft_bg)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(CornerRadius::same(99))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.2, color);
                ui.label(bold_text(text, 11.5, color));
            });
        });
}

/// 精确垂直中线绘制区块标题（图标 + 粗体文字与同行的 26px 胶囊/分段器严格齐平）。
pub fn section_heading(
    ui: &mut egui::Ui,
    icon: Icon,
    icon_color: Color32,
    title: &str,
    text_color: Color32,
    row_height: f32,
) -> egui::Response {
    let font = egui::FontId::new(13.0, crate::ui::theme::bold_family());
    let galley = ui
        .painter()
        .layout_no_wrap(title.to_string(), font, text_color);
    let icon_sz = 14.5_f32;
    let gap = 6.0_f32;
    let total_w = icon_sz + gap + galley.size().x + 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, row_height), Sense::hover());
    let cy = rect.center().y;

    let ic_rect = Rect::from_center_size(
        Pos2::new(rect.left() + icon_sz * 0.5, cy),
        Vec2::splat(icon_sz),
    );
    icons::paint(ui.painter(), ic_rect, icon, icon_color);
    ui.painter().galley(
        Pos2::new(rect.left() + icon_sz + gap, cy - galley.size().y * 0.5),
        galley,
        text_color,
    );
    resp
}

/// 纯文字小标签胶囊（精确居中绘制，彻底避免不同字体的基线偏移）。
pub fn badge(
    ui: &mut egui::Ui,
    color: Color32,
    soft_bg: Color32,
    border: Color32,
    text: &str,
) -> egui::Response {
    let font = egui::FontId::new(11.0, crate::ui::theme::bold_family());
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = Vec2::new(galley.size().x + 14.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(5),
        soft_bg,
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    let text_pos = Pos2::new(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, color);
    resp
}

/// 等高 26px KPI 指标徽章（用于监控看板顶部与分段器完全齐平，返回 Response 支持悬停说明）。
pub fn kpi_badge(
    ui: &mut egui::Ui,
    color: Color32,
    soft_bg: Color32,
    border: Color32,
    text: &str,
) -> egui::Response {
    let font = egui::FontId::new(11.0, crate::ui::theme::bold_family());
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = Vec2::new(galley.size().x + 14.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        soft_bg,
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );
    let text_pos = Pos2::new(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, color);
    resp
}

/// 主操作按钮（带可选矢量图标）。
pub fn primary_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Option<Icon>,
    text: &str,
) -> egui::Response {
    styled_button(ui, p, icon, text, true)
}

/// 次操作按钮（边框幽灵样式，带可选矢量图标）。
pub fn secondary_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Option<Icon>,
    text: &str,
) -> egui::Response {
    styled_button(ui, p, icon, text, false)
}

fn styled_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Option<Icon>,
    text: &str,
    primary: bool,
) -> egui::Response {
    let font = if primary {
        egui::FontId::new(12.5, crate::ui::theme::bold_family())
    } else {
        egui::FontId::proportional(12.5)
    };
    let text_color = if primary { p.on_accent } else { p.text };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, text_color);
    let icon_space = if icon.is_some() { 19.0 } else { 0.0 };
    let width = (galley.size().x + icon_space + 22.0).max(74.0);
    let height = 29.0;

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);

    let hover_t =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);

    let (fill, stroke) = if primary {
        let base = if response.is_pointer_button_down_on() {
            lerp_color(p.accent_hover, Color32::BLACK, 0.12)
        } else {
            lerp_color(p.accent, p.accent_hover, hover_t)
        };
        (base, Stroke::NONE)
    } else {
        let base = if response.is_pointer_button_down_on() {
            p.subtle
        } else {
            lerp_color(p.card, p.card_hover, hover_t)
        };
        let border = lerp_color(p.border, p.border_strong, hover_t);
        (base, Stroke::new(1.0_f32, border))
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS_SM),
        fill,
        stroke,
        StrokeKind::Inside,
    );

    let content_w = galley.size().x + icon_space;
    let start_x = rect.center().x - content_w * 0.5;
    let mut text_x = start_x;
    if let Some(ic) = icon {
        let icon_rect =
            Rect::from_center_size(Pos2::new(start_x + 7.0, rect.center().y), Vec2::splat(13.5));
        icons::paint(ui.painter(), icon_rect, ic, text_color);
        text_x += icon_space;
    }
    let text_pos = Pos2::new(text_x, rect.center().y - galley.size().y * 0.5);
    ui.painter().galley(text_pos, galley, text_color);
    response
}

/// 轻量图标按钮（用于标题栏、侧栏快捷开关、复制/清空等）。
pub fn icon_button(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Icon,
    size: Vec2,
    icon_size: f32,
    danger_hover: bool,
    tooltip: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let hover_t =
        ui.ctx()
            .animate_bool_with_time(response.id.with("ib_hover"), response.hovered(), 0.10);

    if hover_t > 0.01 {
        let hover_bg = if danger_hover {
            lerp_color(Color32::TRANSPARENT, p.danger, hover_t)
        } else {
            lerp_color(Color32::TRANSPARENT, with_alpha(p.text, 18), hover_t)
        };
        let radius = if danger_hover { 0 } else { RADIUS_SM };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius), hover_bg);
    }

    let icon_color = if danger_hover && hover_t > 0.5 {
        Color32::WHITE
    } else {
        lerp_color(p.text_secondary, p.text, hover_t)
    };
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(icon_size));
    icons::paint(ui.painter(), icon_rect, icon, icon_color);

    if !tooltip.is_empty() {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

/// 左侧导航栏项（带矢量图标 + Fluent 左侧激活竖条）。
pub fn nav_rail_item(
    ui: &mut egui::Ui,
    p: &Palette,
    selected: bool,
    icon: Icon,
    label: &str,
    dirty: bool,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);

    let hover_t = ui.ctx().animate_bool_with_time(
        response.id.with("nav_hover"),
        response.hovered() && !selected,
        0.12,
    );

    let bg = if selected {
        p.card
    } else if hover_t > 0.01 {
        lerp_color(Color32::TRANSPARENT, with_alpha(p.card, 160), hover_t)
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        let stroke = if selected {
            Stroke::new(1.0_f32, p.border)
        } else {
            Stroke::NONE
        };
        ui.painter()
            .rect(rect, CornerRadius::same(8), bg, stroke, StrokeKind::Inside);
    }

    if selected {
        let pill = Rect::from_min_max(
            Pos2::new(rect.left() + 3.0, rect.top() + 9.0),
            Pos2::new(rect.left() + 6.0, rect.bottom() - 9.0),
        );
        ui.painter()
            .rect_filled(pill, CornerRadius::same(2), p.accent);
    }

    let fg = if selected {
        p.text
    } else {
        lerp_color(p.text_secondary, p.text, hover_t)
    };
    let icon_color = if selected { p.accent } else { fg };

    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 23.0, rect.center().y),
        Vec2::splat(15.0),
    );
    icons::paint(ui.painter(), icon_rect, icon, icon_color);

    let font = if selected {
        egui::FontId::new(13.0, crate::ui::theme::bold_family())
    } else {
        egui::FontId::proportional(13.0)
    };
    ui.painter().text(
        Pos2::new(rect.left() + 40.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    if dirty {
        ui.painter()
            .circle_filled(Pos2::new(rect.right() - 12.0, rect.center().y), 3.3, p.warn);
    }

    let response = if dirty {
        response.on_hover_text("该栏目有未保存的修改")
    } else {
        response
    };
    ui.add_space(2.0);
    response.clicked()
}

/// 现代滑动开关（平滑动画 + 禁用态清晰）。
pub fn toggle_switch(
    ui: &mut egui::Ui,
    p: &Palette,
    on: &mut bool,
    enabled: bool,
) -> egui::Response {
    let desired = Vec2::new(38.0, 20.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(desired, sense);
    if enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            *on = !*on;
            response.mark_changed();
        }
    }

    let anim = ui.ctx().animate_bool_with_time(response.id, *on, 0.14);
    let track_color = if enabled {
        lerp_color(p.toggle_off, p.accent, anim)
    } else {
        p.border_strong
    };
    let radius = rect.height() * 0.5;
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius as u8), track_color);

    let knob_r = radius - 3.0;
    let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), anim);
    let knob_center = Pos2::new(knob_x, rect.center().y);
    let knob_color = if enabled {
        p.toggle_knob
    } else {
        with_alpha(p.toggle_knob, 210)
    };
    ui.painter().circle_filled(knob_center, knob_r, knob_color);
    response
}

/// 分段胶囊选择器（纯 Painter 固定 26px 总高，与同行的标题、徽章、搜索框严格共享同一垂直中线）。
pub fn segmented_control(
    ui: &mut egui::Ui,
    p: &Palette,
    id_salt: &str,
    selected: &mut usize,
    labels: &[&str],
) -> bool {
    let mut changed = false;
    let pad = 2.0_f32;
    let gap = 2.0_f32;
    let total_h = 26.0_f32;
    let item_h = total_h - pad * 2.0;

    // 预先排版各分段宽度，确保整体一次 allocate 精确 26px 高度，不引入 Frame 偏移
    let mut items = Vec::with_capacity(labels.len());
    let mut content_w = 0.0_f32;
    for (index, label) in labels.iter().enumerate() {
        let active = *selected == index;
        let font = if active {
            egui::FontId::new(11.5, crate::ui::theme::bold_family())
        } else {
            egui::FontId::proportional(11.5)
        };
        let fg = if active { p.text } else { p.text_secondary };
        let galley = ui.painter().layout_no_wrap((*label).to_string(), font, fg);
        let w = (galley.size().x + 16.0).max(38.0);
        if index > 0 {
            content_w += gap;
        }
        content_w += w;
        items.push((index, active, fg, galley, w));
    }

    let total_w = content_w + pad * 2.0;
    let (outer_rect, _) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::hover());

    ui.painter().rect(
        outer_rect,
        CornerRadius::same(6),
        p.subtle,
        Stroke::new(1.0_f32, p.border),
        StrokeKind::Inside,
    );

    let mut x = outer_rect.left() + pad;
    let y0 = outer_rect.top() + pad;
    for (index, active, fg, galley, w) in items {
        let rect = Rect::from_min_size(Pos2::new(x, y0), Vec2::new(w, item_h));
        let id = ui.make_persistent_id((id_salt, index));
        let resp = ui
            .interact(rect, id, Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let hover_t =
            ui.ctx()
                .animate_bool_with_time(id.with("hover"), resp.hovered() && !active, 0.10);
        if active {
            ui.painter().rect(
                rect,
                CornerRadius::same(4),
                p.card,
                Stroke::new(1.0_f32, p.border),
                StrokeKind::Inside,
            );
        } else if hover_t > 0.01 {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(4),
                with_alpha(p.card, (140.0 * hover_t) as u8),
            );
        }
        let text_pos = Pos2::new(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, fg);
        if resp.clicked() && !active {
            *selected = index;
            changed = true;
        }
        x += w + gap;
    }

    changed
}

/// 带 −/+ 步进与单位后缀的现代数字输入框（固定 124×28，纯栅格居中对齐，无嵌套双框）。
pub fn stepper_field(
    ui: &mut egui::Ui,
    p: &Palette,
    value: &mut i64,
    range: std::ops::RangeInclusive<i64>,
    step: i64,
    unit: &str,
) {
    let min_v = *range.start();
    let max_v = *range.end();
    const TOTAL_W: f32 = 124.0;
    const TOTAL_H: f32 = 28.0;
    const BTN_W: f32 = 26.0;

    let (outer_rect, _) = ui.allocate_exact_size(Vec2::new(TOTAL_W, TOTAL_H), Sense::hover());

    // 统一外框
    ui.painter().rect(
        outer_rect,
        CornerRadius::same(RADIUS_SM),
        p.input_bg,
        Stroke::new(1.0_f32, p.border),
        StrokeKind::Inside,
    );

    let minus_rect = Rect::from_min_max(
        Pos2::new(outer_rect.left() + 1.0, outer_rect.top() + 1.0),
        Pos2::new(outer_rect.left() + BTN_W, outer_rect.bottom() - 1.0),
    );
    let plus_rect = Rect::from_min_max(
        Pos2::new(outer_rect.right() - BTN_W, outer_rect.top() + 1.0),
        Pos2::new(outer_rect.right() - 1.0, outer_rect.bottom() - 1.0),
    );
    let center_rect = Rect::from_min_max(
        Pos2::new(minus_rect.right() + 1.0, outer_rect.top() + 2.0),
        Pos2::new(plus_rect.left() - 1.0, outer_rect.bottom() - 2.0),
    );

    // 左侧 − 按钮
    let can_dec = *value > min_v;
    let dec_resp = ui.interact(
        minus_rect,
        ui.id().with(("stepper_dec", outer_rect.min.y as i32)),
        if can_dec {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if can_dec {
        let dec_resp = dec_resp.on_hover_cursor(CursorIcon::PointingHand);
        if dec_resp.hovered() {
            ui.painter().rect_filled(
                minus_rect,
                CornerRadius {
                    nw: RADIUS_SM.saturating_sub(1),
                    sw: RADIUS_SM.saturating_sub(1),
                    ne: 0,
                    se: 0,
                },
                p.subtle,
            );
        }
        if dec_resp.clicked() {
            *value = (*value - step).clamp(min_v, max_v);
        }
    }
    ui.painter().text(
        minus_rect.center(),
        egui::Align2::CENTER_CENTER,
        "−",
        egui::FontId::proportional(14.0),
        if can_dec {
            p.text_secondary
        } else {
            with_alpha(p.text_muted, 90)
        },
    );

    // 右侧 + 按钮
    let can_inc = *value < max_v;
    let inc_resp = ui.interact(
        plus_rect,
        ui.id().with(("stepper_inc", outer_rect.min.y as i32)),
        if can_inc {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if can_inc {
        let inc_resp = inc_resp.on_hover_cursor(CursorIcon::PointingHand);
        if inc_resp.hovered() {
            ui.painter().rect_filled(
                plus_rect,
                CornerRadius {
                    nw: 0,
                    sw: 0,
                    ne: RADIUS_SM.saturating_sub(1),
                    se: RADIUS_SM.saturating_sub(1),
                },
                p.subtle,
            );
        }
        if inc_resp.clicked() {
            *value = (*value + step).clamp(min_v, max_v);
        }
    }
    ui.painter().text(
        plus_rect.center(),
        egui::Align2::CENTER_CENTER,
        "+",
        egui::FontId::proportional(14.0),
        if can_inc {
            p.text_secondary
        } else {
            with_alpha(p.text_muted, 90)
        },
    );

    // 两侧细分隔线
    let div_stroke = Stroke::new(1.0_f32, p.divider);
    ui.painter().line_segment(
        [
            Pos2::new(minus_rect.right(), outer_rect.top() + 4.0),
            Pos2::new(minus_rect.right(), outer_rect.bottom() - 4.0),
        ],
        div_stroke,
    );
    ui.painter().line_segment(
        [
            Pos2::new(plus_rect.left(), outer_rect.top() + 4.0),
            Pos2::new(plus_rect.left(), outer_rect.bottom() - 4.0),
        ],
        div_stroke,
    );

    // 中央数值 + 单位（去内框透明融入外框，精确居中）
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(center_rect).layout(
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
    ));
    let vis = &mut child.visuals_mut().widgets;
    vis.inactive.weak_bg_fill = Color32::TRANSPARENT;
    vis.inactive.bg_fill = Color32::TRANSPARENT;
    vis.inactive.bg_stroke = Stroke::NONE;
    vis.hovered.weak_bg_fill = p.subtle;
    vis.hovered.bg_stroke = Stroke::NONE;
    vis.active.weak_bg_fill = p.subtle;
    vis.active.bg_stroke = Stroke::NONE;

    let suffix = format!(" {unit}");
    let drag = egui::DragValue::new(value)
        .range(range)
        .speed(step as f64 * 0.35)
        .suffix(suffix);
    child.add(drag);
}

/// 带未保存圆点提示的保存按钮。
pub fn save_button(ui: &mut egui::Ui, p: &Palette, dirty: bool) -> egui::Response {
    if dirty {
        let resp = styled_button(ui, p, Some(Icon::Check), "保存设置", true);
        let dot = Pos2::new(resp.rect.right() - 4.0, resp.rect.top() + 4.0);
        ui.painter().circle_filled(dot, 4.0, p.warn);
        resp
    } else {
        secondary_button(ui, p, Some(Icon::Check), "保存设置")
    }
}

/// 可拖拽纵向分隔条（调整左右宽度），返回 `(水平拖拽增量 dx, 是否双击恢复默认)`。
pub fn vertical_splitter(ui: &mut egui::Ui, p: &Palette, height: f32) -> (f32, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(10.0, height), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let active = resp.hovered() || resp.dragged();
    let grip_color = if active {
        p.accent
    } else {
        with_alpha(p.border_strong, 150)
    };
    let grip = Rect::from_center_size(
        rect.center(),
        Vec2::new(
            if active { 3.0 } else { 2.0 },
            if active { 42.0 } else { 30.0 },
        ),
    );
    ui.painter()
        .rect_filled(grip, CornerRadius::same(2), grip_color);
    let dx = if resp.dragged() {
        resp.drag_delta().x
    } else {
        0.0
    };
    let dbl = resp.double_clicked();
    resp.on_hover_text("左右拖拽调整宽度比例 · 双击恢复默认");
    (dx, dbl)
}

/// 卡片内部带细竖线的可拖拽纵向分隔条（用于宽屏稳定性面板左右栏拖拽）。
pub fn inner_vertical_splitter(ui: &mut egui::Ui, p: &Palette, height: f32) -> (f32, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(18.0, height), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let active = resp.hovered() || resp.dragged();
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        Stroke::new(1.0_f32, if active { p.accent_border } else { p.divider }),
    );
    let grip_color = if active {
        p.accent
    } else {
        with_alpha(p.border_strong, 130)
    };
    let grip = Rect::from_center_size(
        rect.center(),
        Vec2::new(
            if active { 3.0 } else { 2.0 },
            if active { 40.0 } else { 24.0 },
        ),
    );
    ui.painter()
        .rect_filled(grip, CornerRadius::same(2), grip_color);
    let dx = if resp.dragged() {
        resp.drag_delta().x
    } else {
        0.0
    };
    let dbl = resp.double_clicked();
    resp.on_hover_text("左右拖拽调整双栏比例 · 双击恢复默认");
    (dx, dbl)
}

/// 可拖拽横向分隔条（调整图表与日志上下高度），返回 `(纵向拖拽增量 dy, 是否双击恢复默认)`。
pub fn horizontal_splitter(ui: &mut egui::Ui, p: &Palette, width: f32) -> (f32, bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 10.0), Sense::click_and_drag());
    let resp = resp.on_hover_cursor(CursorIcon::ResizeVertical);
    let active = resp.hovered() || resp.dragged();
    let grip_color = if active {
        p.accent
    } else {
        with_alpha(p.border_strong, 150)
    };
    let grip = Rect::from_center_size(
        rect.center(),
        Vec2::new(
            if active { 54.0 } else { 38.0 },
            if active { 3.0 } else { 2.0 },
        ),
    );
    ui.painter()
        .rect_filled(grip, CornerRadius::same(2), grip_color);
    let dy = if resp.dragged() {
        resp.drag_delta().y
    } else {
        0.0
    };
    let dbl = resp.double_clicked();
    resp.on_hover_text("上下拖拽调整面板与日志高度 · 双击恢复默认");
    (dy, dbl)
}
