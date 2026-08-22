//! Publication-quality SVG figures.
//!
//! Output is a standalone SVG per figure: embeddable in LaTeX, viewable in a
//! browser, and readable in both light and dark themes. Nothing is rasterised
//! and there is no plotting dependency, so a figure in a paper and a figure in
//! the repository are the same artifact.
//!
//! Conventions, chosen once so every figure reads as one system:
//!
//!   * Every series is **direct-labelled** at its right end as well as being in
//!     the legend, so identity never rests on colour alone -- which also keeps
//!     the figures legible when a journal prints them in greyscale.
//!   * Dispersion is drawn, not summarised. A series carrying an interquartile
//!     band draws it; a point estimate with no band is visibly a point
//!     estimate.
//!   * Reference lines (ideal linear scaling, O(N) growth) are dashed and
//!     recessive: the claim under test is how far the measurement departs from
//!     them.
//!   * Log axes are used where a span crosses decades, and are labelled as
//!     such, because a linear axis over four decades hides everything but the
//!     largest point.
//!
//! The categorical palette is the validated default: adjacent-pair CVD dE 9.1
//! light / 8.4 dark, normal-vision 22.9 / 19.8. Three light-mode slots fall
//! below 3:1 against the surface, which is why direct labels are mandatory
//! rather than decorative.

use std::fmt::Write as _;

/// Categorical slots, light and dark. Assigned in fixed order, never cycled.
const SERIES_LIGHT: [&str; 5] = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#4a3aa7"];
const SERIES_DARK: [&str; 5] = ["#3987e5", "#d95926", "#199e70", "#c98500", "#9085e9"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Linear,
    Log,
}

#[derive(Clone)]
pub struct Series {
    pub name: String,
    /// (x, y)
    pub points: Vec<(f64, f64)>,
    /// (x, low, high) -- drawn as a band and as whiskers.
    pub band: Vec<(f64, f64, f64)>,
    /// Dashed and recessive: a reference, not a measurement.
    pub reference: bool,
}

impl Series {
    pub fn new(name: &str, points: Vec<(f64, f64)>) -> Series {
        Series { name: name.to_string(), points, band: Vec::new(), reference: false }
    }
    pub fn with_band(mut self, band: Vec<(f64, f64, f64)>) -> Series {
        self.band = band;
        self
    }
    pub fn as_reference(mut self) -> Series {
        self.reference = true;
        self
    }
}

pub struct Chart {
    pub title: String,
    pub subtitle: String,
    pub x_label: String,
    pub y_label: String,
    pub x_scale: Scale,
    pub y_scale: Scale,
    pub series: Vec<Series>,
    /// Printed under the figure. Journal convention, and the place to record
    /// what the reader must know to interpret the numbers.
    pub caption: String,
    pub width: f64,
    pub height: f64,
}

impl Chart {
    pub fn new(title: &str, x_label: &str, y_label: &str) -> Chart {
        Chart {
            title: title.to_string(),
            subtitle: String::new(),
            x_label: x_label.to_string(),
            y_label: y_label.to_string(),
            x_scale: Scale::Linear,
            y_scale: Scale::Linear,
            series: Vec::new(),
            caption: String::new(),
            width: 760.0,
            height: 460.0,
        }
    }

    pub fn subtitle(mut self, s: &str) -> Chart {
        self.subtitle = s.to_string();
        self
    }
    pub fn caption(mut self, s: &str) -> Chart {
        self.caption = s.to_string();
        self
    }
    pub fn log_x(mut self) -> Chart {
        self.x_scale = Scale::Log;
        self
    }
    pub fn log_y(mut self) -> Chart {
        self.y_scale = Scale::Log;
        self
    }
    pub fn add(mut self, s: Series) -> Chart {
        self.series.push(s);
        self
    }

    fn extent(&self) -> (f64, f64, f64, f64) {
        let mut x0 = f64::INFINITY;
        let mut x1 = f64::NEG_INFINITY;
        let mut y0 = f64::INFINITY;
        let mut y1 = f64::NEG_INFINITY;
        for s in &self.series {
            for (x, y) in &s.points {
                if x.is_finite() {
                    x0 = x0.min(*x);
                    x1 = x1.max(*x);
                }
                if y.is_finite() {
                    y0 = y0.min(*y);
                    y1 = y1.max(*y);
                }
            }
            for (_, lo, hi) in &s.band {
                if lo.is_finite() {
                    y0 = y0.min(*lo);
                }
                if hi.is_finite() {
                    y1 = y1.max(*hi);
                }
            }
        }
        if !x0.is_finite() {
            return (0.0, 1.0, 0.0, 1.0);
        }
        // Log axes cannot show zero; clamp to the smallest positive value.
        if self.x_scale == Scale::Log && x0 <= 0.0 {
            x0 = 1e-9;
        }
        if self.y_scale == Scale::Log && y0 <= 0.0 {
            y0 = 1e-9;
        }
        if self.y_scale == Scale::Linear {
            // Linear y starts at zero: a truncated baseline exaggerates every
            // difference on the chart, which is the most common way a correct
            // measurement becomes a misleading figure.
            y0 = y0.min(0.0);
            y1 *= 1.08;
        } else {
            y0 /= 1.6;
            y1 *= 1.6;
        }
        if (x1 - x0).abs() < f64::EPSILON {
            x1 = x0 + 1.0;
        }
        if (y1 - y0).abs() < f64::EPSILON {
            y1 = y0 + 1.0;
        }
        (x0, x1, y0, y1)
    }

    pub fn to_svg(&self) -> String {
        // The vertical budget below the plot is computed, not guessed: tick
        // labels, axis title, legend and however many lines the caption wraps
        // to each get their own band. Fixing it as a constant is what made the
        // legend sit on top of the caption and the caption fall off the canvas.
        let cap_lines = if self.caption.is_empty() { 0 } else { wrap(&self.caption, 96).len() };
        let legend_h = if self.series.len() >= 2 { 22.0 } else { 0.0 };
        let (ml, mr, mt) = (78.0, 132.0, 62.0);
        let mb = 30.0 + 22.0 + legend_h + cap_lines as f64 * 14.0 + 14.0;
        let height = self.height.max(mt + 200.0 + mb);
        let (x0, x1, y0, y1) = self.extent();
        let pw = self.width - ml - mr;
        let ph = height - mt - mb;

        let sx = |v: f64| -> f64 {
            let t = match self.x_scale {
                Scale::Linear => (v - x0) / (x1 - x0),
                Scale::Log => (v.max(1e-12).ln() - x0.max(1e-12).ln())
                    / (x1.max(1e-12).ln() - x0.max(1e-12).ln()),
            };
            ml + t * pw
        };
        let sy = |v: f64| -> f64 {
            let t = match self.y_scale {
                Scale::Linear => (v - y0) / (y1 - y0),
                Scale::Log => (v.max(1e-12).ln() - y0.max(1e-12).ln())
                    / (y1.max(1e-12).ln() - y0.max(1e-12).ln()),
            };
            mt + ph - t * ph
        };

        let mut s = String::new();
        let _ = write!(
            s,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" font-family="'IBM Plex Sans',-apple-system,'Segoe UI',Helvetica,Arial,sans-serif" role="img" aria-label="{title}">"#,
            w = self.width,
            h = height,
            title = esc(&self.title)
        );

        // Theme tokens. Defined on bare :root for light, then redefined under
        // both the media query and the data-theme stamp so the figure follows
        // the viewer in all three states.
        let _ = write!(s, "<style>");
        s.push_str(
            ":root{--fg:#151a24;--fg2:#525d74;--fg3:#7d879c;--grid:#e2e6ee;--axis:#aeb6c6;--bg:#ffffff;",
        );
        for (i, c) in SERIES_LIGHT.iter().enumerate() {
            let _ = write!(s, "--s{}:{};", i + 1, c);
        }
        s.push('}');
        s.push_str("@media(prefers-color-scheme:dark){:root:not([data-theme=\"light\"]){--fg:#e3e8f2;--fg2:#9aa5bc;--fg3:#6e7a92;--grid:#242c3a;--axis:#3a4557;--bg:#0d1017;");
        for (i, c) in SERIES_DARK.iter().enumerate() {
            let _ = write!(s, "--s{}:{};", i + 1, c);
        }
        s.push_str("}}");
        s.push_str(":root[data-theme=\"dark\"]{--fg:#e3e8f2;--fg2:#9aa5bc;--fg3:#6e7a92;--grid:#242c3a;--axis:#3a4557;--bg:#0d1017;");
        for (i, c) in SERIES_DARK.iter().enumerate() {
            let _ = write!(s, "--s{}:{};", i + 1, c);
        }
        s.push('}');
        s.push_str(
            ".ttl{font-size:15px;font-weight:600;fill:var(--fg)}\
             .sub{font-size:11.5px;fill:var(--fg2)}\
             .ax{font-size:11px;fill:var(--fg2);font-variant-numeric:tabular-nums}\
             .axt{font-size:11.5px;font-weight:500;fill:var(--fg2)}\
             .lbl{font-size:11px;font-weight:600;font-variant-numeric:tabular-nums}\
             .cap{font-size:10.5px;fill:var(--fg3)}\
             .lg{font-size:11px;fill:var(--fg2)}",
        );
        let _ = write!(s, "</style>");

        let _ = write!(
            s,
            r#"<rect width="{}" height="{}" fill="var(--bg)"/>"#,
            self.width, height
        );
        let _ = write!(s, r#"<text x="{ml}" y="26" class="ttl">{}</text>"#, esc(&self.title));
        if !self.subtitle.is_empty() {
            let _ = write!(s, r#"<text x="{ml}" y="43" class="sub">{}</text>"#, esc(&self.subtitle));
        }

        // Grid and ticks.
        let yt = ticks(y0, y1, self.y_scale, 6);
        for t in &yt {
            let y = sy(*t);
            let _ = write!(
                s,
                r#"<line x1="{ml}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="var(--grid)" stroke-width="1"/>"#,
                ml + pw
            );
            let _ = write!(
                s,
                r#"<text x="{:.1}" y="{:.1}" class="ax" text-anchor="end">{}</text>"#,
                ml - 9.0,
                y + 3.8,
                fmt_num(*t)
            );
        }
        // A discrete axis (thread counts, shard counts) must tick on its own
        // values. Generated ticks land between them and, once rounded for
        // display, print duplicates: "1, 2, 2, 2, 3, 4" for the values 1, 2, 4.
        let xt = {
            let mut distinct: Vec<f64> = Vec::new();
            for ser in self.series.iter().filter(|s| !s.reference) {
                for (x, _) in &ser.points {
                    if !distinct.iter().any(|v| (v - x).abs() < f64::EPSILON) {
                        distinct.push(*x);
                    }
                }
            }
            distinct.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            if self.x_scale == Scale::Linear && (2..=8).contains(&distinct.len()) {
                distinct
            } else {
                ticks(x0, x1, self.x_scale, 6)
            }
        };
        for t in &xt {
            let x = sx(*t);
            let _ = write!(
                s,
                r#"<line x1="{x:.1}" y1="{mt}" x2="{x:.1}" y2="{:.1}" stroke="var(--grid)" stroke-width="1"/>"#,
                mt + ph
            );
            let _ = write!(
                s,
                r#"<text x="{x:.1}" y="{:.1}" class="ax" text-anchor="middle">{}</text>"#,
                mt + ph + 18.0,
                fmt_num(*t)
            );
        }
        // Baseline and left rule only: a full box is chartjunk.
        let _ = write!(
            s,
            r#"<line x1="{ml}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="var(--axis)" stroke-width="1"/>"#,
            mt + ph,
            ml + pw,
            mt + ph
        );

        let axis_note = |sc: Scale| if sc == Scale::Log { " (log)" } else { "" };
        let _ = write!(
            s,
            r#"<text x="{:.1}" y="{:.1}" class="axt" text-anchor="middle">{}{}</text>"#,
            ml + pw / 2.0,
            mt + ph + 40.0,
            esc(&self.x_label),
            axis_note(self.x_scale)
        );
        let _ = write!(
            s,
            r#"<text transform="translate({:.1},{:.1}) rotate(-90)" class="axt" text-anchor="middle">{}{}</text>"#,
            18.0,
            mt + ph / 2.0,
            esc(&self.y_label),
            axis_note(self.y_scale)
        );

        // Series.
        let mut labels: Vec<(f64, f64, String, String)> = Vec::new();
        for (i, ser) in self.series.iter().enumerate() {
            let col = format!("var(--s{})", (i % SERIES_LIGHT.len()) + 1);
            if ser.points.is_empty() {
                continue;
            }
            if ser.reference {
                let d = path_of(&ser.points, &sx, &sy);
                let _ = write!(
                    s,
                    r#"<path d="{d}" fill="none" stroke="var(--fg3)" stroke-width="1.5" stroke-dasharray="5 4" opacity="0.75"/>"#
                );
            } else {
                // Interquartile band first, so the line sits on top of it.
                if !ser.band.is_empty() {
                    let mut d = String::new();
                    for (n, (x, _, hi)) in ser.band.iter().enumerate() {
                        let _ = write!(
                            d,
                            "{}{:.1} {:.1}",
                            if n == 0 { "M" } else { "L" },
                            sx(*x),
                            sy(*hi)
                        );
                    }
                    for (x, lo, _) in ser.band.iter().rev() {
                        let _ = write!(d, "L{:.1} {:.1}", sx(*x), sy(*lo));
                    }
                    d.push('Z');
                    let _ = write!(s, r#"<path d="{d}" fill="{col}" opacity="0.14"/>"#);
                    for (x, lo, hi) in &ser.band {
                        let _ = write!(
                            s,
                            r#"<line x1="{x1:.1}" y1="{:.1}" x2="{x1:.1}" y2="{:.1}" stroke="{col}" stroke-width="1.25" opacity="0.6"/>"#,
                            sy(*lo),
                            sy(*hi),
                            x1 = sx(*x)
                        );
                    }
                }
                let d = path_of(&ser.points, &sx, &sy);
                let _ = write!(s, r#"<path d="{d}" fill="none" stroke="{col}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>"#);
                // A 2px surface ring keeps overlapping markers separable.
                for (x, y) in &ser.points {
                    let _ = write!(
                        s,
                        r#"<circle cx="{:.1}" cy="{:.1}" r="4" fill="{col}" stroke="var(--bg)" stroke-width="2"/>"#,
                        sx(*x),
                        sy(*y)
                    );
                }
            }
            // Direct label at the right end, positioned after every series
            // is placed so overlapping ends can be pushed apart.
            let (lx, ly) = *ser.points.last().unwrap();
            let fill = if ser.reference { "var(--fg3)".to_string() } else { col.clone() };
            labels.push((sx(lx) + 9.0, sy(ly) + 3.8, fill, ser.name.clone()));
        }

        // Nudge colliding end labels apart. Two series that finish at nearly
        // the same value would otherwise print on top of each other, which
        // defeats the point of direct labelling.
        labels.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        const GAP: f64 = 13.5;
        for i in 1..labels.len() {
            if labels[i].1 - labels[i - 1].1 < GAP {
                labels[i].1 = labels[i - 1].1 + GAP;
            }
        }
        // Keep the whole stack inside the plot area.
        if let Some(last) = labels.last() {
            let overflow = last.1 - (mt + ph);
            if overflow > 0.0 {
                for l in labels.iter_mut() {
                    l.1 -= overflow;
                }
            }
        }
        for (x, y, fill, name) in &labels {
            let _ = write!(
                s,
                r#"<text x="{x:.1}" y="{y:.1}" class="lbl" fill="{fill}">{}</text>"#,
                esc(name)
            );
        }

        // Legend, present whenever there are two or more series.
        if self.series.len() >= 2 {
            let mut x = ml;
            let y = mt + ph + 62.0;
            for (i, ser) in self.series.iter().enumerate() {
                let col = if ser.reference {
                    "var(--fg3)".to_string()
                } else {
                    format!("var(--s{})", (i % SERIES_LIGHT.len()) + 1)
                };
                let dash = if ser.reference { r#" stroke-dasharray="5 4""# } else { "" };
                let _ = write!(
                    s,
                    r#"<line x1="{x:.1}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="{col}" stroke-width="2"{dash}/>"#,
                    x + 16.0
                );
                let _ = write!(
                    s,
                    r#"<text x="{:.1}" y="{:.1}" class="lg">{}</text>"#,
                    x + 22.0,
                    y + 3.8,
                    esc(&ser.name)
                );
                x += 30.0 + 6.6 * ser.name.chars().count() as f64;
            }
        }

        if !self.caption.is_empty() {
            for (i, line) in wrap(&self.caption, 96).iter().enumerate() {
                let _ = write!(
                    s,
                    r#"<text x="{ml}" y="{:.1}" class="cap">{}</text>"#,
                    mt + ph + 62.0 + legend_h + 8.0 + i as f64 * 14.0,
                    esc(line)
                );
            }
        }
        s.push_str("</svg>");
        s
    }
}

fn path_of(
    pts: &[(f64, f64)],
    sx: &dyn Fn(f64) -> f64,
    sy: &dyn Fn(f64) -> f64,
) -> String {
    let mut d = String::new();
    for (n, (x, y)) in pts.iter().enumerate() {
        let _ = write!(d, "{}{:.1} {:.1}", if n == 0 { "M" } else { "L" }, sx(*x), sy(*y));
    }
    d
}

/// Tick positions: 1-2-5 per decade on a log axis, nice round steps on linear.
fn ticks(lo: f64, hi: f64, scale: Scale, target: usize) -> Vec<f64> {
    let mut out = Vec::new();
    match scale {
        Scale::Log => {
            let d0 = lo.max(1e-12).log10().floor() as i32;
            let d1 = hi.max(1e-12).log10().ceil() as i32;
            for d in d0..=d1 {
                for m in [1.0, 2.0, 5.0] {
                    let v = m * 10f64.powi(d);
                    if v >= lo && v <= hi {
                        out.push(v);
                    }
                }
            }
            // Too many decades: thin to whole decades only.
            if out.len() > target * 2 {
                out.retain(|v| (v.log10() - v.log10().round()).abs() < 1e-9);
            }
        }
        Scale::Linear => {
            let span = hi - lo;
            if span <= 0.0 {
                return vec![lo];
            }
            let raw = span / target as f64;
            let mag = 10f64.powf(raw.log10().floor());
            let norm = raw / mag;
            let step = if norm <= 1.0 {
                1.0
            } else if norm <= 2.0 {
                2.0
            } else if norm <= 5.0 {
                5.0
            } else {
                10.0
            } * mag;
            let mut v = (lo / step).ceil() * step;
            while v <= hi + step * 0.001 {
                out.push(v);
                v += step;
            }
        }
    }
    out
}

fn fmt_num(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.0}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.0}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.0}k", v / 1e3)
    } else if a >= 1.0 || a == 0.0 {
        // Only drop the decimals when there are none to drop, or a fractional
        // tick prints as a duplicate of its neighbour.
        if (v - v.round()).abs() < 1e-9 { format!("{:.0}", v.round()) } else { format!("{v:.1}") }
    } else if a >= 0.01 {
        format!("{v:.2}")
    } else {
        format!("{v:.0e}")
    }
}

fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for w in s.split_whitespace() {
        if !line.is_empty() && line.chars().count() + w.chars().count() + 1 > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(w);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Chart {
        Chart::new("T", "x", "y")
            .add(Series::new("a", vec![(1.0, 10.0), (2.0, 20.0)]))
            .add(Series::new("b", vec![(1.0, 5.0), (2.0, 7.0)]))
    }

    #[test]
    fn svg_is_wellformed_and_self_contained() {
        let s = demo().to_svg();
        assert!(s.starts_with("<svg") && s.ends_with("</svg>"));
        assert_eq!(s.matches("<svg").count(), 1);
        // No *fetched* resources: a figure must render offline and inside a
        // strict content-security policy. The SVG namespace URI is a name,
        // not a fetch, so it is excluded from this check.
        let body = s.replace("http://www.w3.org/2000/svg", "");
        for forbidden in ["http://", "https://", "xlink", "<image", "@import", "url("] {
            assert!(!body.contains(forbidden), "figure must not reference {forbidden}");
        }
    }

    #[test]
    fn both_themes_are_defined() {
        let s = demo().to_svg();
        assert!(s.contains("prefers-color-scheme:dark"));
        assert!(s.contains("[data-theme=\"dark\"]"));
        assert!(s.contains("fill=\"var(--bg)\""), "must paint its own ground");
    }

    /// Identity must never rest on colour alone.
    #[test]
    fn every_series_is_direct_labelled_and_legended() {
        let s = demo().to_svg();
        assert_eq!(s.matches(">a</text>").count(), 2, "direct label + legend");
        assert_eq!(s.matches(">b</text>").count(), 2);
    }

    /// A truncated baseline exaggerates differences; linear y must start at 0.
    #[test]
    fn linear_y_axis_includes_zero() {
        let c = Chart::new("T", "x", "y").add(Series::new("a", vec![(1.0, 100.0), (2.0, 101.0)]));
        let (_, _, y0, _) = c.extent();
        assert!(y0 <= 0.0, "y0={y0}");
    }

    #[test]
    fn log_axis_survives_zero_and_negative_input() {
        let c = Chart::new("T", "x", "y")
            .log_x()
            .log_y()
            .add(Series::new("a", vec![(0.0, 0.0), (10.0, 100.0)]));
        let svg = c.to_svg();
        assert!(!svg.contains("NaN"), "log scale produced NaN geometry");
        assert!(!svg.contains("inf"));
    }

    #[test]
    fn log_ticks_are_one_two_five_per_decade() {
        let t = ticks(1.0, 100.0, Scale::Log, 6);
        assert!(t.contains(&1.0) && t.contains(&10.0) && t.contains(&100.0));
        assert!(t.iter().all(|v| *v > 0.0));
    }

    #[test]
    fn reference_series_are_dashed_and_not_coloured_as_data() {
        let c = demo().add(Series::new("ideal", vec![(1.0, 1.0), (2.0, 2.0)]).as_reference());
        let s = c.to_svg();
        assert!(s.contains("stroke-dasharray"));
        assert!(s.contains(">ideal</text>"));
    }

    #[test]
    fn bands_are_drawn_when_present() {
        let c = Chart::new("T", "x", "y").add(
            Series::new("a", vec![(1.0, 10.0), (2.0, 20.0)])
                .with_band(vec![(1.0, 9.0, 11.0), (2.0, 18.0, 22.0)]),
        );
        let s = c.to_svg();
        assert!(s.contains("opacity=\"0.14\""), "IQR band must be drawn");
    }

    #[test]
    fn empty_chart_does_not_panic() {
        let s = Chart::new("empty", "x", "y").to_svg();
        assert!(s.contains("</svg>"));
    }
}

#[cfg(test)]
mod axis_tests {
    use super::*;

    /// Regression: thread counts 1, 2, 4 produced generated ticks that rounded
    /// to "1, 2, 2, 2, 3, 4" -- three labels reading 2.
    #[test]
    fn discrete_x_axis_ticks_on_its_own_values() {
        let svg = Chart::new("T", "threads", "ops/s")
            .add(Series::new("m", vec![(1.0, 10.0), (2.0, 12.0), (4.0, 9.0)]))
            .to_svg();
        let labels: Vec<&str> = svg
            .match_indices("text-anchor=\"middle\">")
            .map(|(i, m)| {
                let rest = &svg[i + m.len()..];
                &rest[..rest.find('<').unwrap_or(0)]
            })
            .collect();
        let mut nums: Vec<&&str> = labels.iter().filter(|l| l.parse::<f64>().is_ok()).collect();
        let before = nums.len();
        nums.sort();
        nums.dedup();
        assert_eq!(before, nums.len(), "duplicate x tick labels: {labels:?}");
    }

    #[test]
    fn fractional_ticks_keep_a_decimal() {
        assert_eq!(fmt_num(2.0), "2");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(fmt_num(0.0), "0");
    }
}
