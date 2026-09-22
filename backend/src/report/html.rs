// SPDX-FileCopyrightText: © 2026 LEDGER SAS
// SPDX-License-Identifier: MIT

//! Standalone HTML rendering for the report dashboard.

use super::builder::MAX_SNAPSHOTS_SCANNED;
use super::types::*;

const REPORT_REFRESH_SECS: u32 = 30;
/// Files listed in the HTML table before the rest is folded away.
const HTML_FILES_SHOWN: usize = 15;

const REPORT_CSS: &str = r#"
:root {
  --bg: #0b0f19; --panel: #131a2a; --panel-alt: #192338; --border: #243049;
  --text: #e6ebf5; --muted: #8b97ad; --ok: #22c55e; --warn: #f59e0b;
  --bad: #ef4444; --accent: #6366f1;
}
* { box-sizing: border-box; }
body {
  margin: 0; padding: 32px 24px 64px; font-size: 14px; line-height: 1.55;
  color: var(--text); background: var(--bg);
  background-image: radial-gradient(1100px 520px at 18% -12%, #1c2745 0%, rgba(11,15,25,0) 62%);
  font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  -webkit-font-smoothing: antialiased;
}
.wrap { max-width: 1120px; margin: 0 auto; }
.topbar { display: flex; flex-wrap: wrap; gap: 24px; align-items: center; justify-content: space-between; margin-bottom: 32px; }
.brand { display: flex; align-items: center; gap: 14px; }
.logo {
  width: 44px; height: 44px; border-radius: 12px; display: grid; place-items: center;
  font-weight: 700; letter-spacing: .5px; color: #fff;
  background: linear-gradient(135deg, var(--accent), #a855f7);
  box-shadow: 0 6px 20px rgba(99,102,241,.35);
}
h1 { margin: 0; font-size: 20px; letter-spacing: -.01em; }
h2 { margin: 0; font-size: 16px; letter-spacing: -.01em; }
h3 { margin: 24px 0 10px; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; color: var(--muted); }
.sub, .muted { color: var(--muted); }
.sub { margin: 2px 0 0; font-size: 12.5px; }
.chips { display: flex; flex-wrap: wrap; gap: 10px; }
.chip {
  background: var(--panel); border: 1px solid var(--border); border-radius: 12px;
  padding: 10px 16px; min-width: 88px; text-align: center;
}
.chip-value { display: block; font-size: 20px; font-weight: 650; font-variant-numeric: tabular-nums; }
.chip-label { display: block; font-size: 11px; text-transform: uppercase; letter-spacing: .07em; color: var(--muted); }
.card {
  background: var(--panel); border: 1px solid var(--border); border-radius: 16px;
  padding: 22px 24px; margin-bottom: 20px; box-shadow: 0 1px 2px rgba(0,0,0,.3);
}
.card-head { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; }
.card-head .spacer { flex: 1; }
.pill {
  font-size: 11px; font-weight: 650; text-transform: uppercase; letter-spacing: .06em;
  padding: 3px 10px; border-radius: 999px; border: 1px solid transparent; white-space: nowrap;
}
.pill-healthy, .pill-running { color: var(--ok); background: rgba(34,197,94,.12); border-color: rgba(34,197,94,.3); }
.pill-stale { color: var(--warn); background: rgba(245,158,11,.12); border-color: rgba(245,158,11,.3); }
.pill-down, .pill-crashed { color: var(--bad); background: rgba(239,68,68,.12); border-color: rgba(239,68,68,.3); }
.pill-unknown, .pill-stopped { color: var(--muted); background: rgba(139,151,173,.12); border-color: rgba(139,151,173,.28); }
.tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 12px; margin-top: 18px; }
.tile { background: var(--panel-alt); border: 1px solid var(--border); border-radius: 12px; padding: 12px 14px; }
.tile-value { display: block; font-size: 22px; font-weight: 650; font-variant-numeric: tabular-nums; }
.tile-label { display: block; font-size: 11px; text-transform: uppercase; letter-spacing: .07em; color: var(--muted); }
.ok { color: var(--ok); } .warn { color: var(--warn); } .bad { color: var(--bad); }
.bar-row { display: grid; grid-template-columns: 104px 1fr 210px 128px; gap: 14px; align-items: center; margin-bottom: 9px; }
.bar-name { font-weight: 550; text-transform: capitalize; }
.bar { height: 9px; border-radius: 999px; background: var(--panel-alt); border: 1px solid var(--border); overflow: hidden; }
.bar-fill { height: 100%; border-radius: 999px; }
.bar-value { text-align: right; font-variant-numeric: tabular-nums; }
.delta { font-size: 11px; font-weight: 600; padding: 1px 6px; border-radius: 6px; margin-left: 8px; white-space: nowrap; }
.delta-up { color: var(--ok); background: rgba(34,197,94,.12); }
.delta-down { color: var(--bad); background: rgba(239,68,68,.12); }
.delta-flat { color: var(--muted); background: rgba(139,151,173,.12); }
.spark { width: 120px; height: 26px; display: block; opacity: .85; }
.covstats { display: flex; flex-wrap: wrap; gap: 6px 18px; margin-top: 14px; font-size: 12.5px; color: var(--muted); }
.covstats b { color: var(--text); font-weight: 600; }
.filecell { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 12px; direction: rtl; text-align: left; }
.minibar { display: inline-block; width: 54px; height: 6px; border-radius: 999px; background: var(--panel-alt); border: 1px solid var(--border); overflow: hidden; margin-right: 8px; vertical-align: middle; }
.minibar-fill { display: block; height: 100%; }
details.more { margin-top: 10px; }
details.more summary { cursor: pointer; color: #93a4ff; font-size: 12.5px; padding: 4px 0; }
table { width: 100%; border-collapse: collapse; }
th {
  text-align: left; font-size: 11px; text-transform: uppercase; letter-spacing: .07em;
  color: var(--muted); font-weight: 600; padding: 0 10px 8px; border-bottom: 1px solid var(--border);
}
td { padding: 9px 10px; border-bottom: 1px solid rgba(36,48,73,.55); vertical-align: middle; }
tr:last-child td { border-bottom: none; }
td.num { font-variant-numeric: tabular-nums; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 12px; }
.path { display: block; font-size: 11.5px; color: var(--muted); }
.empty { text-align: center; padding: 48px 24px; color: var(--muted); }
footer { margin-top: 32px; text-align: center; font-size: 12px; color: var(--muted); }
a { color: #93a4ff; }
"#;

pub(super) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(super) fn format_duration(seconds: i64) -> String {
    if seconds < 0 {
        return "-".to_string();
    }
    let (days, hours) = (seconds / 86_400, (seconds % 86_400) / 3_600);
    let (minutes, secs) = ((seconds % 3_600) / 60, seconds % 60);
    if days > 0 {
        format!("{days}j {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

fn coverage_color(pct: f64) -> &'static str {
    if pct >= 70.0 {
        "#22c55e"
    } else if pct >= 40.0 {
        "#f59e0b"
    } else {
        "#ef4444"
    }
}

/// Inline SVG trend line. Values are normalised to their own min/max so that
/// small variations stay visible.
pub(super) fn sparkline(values: &[f64], color: &str) -> String {
    if values.len() < 2 {
        return "<span></span>".to_string();
    }
    let (width, height) = (120.0_f64, 26.0_f64);
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = if (max - min).abs() < f64::EPSILON {
        1.0
    } else {
        max - min
    };
    let step = width / (values.len() - 1) as f64;
    let points: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = i as f64 * step;
            let y = height - 2.0 - ((v - min) / span) * (height - 4.0);
            format!("{x:.1},{y:.1}")
        })
        .collect();
    format!(
        "<svg class=\"spark\" viewBox=\"0 0 {width} {height}\" preserveAspectRatio=\"none\">\
         <polyline points=\"{pts}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\" \
         stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>",
        pts = points.join(" "),
    )
}

pub(super) fn render_delta(delta: Option<f64>) -> String {
    match delta {
        Some(d) if d >= 0.005 => format!("<span class=\"delta delta-up\">+{d:.2} pts</span>"),
        Some(d) if d <= -0.005 => format!("<span class=\"delta delta-down\">{d:.2} pts</span>"),
        Some(_) => "<span class=\"delta delta-flat\">stable</span>".to_string(),
        None => String::new(),
    }
}

fn render_bar_row(
    metric: &str,
    pct: f64,
    covered: i64,
    total: i64,
    delta: Option<f64>,
    history: &[f64],
    title: &str,
) -> String {
    let color = coverage_color(pct);
    format!(
        r#"  <div class="bar-row" title="{title}">
    <span class="bar-name">{metric}</span>
    <div class="bar"><div class="bar-fill" style="width:{pct}%;background:{color}"></div></div>
    <span class="bar-value">{pct}% <span class="muted">{covered} / {total}</span>{delta}</span>
    {spark}
  </div>
"#,
        title = html_escape(title),
        metric = html_escape(metric),
        delta = render_delta(delta),
        spark = sparkline(history, color),
    )
}

fn render_coverage_stats(cov: &CoverageReport) -> String {
    let mut stats = String::from("  <div class=\"covstats\">");
    stats.push_str(&format!(
        "<span>snapshot du <b>{}</b></span>",
        cov.captured_at.format("%Y-%m-%d %H:%M:%S")
    ));
    match cov.previous_captured_at {
        Some(previous) => stats.push_str(&format!(
            "<span>écart mesuré depuis le <b>{}</b></span>",
            previous.format("%Y-%m-%d %H:%M")
        )),
        None => stats.push_str("<span>premier snapshot, aucun écart mesurable</span>"),
    }
    stats.push_str(&format!(
        "<span><b>{}</b> snapshots</span><span><b>{}</b> builds distincts</span>\
         <span>moteur <b>{}</b></span><span><b>{}</b> fuzzers</span>",
        cov.snapshots_total,
        cov.distinct_builds,
        html_escape(&cov.fuzzer_engine),
        cov.num_fuzzers,
    ));
    if let Some(first) = cov.first_captured_at {
        stats.push_str(&format!(
            "<span>historique depuis le <b>{}</b></span>",
            first.format("%Y-%m-%d")
        ));
    }
    if cov.history_truncated {
        stats.push_str(&format!(
            "<span>courbes limitées aux <b>{MAX_SNAPSHOTS_SCANNED}</b> derniers snapshots</span>"
        ));
    }
    stats.push_str(&format!(
        "<span>build <b class=\"mono\">{}</b></span>",
        html_escape(
            cov.build_fingerprint
                .get(..16)
                .unwrap_or(&cov.build_fingerprint)
        ),
    ));
    stats.push_str("</div>\n");
    stats
}

pub(super) fn render_file_rows(files: &[CoverageFileReport]) -> String {
    let mut rows = String::new();
    for f in files {
        let badge = if f.is_new {
            "<span class=\"delta delta-flat\">nouveau</span>".to_string()
        } else {
            render_delta(f.delta_lines_percent)
        };
        rows.push_str(&format!(
            "      <tr><td class=\"filecell\" title=\"{full}\">{short}</td>\
             <td class=\"num\"><span class=\"minibar\">\
             <span class=\"minibar-fill\" style=\"width:{pct}%;background:{color}\"></span></span>{pct}%</td>\
             <td class=\"num\">{lc} / {lt}</td><td class=\"num\">{uncovered}</td>\
             <td class=\"num\">{fc} / {ft}</td><td class=\"num\">{badge}</td></tr>\n",
            full = html_escape(&f.path),
            short = html_escape(shorten_path(&f.path)),
            pct = f.lines_percent,
            color = coverage_color(f.lines_percent),
            lc = f.lines_covered,
            lt = f.lines_total,
            uncovered = f.uncovered_lines,
            fc = f.functions_covered,
            ft = f.functions_total,
            badge = badge,
        ));
    }
    rows
}

/// Keep the tail of a long source path, which is the informative part.
///
/// The budget is counted in characters, not bytes: source paths may hold
/// multi-byte characters, and mixing a byte-length guard with a character
/// offset made the subtraction below underflow.
pub(super) fn shorten_path(path: &str) -> &str {
    const MAX: usize = 58;
    let char_count = path.chars().count();
    if char_count <= MAX {
        return path;
    }
    match path.char_indices().nth(char_count - MAX) {
        Some((idx, _)) => &path[idx..],
        None => path,
    }
}

pub(super) fn render_coverage_files(cov: &CoverageReport) -> String {
    if cov.files.is_empty() {
        return String::new();
    }

    let header = "    <thead><tr><th>Fichier</th><th>Lignes</th><th>Couvert</th>\
                  <th>Restant</th><th>Fonctions</th><th>Évolution</th></tr></thead>\n";
    let shown = cov.files.len().min(HTML_FILES_SHOWN);
    let mut out = format!(
        "  <h3>Fichiers les plus faibles · {} fichiers instrumentés</h3>\n  <table>\n{header}    <tbody>\n",
        cov.files_total
    );
    out.push_str(&render_file_rows(&cov.files[..shown]));
    out.push_str("    </tbody>\n  </table>\n");

    if cov.files.len() > shown {
        out.push_str(&format!(
            "  <details class=\"more\"><summary>Afficher les {} fichiers suivants</summary>\n\
               <table>\n{header}    <tbody>\n",
            cov.files.len() - shown
        ));
        out.push_str(&render_file_rows(&cov.files[shown..]));
        out.push_str("    </tbody>\n  </table>\n  </details>\n");
    }
    if cov.files_total > cov.files.len() {
        out.push_str(&format!(
            "  <p class=\"sub\">Classement par lignes restantes ; {} fichiers moins prioritaires ne sont pas listés.</p>\n",
            cov.files_total - cov.files.len()
        ));
    }
    out
}

fn render_global_coverage(totals: &ReportTotals) -> String {
    if totals.coverage.is_empty() || totals.schedulers < 2 {
        return String::new();
    }
    let mut out = String::from(
        "<section class=\"card\">\n  <div class=\"card-head\"><h2>Coverage global</h2>\
         <span class=\"spacer\"></span>\
         <span class=\"muted\">somme du dernier snapshot de chaque scheduler</span></div>\n",
    );
    for m in &totals.coverage {
        let title = format!("agrégé sur {} scheduler(s)", m.schedulers);
        out.push_str(&render_bar_row(
            &m.metric_name,
            m.percent,
            m.covered,
            m.total,
            None,
            &[],
            &title,
        ));
    }
    out.push_str("</section>\n");
    out
}

pub(super) fn render_report_html(report: &ReportResponse) -> String {
    let t = &report.totals;
    let mut out = String::with_capacity(8192);

    out.push_str("<!DOCTYPE html>\n<html lang=\"fr\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    out.push_str(&format!(
        "<meta http-equiv=\"refresh\" content=\"{REPORT_REFRESH_SECS}\">\n"
    ));
    out.push_str("<title>FuzzWizard — Rapport</title>\n<style>");
    out.push_str(REPORT_CSS);
    out.push_str("</style>\n</head>\n<body>\n<div class=\"wrap\">\n");

    out.push_str(&format!(
        r#"<header class="topbar">
  <div class="brand">
    <span class="logo">FW</span>
    <div>
      <h1>FuzzWizard — Rapport</h1>
      <p class="sub">Généré le {generated}</p>
    </div>
  </div>
  <div class="chips">
    <div class="chip"><span class="chip-value">{schedulers}</span><span class="chip-label">Schedulers</span></div>
    <div class="chip"><span class="chip-value">{fuzzers}</span><span class="chip-label">Fuzzers</span></div>
    <div class="chip"><span class="chip-value ok">{running}</span><span class="chip-label">Running</span></div>
    <div class="chip"><span class="chip-value bad">{crashed}</span><span class="chip-label">Crashed</span></div>
    <div class="chip"><span class="chip-value">{crashes}</span><span class="chip-label">Crashes</span></div>
    <div class="chip"><span class="chip-value warn">{crashes_24h}</span><span class="chip-label">Crashes 24h</span></div>
  </div>
</header>
"#,
        generated = report.generated_at.format("%Y-%m-%d %H:%M:%S"),
        schedulers = t.schedulers,
        fuzzers = t.fuzzers,
        running = t.running,
        crashed = t.crashed,
        crashes = t.crashes,
        crashes_24h = t.crashes_last_24h,
    ));

    out.push_str(&render_global_coverage(&report.totals));

    if report.schedulers.is_empty() {
        out.push_str(
            "<div class=\"card\"><div class=\"empty\">Aucun scheduler connu.<br>\
             Le rapport se remplira dès qu'un scheduler enverra son premier heartbeat.</div></div>\n",
        );
    }

    for s in &report.schedulers {
        let freshness = match s.seconds_since_last_update {
            Some(age) => format!("dernier update il y a {}", format_duration(age)),
            None => "aucun heartbeat reçu".to_string(),
        };

        out.push_str(&format!(
            r#"<section class="card">
  <div class="card-head">
    <h2>{name}</h2>
    <span class="pill pill-{health}">{health}</span>
    <span class="spacer"></span>
    <span class="muted">{freshness}</span>
  </div>
  <div class="tiles">
    <div class="tile"><span class="tile-value">{total}</span><span class="tile-label">Fuzzers</span></div>
    <div class="tile"><span class="tile-value ok">{running}</span><span class="tile-label">Running</span></div>
    <div class="tile"><span class="tile-value bad">{crashed}</span><span class="tile-label">Crashed</span></div>
    <div class="tile"><span class="tile-value muted">{stopped}</span><span class="tile-label">Stopped</span></div>
    <div class="tile"><span class="tile-value">{crash_total}</span><span class="tile-label">Crashes</span></div>
    <div class="tile"><span class="tile-value warn">{crash_24h}</span><span class="tile-label">Crashes 24h</span></div>
  </div>
"#,
            name = html_escape(&s.scheduler_name),
            health = s.health,
            freshness = freshness,
            total = s.fuzzers.total,
            running = s.fuzzers.running,
            crashed = s.fuzzers.crashed,
            stopped = s.fuzzers.stopped,
            crash_total = s.crashes.total,
            crash_24h = s.crashes.last_24h,
        ));

        out.push_str("  <h3>Coverage</h3>\n");
        match &s.coverage {
            Some(cov) if !cov.metrics.is_empty() => {
                for m in &cov.metrics {
                    let history: Vec<f64> = m.history.iter().map(|p| p.percent).collect();
                    let title = match m.peak_captured_at {
                        Some(at) => format!(
                            "pic {:.2}% le {}",
                            m.peak_percent,
                            at.format("%Y-%m-%d %H:%M")
                        ),
                        None => format!("pic {:.2}%", m.peak_percent),
                    };
                    out.push_str(&render_bar_row(
                        &m.metric_name,
                        m.percent,
                        m.covered.into(),
                        m.total.into(),
                        m.delta_percent,
                        &history,
                        &title,
                    ));
                }
                out.push_str(&render_coverage_stats(cov));
                out.push_str(&render_coverage_files(cov));
            }
            _ => out.push_str("  <p class=\"muted\">Aucun snapshot de coverage disponible.</p>\n"),
        }

        out.push_str("  <h3>Fuzzers</h3>\n");
        if s.fuzzer_details.is_empty() {
            out.push_str("  <p class=\"muted\">Aucun fuzzer déclaré par ce scheduler.</p>\n");
        } else {
            out.push_str(
                "  <table>\n    <thead><tr><th>Fuzzer</th><th>Statut</th><th>Uptime</th>\
                 <th>Dernier update</th><th>SHA</th></tr></thead>\n    <tbody>\n",
            );
            for f in &s.fuzzer_details {
                out.push_str(&format!(
                    "      <tr><td><strong>{name}</strong><span class=\"path\">{path}</span></td>\
                     <td><span class=\"pill pill-{status}\">{status}</span></td>\
                     <td class=\"num\">{uptime}</td><td class=\"num\">{age}</td>\
                     <td class=\"mono muted\">{sha}</td></tr>\n",
                    name = html_escape(&f.name),
                    path = html_escape(&f.path),
                    status = html_escape(&f.status),
                    uptime = f
                        .uptime_seconds
                        .map(format_duration)
                        .unwrap_or_else(|| "-".to_string()),
                    age = format_duration(f.seconds_since_last_update),
                    sha = html_escape(f.sha.get(..12).unwrap_or(&f.sha)),
                ));
            }
            out.push_str("    </tbody>\n  </table>\n");
        }

        out.push_str("</section>\n");
    }

    out.push_str(&format!(
        "<footer>Rafraîchissement automatique toutes les {REPORT_REFRESH_SECS} s · \
         <a href=\"/report\">version JSON</a></footer>\n"
    ));
    out.push_str("</div>\n</body>\n</html>\n");
    out
}
