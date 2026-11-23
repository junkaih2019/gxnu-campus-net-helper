//! 统一 GXNU 品牌与应用图标绘制（「B+B 晴岚玉徽无圈版」矢量超采样光栅器）：
//! - 桌面 / 窗口 / `.exe` 图标（`tray_state = None`）：
//!   晴岚蓝青渐变圆角方框（`#38BDF8` → `#2563EB` → `#0D9488`）+ 1932 师大校徽线面同构山城图腾：
//!   * 前景：暖羊脂玉实心靖江王城正门 + 左右齐平飞檐翼廊 + 通长双层汉白玉丹陛石阶台基
//!   * 中景：通透青绿薄釉山腹 + 与城门同色的温润粗线条群山（左伴双峰、中左独秀主峰、右叠双层奇峰）直连飞檐
//!   * 峰巅：校徽标志性的攒尖飞檐「独秀亭」与左肩伴亭扎根山脊
//!   * 右上天际：一轮暖金朝阳悬于群峰之上
//! - 系统托盘 5 态图标（`tray_state = Some(state)`，托盘形态 B）：
//!   保留同款晴岚玉徽圆角方框，右上「日/月」放大并随网络状态呈现 5 态色彩（暂停态化作静默银白弯月）。

use std::sync::OnceLock;

use crate::models::MonitorState;

type Pt = (f32, f32);

#[derive(Clone)]
struct Poly {
    pts: Vec<Pt>,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

impl Poly {
    fn from_pts(pts: Vec<Pt>) -> Self {
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for &(x, y) in &pts {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        Self {
            pts,
            min_x,
            max_x,
            min_y,
            max_y,
        }
    }
}

struct PathBuilder {
    polys: Vec<Poly>,
    cur: Vec<Pt>,
}

impl PathBuilder {
    fn new() -> Self {
        Self {
            polys: Vec::new(),
            cur: Vec::new(),
        }
    }

    fn move_to(&mut self, x: f32, y: f32) {
        if self.cur.len() >= 2 {
            let pts = std::mem::take(&mut self.cur);
            self.polys.push(Poly::from_pts(pts));
        } else {
            self.cur.clear();
        }
        self.cur.push((x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.cur.push((x, y));
    }

    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) {
        let (x0, y0) = self.cur.last().copied().unwrap_or((x1, y1));
        const STEPS: usize = 12;
        for i in 1..=STEPS {
            let t = i as f32 / STEPS as f32;
            let mt = 1.0 - t;
            let b0 = mt * mt * mt;
            let b1 = 3.0 * mt * mt * t;
            let b2 = 3.0 * mt * t * t;
            let b3 = t * t * t;
            self.cur.push((
                b0 * x0 + b1 * x1 + b2 * x2 + b3 * x3,
                b0 * y0 + b1 * y1 + b2 * y2 + b3 * y3,
            ));
        }
    }

    fn upper_semicircle_ccw(&mut self, cx: f32, cy: f32, r: f32, steps: usize) {
        // SVG `M (cx+r, cy) A r r 0 0 0 (cx-r, cy)` in Y-down screen coords sweeps through (cx, cy - r)
        for i in 1..=steps {
            let theta = std::f32::consts::PI * (i as f32 / steps as f32);
            let x = cx + r * theta.cos();
            let y = cy - r * theta.sin();
            self.cur.push((x, y));
        }
    }

    fn close(&mut self) {
        if let Some(&first) = self.cur.first() {
            if self.cur.last().copied() != Some(first) {
                self.cur.push(first);
            }
        }
        if self.cur.len() >= 3 {
            let pts = std::mem::take(&mut self.cur);
            self.polys.push(Poly::from_pts(pts));
        } else {
            self.cur.clear();
        }
    }

    fn finish(mut self) -> Vec<Poly> {
        if self.cur.len() >= 2 {
            let pts = std::mem::take(&mut self.cur);
            self.polys.push(Poly::from_pts(pts));
        }
        self.polys
    }
}

struct EmblemGeometry {
    gate_solid: Vec<Poly>,
    neck_crease: Poly,
    wing_creases: [Poly; 2],
    terrace_bands: [Poly; 2],
    royal_stairs: Poly,
    royal_treads: [Poly; 3],
    pavilions: Vec<Poly>,
    duxiu_fill: Vec<Poly>,
    left_fill: Vec<Poly>,
    rf_fill: Vec<Poly>,
    rr_fill: Vec<Poly>,
    duxiu_stroke: Vec<Poly>,
    left_stroke: Vec<Poly>,
    rf_stroke: Vec<Poly>,
    rr_stroke: Vec<Poly>,
    duxiu_ridges: Vec<Poly>,
}

fn geometry() -> &'static EmblemGeometry {
    static GEOM: OnceLock<EmblemGeometry> = OnceLock::new();
    GEOM.get_or_init(build_geometry)
}

fn rect_poly(x0: f32, y0: f32, x1: f32, y1: f32) -> Poly {
    Poly::from_pts(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)])
}

fn build_geometry() -> EmblemGeometry {
    // 1. 王城正门 + 左右齐平翼廊 + 通长双层汉白玉石阶台基（含中央圆拱门洞镂空）
    let mut g = PathBuilder::new();
    g.move_to(256.0, 258.0);
    g.line_to(290.0, 258.0);
    g.cubic_to(296.0, 258.0, 300.0, 251.0, 305.0, 251.0);
    g.cubic_to(310.0, 251.0, 314.0, 258.0, 321.0, 263.0);
    g.cubic_to(332.0, 272.0, 346.0, 274.0, 357.0, 269.0);
    g.cubic_to(362.0, 267.0, 366.0, 271.0, 364.0, 277.0);
    g.cubic_to(359.0, 288.0, 345.0, 296.0, 326.0, 299.0);
    g.line_to(326.0, 313.0);
    g.cubic_to(345.0, 316.0, 366.0, 318.0, 382.0, 311.0);
    g.cubic_to(388.0, 308.0, 393.0, 313.0, 390.0, 320.0);
    g.cubic_to(385.0, 331.0, 371.0, 339.0, 353.0, 342.0);
    g.line_to(354.0, 349.0);
    g.line_to(402.0, 349.0);
    g.cubic_to(413.0, 349.0, 422.0, 346.0, 430.0, 340.0);
    g.cubic_to(435.0, 337.0, 439.0, 341.0, 436.0, 347.0);
    g.cubic_to(431.0, 356.0, 421.0, 361.0, 409.0, 362.0);
    g.line_to(409.0, 382.0);
    g.line_to(356.0, 382.0);
    g.line_to(356.0, 393.0);
    g.line_to(361.0, 393.0);
    g.cubic_to(363.5, 393.0, 365.0, 394.5, 365.0, 397.0);
    g.line_to(365.0, 401.0);
    g.cubic_to(365.0, 403.2, 363.2, 405.0, 361.0, 405.0);
    g.line_to(151.0, 405.0);
    g.cubic_to(148.8, 405.0, 147.0, 403.2, 147.0, 401.0);
    g.line_to(147.0, 397.0);
    g.cubic_to(147.0, 394.5, 148.5, 393.0, 151.0, 393.0);
    g.line_to(156.0, 393.0);
    g.line_to(156.0, 382.0);
    g.line_to(103.0, 382.0);
    g.line_to(103.0, 362.0);
    g.cubic_to(91.0, 361.0, 81.0, 356.0, 76.0, 347.0);
    g.cubic_to(73.0, 341.0, 77.0, 337.0, 82.0, 340.0);
    g.cubic_to(90.0, 346.0, 99.0, 349.0, 110.0, 349.0);
    g.line_to(158.0, 349.0);
    g.line_to(159.0, 342.0);
    g.cubic_to(141.0, 339.0, 127.0, 331.0, 122.0, 320.0);
    g.cubic_to(119.0, 313.0, 124.0, 308.0, 130.0, 311.0);
    g.cubic_to(146.0, 318.0, 167.0, 316.0, 186.0, 313.0);
    g.line_to(186.0, 299.0);
    g.cubic_to(167.0, 296.0, 153.0, 288.0, 148.0, 277.0);
    g.cubic_to(146.0, 271.0, 150.0, 267.0, 155.0, 269.0);
    g.cubic_to(166.0, 274.0, 180.0, 272.0, 191.0, 263.0);
    g.cubic_to(198.0, 258.0, 202.0, 251.0, 207.0, 251.0);
    g.cubic_to(212.0, 251.0, 216.0, 258.0, 222.0, 258.0);
    g.close();
    // 中央圆拱门洞镂空（稳立于 y=382 台基之上）
    g.move_to(231.0, 382.0);
    g.line_to(281.0, 382.0);
    g.line_to(281.0, 356.0);
    g.upper_semicircle_ccw(256.0, 356.0, 25.0, 18);
    g.close();

    // 2. 独秀峰顶「独秀亭 + 左肩伴亭」
    let mut pav = PathBuilder::new();
    pav.move_to(231.0, 68.0);
    pav.cubic_to(233.0, 74.0, 239.0, 81.0, 247.0, 85.0);
    pav.cubic_to(251.0, 87.0, 255.0, 86.0, 257.0, 84.0);
    pav.cubic_to(259.0, 83.0, 260.0, 86.0, 258.0, 89.0);
    pav.cubic_to(254.0, 93.0, 248.0, 94.0, 242.0, 94.0);
    pav.line_to(242.0, 112.0);
    pav.line_to(220.0, 112.0);
    pav.line_to(220.0, 94.0);
    pav.cubic_to(214.0, 94.0, 208.0, 93.0, 204.0, 89.0);
    pav.cubic_to(202.0, 86.0, 203.0, 83.0, 205.0, 84.0);
    pav.cubic_to(207.0, 86.0, 211.0, 87.0, 215.0, 85.0);
    pav.cubic_to(223.0, 81.0, 229.0, 74.0, 231.0, 68.0);
    pav.close();
    pav.move_to(227.5, 94.0);
    pav.line_to(234.5, 94.0);
    pav.line_to(234.5, 103.0);
    pav.line_to(227.5, 103.0);
    pav.close();
    // 左肩伴亭
    pav.move_to(203.0, 91.0);
    pav.cubic_to(205.0, 95.0, 209.0, 99.0, 214.0, 101.5);
    pav.cubic_to(216.5, 102.5, 218.5, 102.0, 219.5, 101.0);
    pav.cubic_to(221.0, 100.5, 221.5, 102.8, 219.8, 104.8);
    pav.cubic_to(217.0, 107.5, 212.5, 108.5, 209.0, 108.5);
    pav.line_to(209.0, 122.0);
    pav.line_to(197.0, 122.0);
    pav.line_to(197.0, 108.5);
    pav.cubic_to(193.5, 108.5, 189.0, 107.5, 186.2, 104.8);
    pav.cubic_to(184.5, 102.8, 185.0, 100.5, 186.5, 101.0);
    pav.cubic_to(187.5, 102.0, 189.5, 102.5, 192.0, 101.5);
    pav.cubic_to(197.0, 99.0, 201.0, 95.0, 203.0, 91.0);
    pav.close();
    pav.move_to(200.5, 108.5);
    pav.line_to(205.5, 108.5);
    pav.line_to(205.5, 114.5);
    pav.line_to(200.5, 114.5);
    pav.close();

    // 3. 独秀峰山腹与同色连笔山脊轮廓
    let mut df = PathBuilder::new();
    df.move_to(140.0, 326.0);
    df.line_to(143.0, 254.0);
    df.cubic_to(146.0, 214.0, 152.0, 188.0, 160.0, 174.0);
    df.cubic_to(165.0, 165.0, 173.0, 162.0, 179.0, 150.0);
    df.cubic_to(186.0, 126.0, 196.0, 112.0, 208.0, 107.0);
    df.cubic_to(217.0, 103.0, 229.0, 102.0, 240.0, 104.0);
    df.cubic_to(252.0, 106.0, 262.0, 114.0, 270.0, 126.0);
    df.cubic_to(279.0, 140.0, 287.0, 164.0, 294.0, 198.0);
    df.cubic_to(299.0, 224.0, 302.0, 252.0, 304.0, 278.0);
    df.line_to(304.0, 326.0);
    df.close();

    let mut ds = PathBuilder::new();
    ds.move_to(141.0, 315.0);
    ds.line_to(143.0, 254.0);
    ds.cubic_to(146.0, 214.0, 152.0, 188.0, 160.0, 174.0);
    ds.cubic_to(165.0, 165.0, 173.0, 162.0, 179.0, 150.0);
    ds.cubic_to(186.0, 126.0, 196.0, 112.0, 208.0, 107.0);
    ds.cubic_to(217.0, 103.0, 229.0, 102.0, 240.0, 104.0);
    ds.cubic_to(252.0, 106.0, 262.0, 114.0, 270.0, 126.0);
    ds.cubic_to(279.0, 140.0, 287.0, 164.0, 294.0, 198.0);
    ds.cubic_to(299.0, 222.0, 302.0, 248.0, 303.0, 270.0);

    let mut dr = PathBuilder::new();
    dr.move_to(179.0, 150.0);
    dr.cubic_to(173.0, 180.0, 169.0, 216.0, 167.0, 258.0);
    dr.move_to(251.0, 108.0);
    dr.cubic_to(262.0, 126.0, 270.0, 154.0, 275.0, 190.0);

    // 4. 左翼双伴峰
    let mut lf = PathBuilder::new();
    lf.move_to(96.0, 354.0);
    lf.cubic_to(100.0, 314.0, 106.0, 278.0, 114.0, 278.0);
    lf.cubic_to(120.0, 278.0, 124.0, 292.0, 127.0, 294.0);
    lf.cubic_to(131.0, 268.0, 135.0, 244.0, 141.0, 244.0);
    lf.line_to(144.0, 260.0);
    lf.line_to(144.0, 354.0);
    lf.close();

    let mut ls = PathBuilder::new();
    ls.move_to(97.0, 348.0);
    ls.cubic_to(100.0, 312.0, 106.0, 278.0, 114.0, 278.0);
    ls.cubic_to(120.0, 278.0, 124.0, 292.0, 127.0, 294.0);
    ls.cubic_to(131.0, 268.0, 135.0, 246.0, 140.0, 246.0);
    ls.line_to(143.0, 256.0);

    // 5. 右翼前排双峰与后排叠嶂奇峰
    let mut rff = PathBuilder::new();
    rff.move_to(302.0, 354.0);
    rff.line_to(302.0, 282.0);
    rff.cubic_to(314.0, 262.0, 326.0, 242.0, 339.0, 242.0);
    rff.cubic_to(352.0, 242.0, 361.0, 268.0, 371.0, 270.0);
    rff.cubic_to(381.0, 268.0, 392.0, 236.0, 403.0, 236.0);
    rff.cubic_to(412.0, 236.0, 417.0, 294.0, 417.0, 354.0);
    rff.close();

    let mut rfs = PathBuilder::new();
    rfs.move_to(303.0, 278.0);
    rfs.cubic_to(314.0, 260.0, 326.0, 242.0, 339.0, 242.0);
    rfs.cubic_to(352.0, 242.0, 361.0, 268.0, 371.0, 270.0);
    rfs.cubic_to(381.0, 268.0, 392.0, 236.0, 403.0, 236.0);
    rfs.cubic_to(411.0, 236.0, 416.0, 288.0, 415.0, 348.0);

    let mut rrf = PathBuilder::new();
    rrf.move_to(298.0, 250.0);
    rrf.cubic_to(315.0, 220.0, 334.0, 198.0, 352.0, 198.0);
    rrf.cubic_to(365.0, 198.0, 374.0, 220.0, 382.0, 222.0);
    rrf.cubic_to(390.0, 219.0, 396.0, 210.0, 403.0, 210.0);
    rrf.cubic_to(409.0, 210.0, 412.0, 226.0, 410.0, 250.0);
    rrf.cubic_to(407.0, 240.0, 405.0, 236.0, 403.0, 236.0);
    rrf.cubic_to(392.0, 236.0, 381.0, 268.0, 371.0, 270.0);
    rrf.cubic_to(361.0, 268.0, 352.0, 242.0, 339.0, 242.0);
    rrf.cubic_to(326.0, 242.0, 314.0, 260.0, 303.0, 278.0);
    rrf.close();

    let mut rrs = PathBuilder::new();
    rrs.move_to(298.0, 248.0);
    rrs.cubic_to(315.0, 220.0, 334.0, 198.0, 352.0, 198.0);
    rrs.cubic_to(365.0, 198.0, 374.0, 220.0, 382.0, 222.0);
    rrs.cubic_to(390.0, 219.0, 396.0, 210.0, 403.0, 210.0);
    rrs.cubic_to(408.0, 210.0, 411.0, 224.0, 409.0, 244.0);

    EmblemGeometry {
        gate_solid: g.finish(),
        neck_crease: rect_poly(186.0, 299.0, 326.0, 310.0),
        wing_creases: [
            rect_poly(103.0, 362.0, 158.0, 369.0),
            rect_poly(354.0, 362.0, 409.0, 369.0),
        ],
        terrace_bands: [
            rect_poly(156.0, 382.0, 356.0, 386.0),
            rect_poly(151.0, 393.0, 361.0, 396.5),
        ],
        royal_stairs: Poly::from_pts(vec![
            (231.0, 382.0),
            (281.0, 382.0),
            (285.0, 405.0),
            (227.0, 405.0),
            (231.0, 382.0),
        ]),
        royal_treads: [
            Poly::from_pts(vec![(232.0, 388.5), (280.0, 388.5)]),
            Poly::from_pts(vec![(230.0, 394.5), (282.0, 394.5)]),
            Poly::from_pts(vec![(229.0, 400.0), (283.0, 400.0)]),
        ],
        pavilions: pav.finish(),
        duxiu_fill: df.finish(),
        left_fill: lf.finish(),
        rf_fill: rff.finish(),
        rr_fill: rrf.finish(),
        duxiu_stroke: ds.finish(),
        left_stroke: ls.finish(),
        rf_stroke: rfs.finish(),
        rr_stroke: rrs.finish(),
        duxiu_ridges: dr.finish(),
    }
}

/// 渲染 RGBA8 像素数据。
/// - `tray_state = None`：纯净「B · 晴岚玉徽无圈版」品牌主图标（用于窗口标题栏、主窗口左上角、`.exe`）
/// - `tray_state = Some(state)`：系统托盘「B · 圆角方框 5 态」状态图标（右上日月随状态变色，暂停化作弯月）
pub fn render_icon_rgba(tray_state: Option<MonitorState>, size: u32) -> Vec<u8> {
    if size == 0 {
        return Vec::new();
    }
    let geom = geometry();
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let scale_512 = 512.0 / size as f32;

    // 4×4 超采样抗锯齿（大尺寸如 >96px 用 2×2 兼顾速度与细腻度）
    let offsets: &[f32] = if size <= 96 {
        &[0.125, 0.375, 0.625, 0.875]
    } else {
        &[0.25, 0.75]
    };
    let samples_n = (offsets.len() * offsets.len()) as f32;

    for py in 0..size {
        for px in 0..size {
            let mut acc_r = 0.0_f32;
            let mut acc_g = 0.0_f32;
            let mut acc_b = 0.0_f32;
            let mut acc_a = 0.0_f32;

            for &oy in offsets {
                let sy = (py as f32 + oy) * scale_512;
                for &ox in offsets {
                    let sx = (px as f32 + ox) * scale_512;
                    let (r, g, b, a) = sample_qinglan_jade_icon(sx, sy, tray_state, geom);
                    acc_r += r * a;
                    acc_g += g * a;
                    acc_b += b * a;
                    acc_a += a;
                }
            }

            let out_a = (acc_a / samples_n).clamp(0.0, 1.0);
            let idx = ((py * size + px) * 4) as usize;
            if out_a > 0.001 {
                rgba[idx] = (acc_r / acc_a).clamp(0.0, 255.0).round() as u8;
                rgba[idx + 1] = (acc_g / acc_a).clamp(0.0, 255.0).round() as u8;
                rgba[idx + 2] = (acc_b / acc_a).clamp(0.0, 255.0).round() as u8;
                rgba[idx + 3] = (out_a * 255.0).round() as u8;
            }
        }
    }
    rgba
}

#[inline]
fn sample_qinglan_jade_icon(
    sx: f32,
    sy: f32,
    tray_state: Option<MonitorState>,
    geom: &EmblemGeometry,
) -> (f32, f32, f32, f32) {
    // 512 画布满幅圆角方框：x,y ∈ [0, 512], rx = 116（与 Windows 托盘/任务栏主流应用 100% 等大对齐）
    let box_sdf = sd_rounded_box(sx - 256.0, sy - 256.0, 256.0, 256.0, 116.0);
    if box_sdf > 1.5 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let cov = smoothstep(1.5, -1.5, box_sdf);

    let is_offline = tray_state == Some(MonitorState::Offline);
    let is_paused = matches!(
        tray_state,
        Some(MonitorState::Paused | MonitorState::Stopped)
    );

    // 1. 背景渐变色板
    let (bg_c0, bg_c1, bg_c2) = match tray_state {
        None | Some(MonitorState::Checking) => (
            (56.0, 189.0, 248.0), // #38BDF8
            (37.0, 99.0, 235.0),  // #2563EB
            (13.0, 148.0, 136.0), // #0D9488
        ),
        Some(MonitorState::Online) => (
            (56.0, 189.0, 248.0), // #38BDF8
            (37.0, 99.0, 235.0),  // #2563EB
            (29.0, 78.0, 216.0),  // #1D4ED8（纯蓝衬托右上翡翠绿日，对比更鲜明）
        ),
        Some(MonitorState::Authenticating | MonitorState::Verifying | MonitorState::Suspect) => (
            (56.0, 189.0, 248.0),
            (37.0, 99.0, 235.0),
            (30.0, 64.0, 175.0),
        ),
        Some(MonitorState::Offline) => (
            (30.0, 58.0, 138.0), // #1E3A8A 暮夜深蓝衬托朱砂绯红告警月
            (23.0, 37.0, 84.0),  // #172554
            (15.0, 23.0, 42.0),  // #0F172A
        ),
        Some(MonitorState::Paused | MonitorState::Stopped) => (
            (100.0, 116.0, 139.0), // #64748B 静默石板灰
            (71.0, 85.0, 105.0),   // #475569
            (51.0, 65.0, 85.0),    // #334155
        ),
    };

    let bg_t = project_grad(sx, sy, 36.0, 8.0, 476.0, 504.0);
    let (mut cr, mut cg, mut cb) = if bg_t < 0.48 {
        lerp3(bg_c0, bg_c1, bg_t / 0.48)
    } else {
        lerp3(bg_c1, bg_c2, (bg_t - 0.48) / 0.52)
    };

    // 2. 右上日月天际柔光晕
    if !is_paused {
        let glow_d = (sx - 375.0).hypot(sy - 134.0) / 210.0;
        if glow_d < 1.0 {
            let (gr, gg, gb, ga) = match tray_state {
                None
                | Some(
                    MonitorState::Authenticating | MonitorState::Verifying | MonitorState::Suspect,
                ) => (254.0, 240.0, 138.0, 0.58),
                Some(MonitorState::Online) => (110.0, 231.0, 183.0, 0.46),
                Some(MonitorState::Checking) => (224.0, 242.0, 254.0, 0.48),
                Some(MonitorState::Offline) => (253.0, 164.0, 175.0, 0.48),
                Some(MonitorState::Paused | MonitorState::Stopped) => (0.0, 0.0, 0.0, 0.0),
            };
            let w = (1.0 - glow_d).powf(1.85) * ga;
            cr = mix(cr, gr, w);
            cg = mix(cg, gg, w);
            cb = mix(cb, gb, w);
        }
    }

    // 随外框 238→256 等比放大内部山城图腾（1.08 → 1.165），保持饱满视觉占比
    let gx = (sx - 256.0) / 1.165 + 256.0;
    let gy = (sy - 267.0) / 1.165 + 248.0;

    // 3. 右上太阳 / 5 态圆日与弦月
    let orb_sdf = if tray_state.is_none() {
        (gx - 360.0).hypot(gy - 134.0) - 35.0
    } else if is_paused {
        // 暂停态：化作静默银白弦月（弯月）
        let d_outer = (gx - 358.0).hypot(gy - 134.0) - 48.0;
        let d_cut = (gx - 376.0).hypot(gy - 118.0) - 38.5;
        d_outer.max(-d_cut)
    } else {
        // 托盘活跃态：稍放大圆日/满月半径（r=50），确保 16px/24px 任务栏一眼看清状态色
        (gx - 358.0).hypot(gy - 134.0) - 50.0
    };

    if orb_sdf < 1.2 {
        let oa = smoothstep(1.2, -1.2, orb_sdf);
        let ot = project_grad(gx, gy, 322.0, 92.0, 396.0, 176.0);
        let (or_c, og_c, ob_c) = match tray_state {
            None => lerp3_3(
                (255.0, 251.0, 235.0),
                (253.0, 224.0, 71.0),
                (245.0, 158.0, 11.0),
                ot,
            ),
            Some(MonitorState::Online) => lerp3_3(
                (220.0, 252.0, 231.0),
                (74.0, 222.0, 128.0),
                (22.0, 163.0, 74.0),
                ot,
            ),
            Some(MonitorState::Checking) => lerp3_3(
                (255.0, 255.0, 255.0),
                (224.0, 242.0, 254.0),
                (56.0, 189.0, 248.0),
                ot,
            ),
            Some(
                MonitorState::Authenticating | MonitorState::Verifying | MonitorState::Suspect,
            ) => lerp3_3(
                (255.0, 251.0, 235.0),
                (250.0, 204.0, 21.0),
                (234.0, 88.0, 12.0),
                ot,
            ),
            Some(MonitorState::Offline) => lerp3_3(
                (255.0, 228.0, 230.0),
                (251.0, 113.0, 133.0),
                (220.0, 38.0, 38.0),
                ot,
            ),
            Some(MonitorState::Paused | MonitorState::Stopped) => {
                lerp3((248.0, 250.0, 252.0), (203.0, 213.0, 225.0), ot)
            }
        };
        cr = mix(cr, or_c, oa);
        cg = mix(cg, og_c, oa);
        cb = mix(cb, ob_c, oa);
    }

    // 4. 城门与群山底部轻柔投影（仅作用于图腾整体外侧下方）
    if (70.0..=444.0).contains(&gx)
        && (250.0..=422.0).contains(&gy)
        && point_in_even_odd(gx, gy - 10.0, &geom.gate_solid)
    {
        let sh = if is_offline { 0.34 } else { 0.18 };
        cr = mix(cr, 8.0, sh);
        cg = mix(cg, 24.0, sh);
        cb = mix(cb, 42.0, sh);
    }

    // 5. 山腹通透青绿薄釉（左伴峰、右前峰、右后远峰、独秀主峰）
    if (90.0..=422.0).contains(&gx) && (96.0..=360.0).contains(&gy) {
        let in_duxiu = point_in_even_odd(gx, gy, &geom.duxiu_fill);
        let in_main_mtn = in_duxiu
            || point_in_even_odd(gx, gy, &geom.left_fill)
            || point_in_even_odd(gx, gy, &geom.rf_fill);
        let in_rr = !in_main_mtn && point_in_even_odd(gx, gy, &geom.rr_fill);

        if in_main_mtn || in_rr {
            let gt = ((gy - 96.0) / (354.0 - 96.0)).clamp(0.0, 1.0);
            let (mr, mg, mb, ma) = if is_paused {
                let (c_r, c_g, c_b) = lerp3((226.0, 232.0, 240.0), (148.0, 163.0, 184.0), gt);
                (c_r, c_g, c_b, if in_rr { 0.22 } else { 0.30 })
            } else {
                let (c_r, c_g, c_b) = lerp3((167.0, 243.0, 208.0), (16.0, 185.0, 129.0), gt);
                let base_a = mix(0.48, 0.26, gt);
                (c_r, c_g, c_b, if in_rr { base_a * 0.76 } else { base_a })
            };
            cr = mix(cr, mr, ma);
            cg = mix(cg, mg, ma);
            cb = mix(cb, mb, ma);
        }

        // 6. 独秀峰顶「独秀亭 + 左肩伴亭」（仅绘制在 duxiu_fill 上方，使亭柱自然扎根入山脊线）
        if !in_duxiu
            && (180.0..=264.0).contains(&gx)
            && (65.0..=124.0).contains(&gy)
            && point_in_even_odd(gx, gy, &geom.pavilions)
        {
            cr = 255.0;
            cg = 253.0;
            cb = 247.0;
        }
    }

    // 7. 师大校徽同色温润山脉粗线条（直接焊入王城飞檐）
    if (88.0..=426.0).contains(&gx) && (94.0..=358.0).contains(&gy) {
        let mut stroke_a = 0.0_f32;

        let d_rr = dist_to_polys(gx, gy, &geom.rr_stroke);
        if d_rr < 5.0 {
            stroke_a = stroke_a.max(smoothstep(5.0, 3.5, d_rr) * 0.86);
        }
        let d_side =
            dist_to_polys(gx, gy, &geom.left_stroke).min(dist_to_polys(gx, gy, &geom.rf_stroke));
        if d_side < 5.0 {
            stroke_a = stroke_a.max(smoothstep(5.0, 3.5, d_side));
        }
        let d_duxiu = dist_to_polys(gx, gy, &geom.duxiu_stroke);
        if d_duxiu < 5.4 {
            stroke_a = stroke_a.max(smoothstep(5.4, 3.8, d_duxiu));
        }
        let d_ridge = dist_to_polys(gx, gy, &geom.duxiu_ridges);
        if d_ridge < 3.9 {
            stroke_a = stroke_a.max(smoothstep(3.9, 2.4, d_ridge) * 0.82);
        }

        if stroke_a > 0.001 {
            cr = mix(cr, 255.0, stroke_a);
            cg = mix(cg, 253.0, stroke_a);
            cb = mix(cb, 247.0, stroke_a);
        }
    }

    // 8. 前景实心王城正门 + 左右齐平翼廊 + 通长双层汉白玉丹陛石阶台基
    if (70.0..=442.0).contains(&gx)
        && (248.0..=408.0).contains(&gy)
        && point_in_even_odd(gx, gy, &geom.gate_solid)
    {
        let gt = ((gy - 252.0) / (405.0 - 252.0)).clamp(0.0, 1.0);
        let (mut gr, mut gg, mut gb) = if is_paused {
            lerp3((255.0, 255.0, 255.0), (226.0, 232.0, 240.0), gt)
        } else if gt < 0.68 {
            lerp3((255.0, 255.0, 255.0), (255.0, 251.0, 235.0), gt / 0.68)
        } else {
            lerp3(
                (255.0, 251.0, 235.0),
                (254.0, 243.0, 199.0),
                (gt - 0.68) / 0.32,
            )
        };

        if !is_paused {
            if point_in_poly(gx, gy, &geom.neck_crease) {
                gr = mix(gr, 217.0, 0.14);
                gg = mix(gg, 119.0, 0.14);
                gb = mix(gb, 6.0, 0.14);
            } else if point_in_poly(gx, gy, &geom.wing_creases[0])
                || point_in_poly(gx, gy, &geom.wing_creases[1])
            {
                gr = mix(gr, 217.0, 0.11);
                gg = mix(gg, 119.0, 0.11);
                gb = mix(gb, 6.0, 0.11);
            }

            if gy >= 381.5 {
                if point_in_poly(gx, gy, &geom.terrace_bands[0])
                    || point_in_poly(gx, gy, &geom.terrace_bands[1])
                {
                    gr = mix(gr, 217.0, 0.15);
                    gg = mix(gg, 119.0, 0.15);
                    gb = mix(gb, 6.0, 0.15);
                }
                if point_in_poly(gx, gy, &geom.royal_stairs) {
                    gr = mix(gr, 245.0, 0.11);
                    gg = mix(gg, 158.0, 0.11);
                    gb = mix(gb, 11.0, 0.11);
                    for tread in &geom.royal_treads {
                        let td = dist_to_poly_segments(gx, gy, tread);
                        if td < 1.3 {
                            let tw = smoothstep(1.3, 0.4, td) * 0.18;
                            gr = mix(gr, 217.0, tw);
                            gg = mix(gg, 119.0, tw);
                            gb = mix(gb, 6.0, tw);
                        }
                    }
                }
            }
        }

        cr = gr;
        cg = gg;
        cb = gb;
    }

    // 9. 圆角方框玻璃高光内描边
    if box_sdf > -5.5 {
        let rim = smoothstep(-5.5, -1.5, box_sdf) * smoothstep(0.5, -1.5, box_sdf);
        let top_bias = ((408.0 - sy) / 390.0).clamp(0.22, 1.0);
        let rim_a = 0.36 * top_bias * rim;
        cr = mix(cr, 255.0, rim_a);
        cg = mix(cg, 255.0, rim_a);
        cb = mix(cb, 255.0, rim_a);
    }

    (cr, cg, cb, cov)
}

#[inline]
fn point_in_even_odd(x: f32, y: f32, polys: &[Poly]) -> bool {
    let mut inside = false;
    for poly in polys {
        if point_in_poly(x, y, poly) {
            inside = !inside;
        }
    }
    inside
}

#[inline]
fn point_in_poly(x: f32, y: f32, poly: &Poly) -> bool {
    if x < poly.min_x || x > poly.max_x || y < poly.min_y || y > poly.max_y {
        return false;
    }
    let mut inside = false;
    let pts = &poly.pts;
    for i in 0..pts.len().saturating_sub(1) {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[i + 1];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
    }
    inside
}

#[inline]
fn dist_to_polys(x: f32, y: f32, polys: &[Poly]) -> f32 {
    let mut best = f32::INFINITY;
    for poly in polys {
        let pad = 6.0;
        if x < poly.min_x - pad
            || x > poly.max_x + pad
            || y < poly.min_y - pad
            || y > poly.max_y + pad
        {
            continue;
        }
        best = best.min(dist_to_poly_segments(x, y, poly));
    }
    best
}

#[inline]
fn dist_to_poly_segments(x: f32, y: f32, poly: &Poly) -> f32 {
    let mut best_sq = f32::INFINITY;
    let pts = &poly.pts;
    for i in 0..pts.len().saturating_sub(1) {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[i + 1];
        let pax = x - ax;
        let pay = y - ay;
        let bax = bx - ax;
        let bay = by - ay;
        let denom = (bax * bax + bay * bay).max(1e-6);
        let h = ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0);
        let dx = pax - bax * h;
        let dy = pay - bay * h;
        best_sq = best_sq.min(dx * dx + dy * dy);
    }
    best_sq.sqrt()
}

#[inline]
fn project_grad(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> f32 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let denom = (dx * dx + dy * dy).max(1e-6);
    (((x - x0) * dx + (y - y0) * dy) / denom).clamp(0.0, 1.0)
}

#[inline]
fn sd_rounded_box(x: f32, y: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = x.abs() - (bx - r);
    let qy = y.abs() - (by - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (mix(a.0, b.0, t), mix(a.1, b.1, t), mix(a.2, b.2, t))
}

#[inline]
fn lerp3_3(a: (f32, f32, f32), b: (f32, f32, f32), c: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    if t < 0.5 {
        lerp3(a, b, t * 2.0)
    } else {
        lerp3(b, c, (t - 0.5) * 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_brand_and_all_tray_states_cleanly() {
        let brand = render_icon_rgba(None, 64);
        assert_eq!(brand.len(), 64 * 64 * 4);
        // 中心区域（王城门墙体）应完全不透明
        let center_alpha = brand[(32 * 64 + 32) * 4 + 3];
        assert_eq!(center_alpha, 255);

        for state in [
            MonitorState::Online,
            MonitorState::Checking,
            MonitorState::Authenticating,
            MonitorState::Offline,
            MonitorState::Paused,
        ] {
            let rgba = render_icon_rgba(Some(state), 28);
            assert_eq!(rgba.len(), 28 * 28 * 4);
            assert!(rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 255));
            // 四边中点像素 100% 满幅贴边（28×28 占满全高全宽，与托盘相邻图标严格对齐），四角圆角外透明
            assert_eq!(rgba[(14 * 4) + 3], 255);
            assert_eq!(rgba[((27 * 28 + 14) * 4) + 3], 255);
            assert_eq!(rgba[((14 * 28) * 4) + 3], 255);
            assert_eq!(rgba[((14 * 28 + 27) * 4) + 3], 255);
            assert_eq!(rgba[3], 0);
            assert_eq!(rgba[(27 * 4) + 3], 0);
        }

        if let Ok(out_dir) = std::env::var("GXNU_DUMP_ICO_FRAMES") {
            let dir = std::path::Path::new(&out_dir);
            std::fs::create_dir_all(dir).unwrap();
            for sz in [16u32, 24, 32, 48, 64, 128, 256] {
                let buf = render_icon_rgba(None, sz);
                std::fs::write(dir.join(format!("frame_{sz}.rgba")), &buf).unwrap();
            }
        }
    }
}
