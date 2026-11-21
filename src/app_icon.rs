//! 统一 GXNU 品牌与应用图标绘制（纯 SDF + 3×3 超采样抗锯齿）：
//! - 清爽 Fluent 晴空蓝（`#60A5FA` → `#2563EB` → `#0EA5E9`）大圆角矩形
//! - 居中圆润可爱的「独秀峰 + 王城正门拱门」剪影，右肩探出一轮暖金小太阳，左侧点缀桂花金星芒
//! - 峰顶向上跃动两条圆头纯白 Wi-Fi 信号波纹
//! - 托盘态右下角叠加白边柔和状态圆点；`.exe` 图标、任务栏图标、窗口左上角图标四处完全统一

use crate::models::MonitorState;

/// 渲染 RGBA8 像素数据。
/// - `tray_state = None`：纯净品牌图标（用于任务栏窗口图标、主窗口左上角 Logo、`.exe` 图标）
/// - `tray_state = Some(state)`：系统托盘状态图标（带柔和状态色与右下角状态指示点）
pub fn render_icon_rgba(tray_state: Option<MonitorState>, size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let inv = 1.0 / size as f32;

    // 3×3 超采样抗锯齿，保证 16×16 / 24×24 / 32×32 小尺寸下依然精致圆润
    const OFFSETS: [f32; 3] = [1.0 / 6.0, 0.5, 5.0 / 6.0];

    for py in 0..size {
        for px in 0..size {
            let mut acc_r = 0.0_f32;
            let mut acc_g = 0.0_f32;
            let mut acc_b = 0.0_f32;
            let mut acc_a = 0.0_f32;

            for &oy in &OFFSETS {
                let y = ((py as f32 + oy) * inv) * 2.0 - 1.0;
                for &ox in &OFFSETS {
                    let x = ((px as f32 + ox) * inv) * 2.0 - 1.0;
                    let (r, g, b, a) = sample_gxnu_icon(x, y, tray_state);
                    acc_r += r * a;
                    acc_g += g * a;
                    acc_b += b * a;
                    acc_a += a;
                }
            }

            let out_a = (acc_a / 9.0).clamp(0.0, 1.0);
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
fn sample_gxnu_icon(x: f32, y: f32, tray_state: Option<MonitorState>) -> (f32, f32, f32, f32) {
    let is_offline = tray_state == Some(MonitorState::Offline);
    let is_paused = matches!(
        tray_state,
        Some(MonitorState::Paused | MonitorState::Stopped)
    );

    let box_sdf = sd_rounded_box(x, y, 0.88, 0.88, 0.42);

    let (c_tl, c_mid, c_br) = if is_offline {
        // 柔和珊瑚红（断网警示）
        (
            (248.0, 113.0, 113.0),
            (239.0, 68.0, 68.0),
            (220.0, 38.0, 38.0),
        )
    } else if is_paused {
        // 柔和雾霾石板灰（暂停）
        (
            (148.0, 163.0, 184.0),
            (100.0, 116.0, 139.0),
            (71.0, 85.0, 105.0),
        )
    } else {
        // 清爽活泼 Fluent 晴空蓝渐变（#60A5FA → #2563EB → #0EA5E9）
        (
            (96.0, 165.0, 250.0),
            (37.0, 99.0, 235.0),
            (14.0, 165.0, 233.0),
        )
    };

    let mut r = 0.0_f32;
    let mut g = 0.0_f32;
    let mut b = 0.0_f32;
    let mut a = 0.0_f32;

    if box_sdf <= 0.03 {
        let cov = smoothstep(0.03, -0.025, box_sdf);

        let t = ((x + 1.0) * 0.26 + (y + 1.0) * 0.26).clamp(0.0, 1.0);
        let (mut cr, mut cg, mut cb) = if t < 0.5 {
            lerp3(c_tl, c_mid, t * 2.0)
        } else {
            lerp3(c_mid, c_br, (t - 0.5) * 2.0)
        };

        // 左上角清爽晴空柔光（#7DD3FC）
        if !is_paused && !is_offline {
            let sky_glow = smoothstep(1.10, 0.0, (x + 0.52).hypot(y + 0.56)) * 0.28;
            cr = cr * (1.0 - sky_glow) + 125.0 * sky_glow;
            cg = cg * (1.0 - sky_glow) + 211.0 * sky_glow;
            cb = cb * (1.0 - sky_glow) + 252.0 * sky_glow;
        }

        // 1. 独秀峰右肩后方探出的可爱暖金小太阳 + 左侧桂花金四角星芒
        if !is_paused && !is_offline {
            let sun_d = (x - 0.42).hypot(y - 0.14) - 0.22;
            let sun_halo = smoothstep(0.42, -0.05, sun_d) * 0.28;
            cr = cr * (1.0 - sun_halo) + 253.0 * sun_halo;
            cg = cg * (1.0 - sun_halo) + 224.0 * sun_halo;
            cb = cb * (1.0 - sun_halo) + 71.0 * sun_halo;
            if sun_d < 0.025 {
                let sc = smoothstep(0.025, -0.02, sun_d);
                let st = ((y + 0.08) / 0.44).clamp(0.0, 1.0);
                let (sr, sg, sb) = lerp3((254.0, 240.0, 138.0), (251.0, 146.0, 60.0), st);
                cr = cr * (1.0 - sc) + sr * sc;
                cg = cg * (1.0 - sc) + sg * sc;
                cb = cb * (1.0 - sc) + sb * sc;
            }

            let d_star = sd_sparkle(x, y, -0.58, -0.14, 0.105);
            if d_star < 0.025 {
                let kc = smoothstep(0.025, -0.018, d_star);
                cr = cr * (1.0 - kc) + 254.0 * kc;
                cg = cg * (1.0 - kc) + 240.0 * kc;
                cb = cb * (1.0 - kc) + 138.0 * kc;
            }
        }

        // 2. 峰顶上方欢快跃动的圆头 Wi-Fi 信号波纹
        let d_wifi = sd_wifi_waves(x, y);
        let d_wifi_sh = sd_wifi_waves(x, y - 0.045);
        if d_wifi_sh < 0.055 {
            let sh = smoothstep(0.055, -0.02, d_wifi_sh) * 0.16;
            cr *= 1.0 - sh;
            cg *= 1.0 - sh;
            cb *= 1.0 - sh;
        }
        if d_wifi < 0.03 {
            let wc = smoothstep(0.03, -0.022, d_wifi);
            let (wr, wg, wb) = if is_paused {
                (238.0, 242.0, 247.0)
            } else {
                (255.0, 255.0, 255.0)
            };
            cr = cr * (1.0 - wc) + wr * wc;
            cg = cg * (1.0 - wc) + wg * wc;
            cb = cb * (1.0 - wc) + wb * wc;
        }

        // 3. 居中圆润可爱的独秀峰 + 蔚蓝王城正门拱门
        let d_mtn = sd_mountain_island(x, y);
        let d_mtn_sh = sd_mountain_island(x, y - 0.05);
        if d_mtn_sh < 0.055 && d_mtn >= 0.0 {
            let sh = smoothstep(0.055, -0.02, d_mtn_sh) * 0.18;
            cr *= 1.0 - sh;
            cg *= 1.0 - sh;
            cb *= 1.0 - sh;
        }

        if d_mtn < 0.03 {
            let mc = smoothstep(0.03, -0.022, d_mtn);
            let my_t = ((y + 0.10) / 0.70).clamp(0.0, 1.0);
            let (mut mr, mut mg, mut mb) = if is_offline {
                lerp3((255.0, 255.0, 255.0), (254.0, 226.0, 226.0), my_t)
            } else if is_paused {
                lerp3((248.0, 250.0, 252.0), (214.0, 222.0, 234.0), my_t)
            } else {
                lerp3((255.0, 255.0, 255.0), (219.0, 234.0, 254.0), my_t * 0.75)
            };

            let d_arch = sd_wangcheng_arch(x, y);
            if d_arch < 0.025 {
                let ac = smoothstep(0.025, -0.018, d_arch);
                let (ar, ag, ab) = if is_offline {
                    (220.0, 38.0, 38.0)
                } else if is_paused {
                    (100.0, 116.0, 139.0)
                } else {
                    lerp3((37.0, 99.0, 235.0), (14.0, 165.0, 233.0), my_t)
                };
                mr = mr * (1.0 - ac) + ar * ac;
                mg = mg * (1.0 - ac) + ag * ac;
                mb = mb * (1.0 - ac) + ab * ac;
            }

            cr = cr * (1.0 - mc) + mr * mc;
            cg = cg * (1.0 - mc) + mg * mc;
            cb = cb * (1.0 - mc) + mb * mc;
        }

        // 断网态：上方柔和圆头斜杠
        if is_offline {
            let d_slash = sd_segment(x, y, -0.44, -0.56, 0.44, 0.18) - 0.062;
            if d_slash < 0.025 {
                let sc = smoothstep(0.025, -0.02, d_slash);
                cr = cr * (1.0 - sc) + 255.0 * sc;
                cg = cg * (1.0 - sc) + 255.0 * sc;
                cb = cb * (1.0 - sc) + 255.0 * sc;
            }
        }

        // 细腻玻璃高光内描边
        if box_sdf > -0.065 {
            let rim = smoothstep(-0.065, -0.015, box_sdf);
            let top_bias = ((0.65 - y) * 0.5).clamp(0.15, 1.0);
            let rim_alpha = 0.25 * top_bias * rim;
            cr += (255.0 - cr) * rim_alpha;
            cg += (255.0 - cg) * rim_alpha;
            cb += (255.0 - cb) * rim_alpha;
        }

        r = cr;
        g = cg;
        b = cb;
        a = cov;
    }

    // 4. 系统托盘态：右下角白边柔和状态圆点
    if let Some(state) = tray_state {
        let (dr, dg, db) = tray_dot_color(state);
        let bx = x - 0.58;
        let by = y - 0.58;
        let b_dist = bx.hypot(by);
        let ring_r = 0.37;
        let dot_r = 0.26;

        if b_dist <= ring_r + 0.03 {
            let ring_a = smoothstep(ring_r + 0.03, ring_r - 0.02, b_dist);
            let out_a = ring_a + a * (1.0 - ring_a);
            if out_a > 0.001 {
                r = (255.0 * ring_a + r * a * (1.0 - ring_a)) / out_a;
                g = (255.0 * ring_a + g * a * (1.0 - ring_a)) / out_a;
                b = (255.0 * ring_a + b * a * (1.0 - ring_a)) / out_a;
                a = out_a;
            }
        }
        if b_dist <= dot_r + 0.03 {
            let dot_a = smoothstep(dot_r + 0.03, dot_r - 0.02, b_dist);
            let spec = smoothstep(0.15, 0.0, (bx + 0.06).hypot(by + 0.07)) * 0.24;
            let sr = dr + (255.0 - dr) * spec;
            let sg = dg + (255.0 - dg) * spec;
            let sb = db + (255.0 - db) * spec;
            let out_a = dot_a + a * (1.0 - dot_a);
            if out_a > 0.001 {
                r = (sr * dot_a + r * a * (1.0 - dot_a)) / out_a;
                g = (sg * dot_a + g * a * (1.0 - dot_a)) / out_a;
                b = (sb * dot_a + b * a * (1.0 - dot_a)) / out_a;
                a = out_a;
            }
        }
    }

    (r, g, b, a)
}

#[inline]
fn tray_dot_color(state: MonitorState) -> (f32, f32, f32) {
    match state {
        MonitorState::Online => (34.0, 197.0, 94.0),
        MonitorState::Authenticating | MonitorState::Verifying | MonitorState::Suspect => {
            (251.0, 191.0, 36.0)
        }
        MonitorState::Checking => (56.0, 189.0, 248.0),
        MonitorState::Offline => (239.0, 68.0, 68.0),
        MonitorState::Paused | MonitorState::Stopped => (148.0, 163.0, 184.0),
    }
}

/// 独秀峰 + 两侧桂林喀斯特小山丘的圆润剪影（带底部柔和收边，四周留出晴空蓝底框）。
#[inline]
fn sd_mountain_island(x: f32, y: f32) -> f32 {
    let ax = x.abs();
    let py = y.clamp(0.02, 0.54);
    let t = ((py - 0.02) / 0.52).clamp(0.0, 1.0);
    let slope_half_w = 0.04 + 0.36 * t.powf(1.05);
    let d_center = (ax - slope_half_w).max(0.0).hypot(y - py) - 0.14;

    let d_left = ((x + 0.38) * 1.15).hypot((y - 0.44) * 1.25) - 0.24;
    let d_right = ((x - 0.38) * 1.15).hypot((y - 0.46) * 1.25) - 0.22;

    let d_peaks = smin(smin(d_center, d_left, 0.10), d_right, 0.10);
    let d_bottom = y - 0.57;

    let k = 0.09;
    let h = (0.5 - 0.5 * (d_bottom - d_peaks) / k).clamp(0.0, 1.0);
    d_bottom * (1.0 - h) + d_peaks * h + k * h * (1.0 - h)
}

/// 王城正门圆拱剪影。
#[inline]
fn sd_wangcheng_arch(x: f32, y: f32) -> f32 {
    sd_segment(x, y, 0.0, 0.41, 0.0, 0.62) - 0.115
}

/// 峰顶向上展开的两条圆润 Wi-Fi 信号波纹。
#[inline]
fn sd_wifi_waves(x: f32, y: f32) -> f32 {
    let wx = x;
    let wy = y - 0.05;
    let d_arc1 = sd_top_arc(wx, wy, 0.38, 0.68, 0.068);
    let d_arc2 = sd_top_arc(wx, wy, 0.63, 0.72, 0.068);
    d_arc1.min(d_arc2)
}

/// 俏皮四角星芒 SDF。
#[inline]
fn sd_sparkle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let dx = (x - cx).abs();
    let dy = (y - cy).abs();
    (dx.powf(0.72) + dy.powf(0.72)).powf(1.0 / 0.72) - r
}

#[inline]
fn sd_top_arc(x: f32, y: f32, radius: f32, half_angle: f32, half_thick: f32) -> f32 {
    let px = x.abs();
    let py = -y;
    let sc_x = half_angle.sin();
    let sc_y = half_angle.cos();
    let dist = if sc_y * px > sc_x * py {
        (px - radius * sc_x).hypot(py - radius * sc_y)
    } else {
        (px.hypot(py) - radius).abs()
    };
    dist - half_thick
}

#[inline]
fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let denom = (bax * bax + bay * bay).max(1e-6);
    let h = ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0);
    (pax - bax * h).hypot(pay - bay * h)
}

#[inline]
fn sd_rounded_box(x: f32, y: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = x.abs() - (bx - r);
    let qy = y.abs() - (by - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

#[inline]
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b * (1.0 - h) + a * h - k * h * (1.0 - h)
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}
