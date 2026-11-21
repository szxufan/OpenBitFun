use crate::agentic::insights::types::*;
use crate::service::i18n::LocaleId;

/// All user-visible labels in the HTML report, supporting i18n.
pub struct HtmlLabels {
    pub title: &'static str,
    pub subtitle_template: &'static str, // "{msgs} messages across {sessions} sessions ({analyzed} analyzed) | {start} to {end}"
    pub partial_report: &'static str,
    pub at_a_glance: &'static str,
    pub whats_working: &'static str,
    pub whats_hindering: &'static str,
    pub quick_wins: &'static str,
    pub looking_ahead: &'static str,
    pub nav_work: &'static str,
    pub nav_usage: &'static str,
    pub nav_wins: &'static str,
    pub nav_friction: &'static str,
    pub nav_suggestions: &'static str,
    pub nav_horizon: &'static str,
    pub stat_sessions: &'static str,
    pub stat_messages: &'static str,
    pub stat_hours: &'static str,
    pub stat_days: &'static str,
    pub stat_msgs_per_day: &'static str,
    pub stat_median_response: &'static str,
    pub stat_avg_response: &'static str,
    pub section_work: &'static str,
    pub section_usage: &'static str,
    pub section_wins: &'static str,
    pub section_friction: &'static str,
    pub section_suggestions: &'static str,
    pub section_horizon: &'static str,
    pub chart_goals: &'static str,
    pub chart_tools: &'static str,
    pub chart_languages: &'static str,
    pub chart_session_types: &'static str,
    pub chart_tool_errors: &'static str,
    pub chart_agent_types: &'static str,
    pub chart_response_time: &'static str,
    pub chart_time_of_day: &'static str,
    pub chart_what_helped: &'static str,
    pub chart_outcomes: &'static str,
    pub chart_friction_types: &'static str,
    pub chart_satisfaction: &'static str,
    pub time_morning: &'static str,
    pub time_afternoon: &'static str,
    pub time_evening: &'static str,
    pub time_night: &'static str,
    pub sessions_suffix: &'static str,
    pub no_data: &'static str,
    pub no_project_areas: &'static str,
    pub no_interaction_style: &'static str,
    pub no_big_wins: &'static str,
    pub no_friction: &'static str,
    pub no_horizon: &'static str,
    pub md_additions: &'static str,
    pub copy: &'static str,
    pub copy_all_checked: &'static str,
    pub features_to_try: &'static str,
    pub usage_patterns: &'static str,
    pub try_this_prompt: &'static str,
    pub copied: &'static str,
    pub median_label: &'static str,
    pub average_label: &'static str,
    pub stat_lines: &'static str,
    pub stat_files: &'static str,
}

impl HtmlLabels {
    pub fn for_locale(locale: LocaleId) -> Self {
        match locale {
            LocaleId::EnUS => Self::en(),
            LocaleId::ZhCN => Self::zh(),
            LocaleId::ZhTW => Self::zh_tw(),
        }
    }

    pub fn en() -> Self {
        HtmlLabels {
            title: "OpenBitFun Insights",
            subtitle_template: "{msgs} messages across {sessions} sessions ({analyzed} analyzed) | {start} to {end}",
            partial_report: "Partial report. {failed} report sections failed.",
            at_a_glance: "At a Glance",
            whats_working: "What's working:",
            whats_hindering: "What's hindering you:",
            quick_wins: "Quick wins to try:",
            looking_ahead: "Looking ahead:",
            nav_work: "What You Work On",
            nav_usage: "How You Use OpenBitFun",
            nav_wins: "Impressive Things",
            nav_friction: "Where Things Go Wrong",
            nav_suggestions: "Suggestions",
            nav_horizon: "On the Horizon",
            stat_sessions: "Sessions",
            stat_messages: "Messages",
            stat_hours: "Hours",
            stat_days: "Days",
            stat_msgs_per_day: "Msgs/Day",
            stat_median_response: "Median Response",
            stat_avg_response: "Avg Response",
            section_work: "What You Work On",
            section_usage: "How You Use OpenBitFun",
            section_wins: "Impressive Things You Did",
            section_friction: "Where Things Go Wrong",
            section_suggestions: "Suggestions",
            section_horizon: "On the Horizon",
            chart_goals: "What You Wanted",
            chart_tools: "Top Tools Used",
            chart_languages: "Languages",
            chart_session_types: "Session Types",
            chart_tool_errors: "Tool Errors Encountered",
            chart_agent_types: "Agent Types",
            chart_response_time: "User Response Time Distribution",
            chart_time_of_day: "Messages by Time of Day",
            chart_what_helped: "What Helped Most",
            chart_outcomes: "Outcomes",
            chart_friction_types: "Primary Friction Types",
            chart_satisfaction: "Satisfaction (Inferred)",
            time_morning: "Morning (6-12)",
            time_afternoon: "Afternoon (12-18)",
            time_evening: "Evening (18-24)",
            time_night: "Night (0-6)",
            sessions_suffix: "sessions",
            no_data: "No data",
            no_project_areas: "No project areas identified.",
            no_interaction_style: "No interaction style data available.",
            no_big_wins: "No big wins identified yet.",
            no_friction: "No significant friction points found.",
            no_horizon: "No horizon workflows identified.",
            md_additions: "OPENBITFUN.md Additions",
            copy: "Copy",
            copy_all_checked: "Copy All Checked",
            features_to_try: "Features to Try",
            usage_patterns: "Usage Patterns",
            try_this_prompt: "Try this prompt:",
            copied: "Copied!",
            median_label: "Median",
            average_label: "Average",
            stat_lines: "Lines",
            stat_files: "Files",
        }
    }

    pub fn zh() -> Self {
        HtmlLabels {
            title: "OpenBitFun 洞察",
            subtitle_template:
                "{msgs} 条消息，{sessions} 个会话（{analyzed} 个已分析）| {start} 至 {end}",
            partial_report: "报告部分生成。{failed} 个报告章节生成失败。",
            at_a_glance: "概览",
            whats_working: "做得好的：",
            whats_hindering: "遇到的阻碍：",
            quick_wins: "快速提升：",
            looking_ahead: "展望未来：",
            nav_work: "工作领域",
            nav_usage: "使用方式",
            nav_wins: "亮眼成果",
            nav_friction: "问题所在",
            nav_suggestions: "建议",
            nav_horizon: "未来展望",
            stat_sessions: "会话",
            stat_messages: "消息",
            stat_hours: "小时",
            stat_days: "天",
            stat_msgs_per_day: "消息/天",
            stat_median_response: "中位响应",
            stat_avg_response: "平均响应",
            section_work: "工作领域",
            section_usage: "你如何使用 OpenBitFun",
            section_wins: "亮眼成果",
            section_friction: "问题所在",
            section_suggestions: "建议",
            section_horizon: "未来展望",
            chart_goals: "你的需求",
            chart_tools: "常用工具",
            chart_languages: "编程语言",
            chart_session_types: "会话类型",
            chart_tool_errors: "工具错误统计",
            chart_agent_types: "智能体类型",
            chart_response_time: "用户响应时间分布",
            chart_time_of_day: "按时段分布",
            chart_what_helped: "最有帮助的方面",
            chart_outcomes: "结果分布",
            chart_friction_types: "主要摩擦类型",
            chart_satisfaction: "满意度（推断）",
            time_morning: "上午 (6-12)",
            time_afternoon: "下午 (12-18)",
            time_evening: "晚上 (18-24)",
            time_night: "凌晨 (0-6)",
            sessions_suffix: "个会话",
            no_data: "暂无数据",
            no_project_areas: "未识别到项目领域。",
            no_interaction_style: "暂无交互风格数据。",
            no_big_wins: "暂未识别到亮眼成果。",
            no_friction: "未发现明显摩擦点。",
            no_horizon: "暂未识别到未来工作流。",
            md_additions: "OPENBITFUN.md 补充",
            copy: "复制",
            copy_all_checked: "复制选中项",
            features_to_try: "推荐功能",
            usage_patterns: "使用模式",
            try_this_prompt: "试试这个提示：",
            copied: "已复制！",
            median_label: "中位数",
            average_label: "平均值",
            stat_lines: "行",
            stat_files: "文件",
        }
    }

    fn zh_tw() -> Self {
        HtmlLabels {
            title: "OpenBitFun 洞察",
            subtitle_template:
                "{msgs} 則訊息，{sessions} 個會話（{analyzed} 個已分析）| {start} 至 {end}",
            partial_report: "報告部分產生。{failed} 個報告章節產生失敗。",
            at_a_glance: "概覽",
            whats_working: "做得好的：",
            whats_hindering: "遇到的阻礙：",
            quick_wins: "快速改善：",
            looking_ahead: "展望未來：",
            nav_work: "工作領域",
            nav_usage: "使用方式",
            nav_wins: "亮眼成果",
            nav_friction: "問題所在",
            nav_suggestions: "建議",
            nav_horizon: "未來展望",
            stat_sessions: "會話",
            stat_messages: "訊息",
            stat_hours: "小時",
            stat_days: "天",
            stat_msgs_per_day: "訊息／天",
            stat_median_response: "回應時間中位數",
            stat_avg_response: "平均回應時間",
            section_work: "工作領域",
            section_usage: "你如何使用 OpenBitFun",
            section_wins: "亮眼成果",
            section_friction: "問題所在",
            section_suggestions: "建議",
            section_horizon: "未來展望",
            chart_goals: "你的需求",
            chart_tools: "常用工具",
            chart_languages: "程式語言",
            chart_session_types: "會話類型",
            chart_tool_errors: "工具錯誤統計",
            chart_agent_types: "智慧體類型",
            chart_response_time: "使用者回應時間分布",
            chart_time_of_day: "依時段分布",
            chart_what_helped: "最有幫助的部分",
            chart_outcomes: "結果分布",
            chart_friction_types: "主要阻礙類型",
            chart_satisfaction: "滿意度（推估）",
            time_morning: "上午 (6-12)",
            time_afternoon: "下午 (12-18)",
            time_evening: "晚上 (18-24)",
            time_night: "凌晨 (0-6)",
            sessions_suffix: "個會話",
            no_data: "暫無資料",
            no_project_areas: "未辨識出專案領域。",
            no_interaction_style: "暫無互動風格資料。",
            no_big_wins: "尚未辨識出亮眼成果。",
            no_friction: "未發現明顯的阻礙。",
            no_horizon: "尚未辨識出未來的工作流程。",
            md_additions: "OPENBITFUN.md 補充",
            copy: "複製",
            copy_all_checked: "複製選取項目",
            features_to_try: "推薦功能",
            usage_patterns: "使用模式",
            try_this_prompt: "試試這個提示：",
            copied: "已複製！",
            median_label: "中位數",
            average_label: "平均值",
            stat_lines: "行",
            stat_files: "檔案",
        }
    }
}

pub fn generate_html(report: &InsightsReport, locale: LocaleId) -> String {
    let l = HtmlLabels::for_locale(locale);

    let subtitle = l
        .subtitle_template
        .replace("{msgs}", &report.total_messages.to_string())
        .replace("{sessions}", &report.total_sessions.to_string())
        .replace("{analyzed}", &report.analyzed_sessions.to_string())
        .replace(
            "{start}",
            report
                .date_range
                .start
                .split('T')
                .next()
                .unwrap_or_default(),
        )
        .replace(
            "{end}",
            report.date_range.end.split('T').next().unwrap_or_default(),
        );

    let partial_notice =
        if report.analyzed_sessions < report.total_sessions || report.failed_sections != 0 {
            format!(
                "<p>{}</p>",
                html_escape(
                    &l.partial_report
                        .replace("{failed}", &report.failed_sections.to_string())
                )
            )
        } else {
            String::new()
        };

    let at_a_glance = render_at_a_glance(&report.at_a_glance, &l);
    let nav_toc = render_nav_toc(&l);
    let stats_row = render_stats_row(report, &l);
    let project_areas = render_project_areas(&report.project_areas, &l);
    let basic_charts = render_basic_charts(&report.stats, &l);
    let interaction_style = render_interaction_style(&report.interaction_style, &l);
    let usage_charts = render_usage_charts(&report.stats, &l);
    let wins_intro_html = if report.wins_intro.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="section-intro">{}</p>"#,
            markdown_inline(&report.wins_intro)
        )
    };
    let big_wins = render_big_wins(&report.big_wins, &l);
    let outcome_charts = render_outcome_charts(&report.stats, &l);
    let friction_intro_html = if report.friction_intro.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="section-intro">{}</p>"#,
            markdown_inline(&report.friction_intro)
        )
    };
    let friction = render_friction_categories(&report.friction_categories, &l);
    let friction_charts = render_friction_charts(&report.stats, &l);
    let suggestions = render_suggestions(&report.suggestions, &l);
    let horizon = render_horizon(&report.horizon_intro, &report.on_the_horizon, &l);
    let fun_ending = render_fun_ending(&report.fun_ending);

    let js_with_labels = JS_SCRIPT
        .replace("__COPIED__", l.copied)
        .replace("__COPY_ALL_CHECKED__", l.copy_all_checked);

    format!(
        r#"<!DOCTYPE html>
<html lang="{lang}">
<head>
  <meta charset="utf-8">
  <title>{page_title}</title>
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
  <style>
{CSS}
  </style>
</head>
<body>
  <div class="container">
    <h1>{page_title}</h1>
    <p class="subtitle">{subtitle}</p>
    {partial_notice}

    {at_a_glance}
    {nav_toc}
    {stats_row}

    <h2 id="section-work">{section_work}</h2>
    {project_areas}

    {basic_charts}

    <h2 id="section-usage">{section_usage}</h2>
    {interaction_style}

    {usage_charts}

    <h2 id="section-wins">{section_wins}</h2>
    {wins_intro}
    {big_wins}

    {outcome_charts}

    <h2 id="section-friction">{section_friction}</h2>
    {friction_intro}
    {friction}

    {friction_charts}

    <h2 id="section-suggestions">{section_suggestions}</h2>
    {suggestions}

    <h2 id="section-horizon">{section_horizon}</h2>
    {horizon}

    {fun_ending}
  </div>
  <script>
{JS}
  </script>
</body>
</html>"#,
        lang = locale.as_str(),
        CSS = CSS_STYLES,
        JS = js_with_labels,
        page_title = html_escape(l.title),
        subtitle = html_escape(&subtitle),
        section_work = html_escape(l.section_work),
        section_usage = html_escape(l.section_usage),
        section_wins = html_escape(l.section_wins),
        section_friction = html_escape(l.section_friction),
        section_suggestions = html_escape(l.section_suggestions),
        section_horizon = html_escape(l.section_horizon),
        at_a_glance = at_a_glance,
        nav_toc = nav_toc,
        stats_row = stats_row,
        project_areas = project_areas,
        basic_charts = basic_charts,
        interaction_style = interaction_style,
        usage_charts = usage_charts,
        wins_intro = wins_intro_html,
        big_wins = big_wins,
        outcome_charts = outcome_charts,
        friction_intro = friction_intro_html,
        friction = friction,
        friction_charts = friction_charts,
        suggestions = suggestions,
        horizon = horizon,
        fun_ending = fun_ending,
    )
}

fn render_at_a_glance(aag: &AtAGlance, l: &HtmlLabels) -> String {
    format!(
        r##"<div class="at-a-glance">
  <div class="glance-title">{title}</div>
  <div class="glance-sections">
    <div class="glance-section"><strong>{working}</strong> {working_text} <a href="#section-wins" class="see-more">{nav_wins} &rarr;</a></div>
    <div class="glance-section"><strong>{hindering}</strong> {hindering_text} <a href="#section-friction" class="see-more">{nav_friction} &rarr;</a></div>
    <div class="glance-section"><strong>{quick}</strong> {quick_text} <a href="#section-suggestions" class="see-more">{nav_suggestions} &rarr;</a></div>
    <div class="glance-section"><strong>{ahead}</strong> {ahead_text} <a href="#section-horizon" class="see-more">{nav_horizon} &rarr;</a></div>
  </div>
</div>"##,
        title = html_escape(l.at_a_glance),
        working = html_escape(l.whats_working),
        working_text = markdown_inline(&aag.whats_working),
        hindering = html_escape(l.whats_hindering),
        hindering_text = markdown_inline(&aag.whats_hindering),
        quick = html_escape(l.quick_wins),
        quick_text = markdown_inline(&aag.quick_wins),
        ahead = html_escape(l.looking_ahead),
        ahead_text = markdown_inline(&aag.looking_ahead),
        nav_wins = html_escape(l.section_wins),
        nav_friction = html_escape(l.section_friction),
        nav_suggestions = html_escape(l.section_suggestions),
        nav_horizon = html_escape(l.section_horizon),
    )
}

fn render_nav_toc(l: &HtmlLabels) -> String {
    format!(
        r##"<nav class="nav-toc">
  <a href="#section-work">{}</a>
  <a href="#section-usage">{}</a>
  <a href="#section-wins">{}</a>
  <a href="#section-friction">{}</a>
  <a href="#section-suggestions">{}</a>
  <a href="#section-horizon">{}</a>
</nav>"##,
        html_escape(l.nav_work),
        html_escape(l.nav_usage),
        html_escape(l.nav_wins),
        html_escape(l.nav_friction),
        html_escape(l.nav_suggestions),
        html_escape(l.nav_horizon),
    )
}

fn render_stats_row(report: &InsightsReport, l: &HtmlLabels) -> String {
    let response_time_stats = match (
        report.stats.median_response_time_secs,
        report.stats.avg_response_time_secs,
    ) {
        (Some(median), Some(avg)) => format!(
            r#"  <div class="stat"><div class="stat-value">{}</div><div class="stat-label">{}</div></div>
  <div class="stat"><div class="stat-value">{}</div><div class="stat-label">{}</div></div>"#,
            format_duration_short(median),
            html_escape(l.stat_median_response),
            format_duration_short(avg),
            html_escape(l.stat_avg_response),
        ),
        _ => String::new(),
    };

    let code_stats = if report.stats.total_lines_added > 0 || report.stats.total_lines_removed > 0 {
        format!(
            r#"  <div class="stat"><div class="stat-value">+{}/-{}</div><div class="stat-label">{}</div></div>
  <div class="stat"><div class="stat-value">{}</div><div class="stat-label">{}</div></div>"#,
            format_number(report.stats.total_lines_added),
            format_number(report.stats.total_lines_removed),
            html_escape(l.stat_lines),
            format_number(report.stats.total_files_modified),
            html_escape(l.stat_files),
        )
    } else {
        String::new()
    };

    format!(
        r#"<div class="stats-row">
{code_stats}
  <div class="stat"><div class="stat-value">{sessions}</div><div class="stat-label">{l_sessions}</div></div>
  <div class="stat"><div class="stat-value">{messages}</div><div class="stat-label">{l_messages}</div></div>
  <div class="stat"><div class="stat-value">{hours:.1}</div><div class="stat-label">{l_hours}</div></div>
  <div class="stat"><div class="stat-value">{days}</div><div class="stat-label">{l_days}</div></div>
  <div class="stat"><div class="stat-value">{mpd:.1}</div><div class="stat-label">{l_mpd}</div></div>
{response_time_stats}
</div>"#,
        sessions = report.total_sessions,
        messages = report.total_messages,
        hours = report.stats.total_hours,
        days = report.days_covered,
        mpd = report.stats.msgs_per_day,
        l_sessions = html_escape(l.stat_sessions),
        l_messages = html_escape(l.stat_messages),
        l_hours = html_escape(l.stat_hours),
        l_days = html_escape(l.stat_days),
        l_mpd = html_escape(l.stat_msgs_per_day),
    )
}

fn format_duration_short(secs: f64) -> String {
    if secs < 60.0 {
        format!("{:.0}s", secs)
    } else if secs < 3600.0 {
        format!("{:.1}m", secs / 60.0)
    } else {
        format!("{:.1}h", secs / 3600.0)
    }
}

fn format_number(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn render_project_areas(areas: &[ProjectArea], l: &HtmlLabels) -> String {
    if areas.is_empty() {
        return format!(
            r#"<div class="empty">{}</div>"#,
            html_escape(l.no_project_areas)
        );
    }

    let items: Vec<String> = areas
        .iter()
        .map(|a| {
            format!(
                r#"<div class="project-area">
  <div class="area-header">
    <span class="area-name">{name}</span>
    <span class="area-count">~{count} {suffix}</span>
  </div>
  <div class="area-desc">{desc}</div>
</div>"#,
                name = html_escape(&a.name),
                count = a.session_count,
                suffix = html_escape(l.sessions_suffix),
                desc = markdown_inline(&a.description),
            )
        })
        .collect();

    format!(r#"<div class="project-areas">{}</div>"#, items.join("\n"))
}

// ============ Charts split by section ============

fn render_basic_charts(stats: &InsightsStats, l: &HtmlLabels) -> String {
    let goals_chart = render_bar_chart(l.chart_goals, &stats.top_goals, "#2563eb", 6);
    let tools_chart = render_bar_chart(l.chart_tools, &stats.top_tools, "#0891b2", 6);

    let mut lang_items: Vec<(String, u32)> = stats
        .languages
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    lang_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    lang_items.truncate(6);
    let lang_chart = render_bar_chart(l.chart_languages, &lang_items, "#10b981", 6);

    let mut type_items: Vec<(String, u32)> = stats
        .session_types
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    type_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    type_items.truncate(6);
    let types_chart = render_bar_chart(l.chart_session_types, &type_items, "#8b5cf6", 6);

    let row1 = wrap_charts_row(&goals_chart, &tools_chart);
    let row2 = wrap_charts_row(&lang_chart, &types_chart);
    format!("{}{}", row1, row2)
}

fn render_usage_charts(stats: &InsightsStats, l: &HtmlLabels) -> String {
    let mut html = String::new();

    if !stats.response_time_buckets.is_empty() {
        let response_time_chart =
            render_response_time_chart(&stats.response_time_buckets, stats, l);
        html.push_str(&response_time_chart);
    }

    let time_of_day_chart = render_time_of_day_chart(stats, l);

    let mut tool_error_items: Vec<(String, u32)> = stats
        .tool_errors
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    tool_error_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    tool_error_items.truncate(6);
    let tool_errors_chart = render_bar_chart(l.chart_tool_errors, &tool_error_items, "#dc2626", 6);

    let mut agent_types_chart = String::new();
    if !stats.agent_types.is_empty() {
        let mut agent_type_items: Vec<(String, u32)> = stats
            .agent_types
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        agent_type_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        agent_type_items.truncate(6);
        agent_types_chart = render_bar_chart(l.chart_agent_types, &agent_type_items, "#f97316", 6);
    }

    html.push_str(&wrap_charts_row(&time_of_day_chart, &tool_errors_chart));
    if !agent_types_chart.is_empty() {
        html.push_str(&wrap_charts_row(&agent_types_chart, ""));
    }

    html
}

fn render_outcome_charts(stats: &InsightsStats, l: &HtmlLabels) -> String {
    let has_success = !stats.success.is_empty();
    let has_outcomes = !stats.outcomes.is_empty();

    if !has_success && !has_outcomes {
        return String::new();
    }

    let mut success_items: Vec<(String, u32)> =
        stats.success.iter().map(|(k, v)| (k.clone(), *v)).collect();
    success_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    success_items.truncate(6);
    let success_chart = render_bar_chart(l.chart_what_helped, &success_items, "#16a34a", 6);

    let mut outcome_items: Vec<(String, u32)> = stats
        .outcomes
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    outcome_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    outcome_items.truncate(6);
    let outcomes_chart = render_bar_chart(l.chart_outcomes, &outcome_items, "#8b5cf6", 6);

    wrap_charts_row(&success_chart, &outcomes_chart)
}

fn render_friction_charts(stats: &InsightsStats, l: &HtmlLabels) -> String {
    let has_friction = !stats.friction.is_empty();
    let has_satisfaction = !stats.satisfaction.is_empty();

    if !has_friction && !has_satisfaction {
        return String::new();
    }

    let mut friction_items: Vec<(String, u32)> = stats
        .friction
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    friction_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    friction_items.truncate(6);
    let friction_chart = render_bar_chart(l.chart_friction_types, &friction_items, "#dc2626", 6);

    let mut satisfaction_items: Vec<(String, u32)> = stats
        .satisfaction
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    satisfaction_items.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    satisfaction_items.truncate(6);
    let satisfaction_chart =
        render_bar_chart(l.chart_satisfaction, &satisfaction_items, "#eab308", 6);

    wrap_charts_row(&friction_chart, &satisfaction_chart)
}

/// Wraps one or two chart cards into a layout row.
/// - Two non-empty cards → 2-column grid `.charts-row`.
/// - One non-empty card  → standalone full-width (no grid wrapper, just margin).
/// - Both empty           → empty string.
fn wrap_charts_row(card_a: &str, card_b: &str) -> String {
    match (card_a.is_empty(), card_b.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!(
            r#"<div class="charts-row charts-row-single">{}</div>"#,
            card_a
        ),
        (true, false) => format!(
            r#"<div class="charts-row charts-row-single">{}</div>"#,
            card_b
        ),
        (false, false) => format!(r#"<div class="charts-row">{}{}</div>"#, card_a, card_b),
    }
}

// ============ Chart helpers ============

fn render_response_time_chart(
    buckets: &std::collections::HashMap<String, u32>,
    stats: &InsightsStats,
    l: &HtmlLabels,
) -> String {
    let bucket_order = ["2-10s", "10-30s", "30s-1m", "1-2m", "2-5m", "5-15m", ">15m"];
    let ordered_items: Vec<(String, u32)> = bucket_order
        .iter()
        .filter_map(|&label| {
            buckets.get(label).and_then(|&v| {
                if v > 0 {
                    Some((label.to_string(), v))
                } else {
                    None
                }
            })
        })
        .collect();

    if ordered_items.is_empty() {
        return String::new();
    }

    let max_val = ordered_items.iter().map(|(_, v)| *v).max().unwrap_or(1) as f64;
    let bars: String = ordered_items.iter().map(|(label, value)| {
        let pct = (*value as f64 / max_val) * 100.0;
        format!(
            r#"<div class="bar-row"><div class="bar-label">{}</div><div class="bar-track"><div class="bar-fill" style="width:{:.1}%;background:#6366f1"></div></div><div class="bar-value">{}</div></div>"#,
            html_escape(label), pct, value,
        )
    }).collect();

    let footer = match (
        stats.median_response_time_secs,
        stats.avg_response_time_secs,
    ) {
        (Some(median), Some(avg)) => format!(
            r#"<div style="font-size:12px;color:#64748b;margin-top:8px">{}: {:.1}s &bull; {}: {:.1}s</div>"#,
            html_escape(l.median_label),
            median,
            html_escape(l.average_label),
            avg,
        ),
        _ => String::new(),
    };

    format!(
        r#"<div class="chart-card" style="margin:24px 0"><div class="chart-title">{}</div>{}{}</div>"#,
        html_escape(l.chart_response_time),
        bars,
        footer,
    )
}

const TIME_PERIODS: [std::ops::Range<u32>; 4] = [6..12, 12..18, 18..24, 0..6];

fn render_time_of_day_chart(stats: &InsightsStats, l: &HtmlLabels) -> String {
    let labels = [
        l.time_morning,
        l.time_afternoon,
        l.time_evening,
        l.time_night,
    ];
    let items = labels
        .iter()
        .zip(TIME_PERIODS)
        .map(|(label, hours)| {
            let count = hours
                .map(|hour| stats.hour_counts.get(&hour).copied().unwrap_or(0))
                .sum();
            (label.to_string(), count)
        })
        .collect::<Vec<_>>();

    // Legacy reports only have local hour counts, so there is no reliable way
    // to convert them to a different time zone.
    if stats.activity_by_utc_minute.is_empty() {
        return render_bar_chart(l.chart_time_of_day, &items, "#8b5cf6", 4);
    }

    let minutes = serde_json::to_string(&stats.activity_by_utc_minute).unwrap_or_default();
    let labels = serde_json::to_string(&labels).unwrap_or_default();
    format!(
        r#"<div class="chart-card" id="time-of-day-chart" data-minutes="{minutes}" data-labels="{labels}">
  <div class="chart-title" style="display:flex;justify-content:space-between;align-items:center">
    <span>{title}</span>
    <select id="tz-selector" class="tz-select" aria-label="{title}"></select>
  </div>
  <div id="time-bars"></div>
</div>"#,
        title = html_escape(l.chart_time_of_day),
        minutes = html_escape(&minutes),
        labels = html_escape(&labels),
    )
}

fn render_bar_chart(title: &str, items: &[(String, u32)], color: &str, max_items: usize) -> String {
    let non_zero: Vec<&(String, u32)> = items.iter().filter(|(_, v)| *v > 0).collect();

    if non_zero.is_empty() {
        return String::new();
    }

    let max_val = non_zero.iter().map(|(_, v)| *v).max().unwrap_or(1) as f64;
    let bars: Vec<String> = non_zero
        .iter()
        .take(max_items)
        .map(|(label, value)| {
            let pct = (*value as f64 / max_val) * 100.0;
            let display_label = label
                .replace('_', " ")
                .split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().to_string() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                r#"<div class="bar-row">
  <div class="bar-label">{}</div>
  <div class="bar-track"><div class="bar-fill" style="width:{:.1}%;background:{}"></div></div>
  <div class="bar-value">{}</div>
</div>"#,
                html_escape(&display_label),
                pct,
                color,
                value,
            )
        })
        .collect();

    format!(
        r#"<div class="chart-card"><div class="chart-title">{}</div>{}</div>"#,
        html_escape(title),
        bars.join("\n"),
    )
}

// ============ Content sections ============

fn render_interaction_style(style: &InteractionStyle, l: &HtmlLabels) -> String {
    if style.narrative.is_empty() && style.key_patterns.is_empty() {
        return format!(
            r#"<div class="empty">{}</div>"#,
            html_escape(l.no_interaction_style)
        );
    }

    let patterns_html = if style.key_patterns.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = style
            .key_patterns
            .iter()
            .map(|p| format!(r#"<div class="key-insight">{}</div>"#, markdown_inline(p)))
            .collect();
        items.join("\n")
    };

    format!(
        r#"<div class="narrative">
  <p>{}</p>
  {}
</div>"#,
        markdown_inline(&style.narrative),
        patterns_html,
    )
}

fn render_big_wins(wins: &[BigWin], l: &HtmlLabels) -> String {
    if wins.is_empty() {
        return format!(r#"<div class="empty">{}</div>"#, html_escape(l.no_big_wins));
    }

    let items: Vec<String> = wins
        .iter()
        .map(|w| {
            let impact_html = if w.impact.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="big-win-impact">{}</div>"#,
                    markdown_inline(&w.impact)
                )
            };
            format!(
                r#"<div class="big-win">
  <div class="big-win-title">{}</div>
  <div class="big-win-desc">{}</div>
  {}
</div>"#,
                html_escape(&w.title),
                markdown_inline(&w.description),
                impact_html,
            )
        })
        .collect();

    format!(r#"<div class="big-wins">{}</div>"#, items.join("\n"))
}

fn render_friction_categories(categories: &[FrictionCategory], l: &HtmlLabels) -> String {
    if categories.is_empty() {
        return format!(r#"<div class="empty">{}</div>"#, html_escape(l.no_friction));
    }

    let items: Vec<String> = categories
        .iter()
        .map(|f| {
            let examples_html = if f.examples.is_empty() {
                String::new()
            } else {
                let lis: Vec<String> = f
                    .examples
                    .iter()
                    .map(|e| format!("<li>{}</li>", markdown_inline(e)))
                    .collect();
                format!(
                    r#"<ul class="friction-examples">{}</ul>"#,
                    lis.join("\n")
                )
            };

            let suggestion_html = if f.suggestion.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="key-insight" style="background:#fef2f2;border-color:#fca5a5;color:#991b1b;margin-top:10px">{}</div>"#,
                    markdown_inline(&f.suggestion)
                )
            };

            format!(
                r#"<div class="friction-category">
  <div class="friction-title">{}</div>
  <div class="friction-desc">{}</div>
  {}
  {}
</div>"#,
                html_escape(&f.category),
                markdown_inline(&f.description),
                examples_html,
                suggestion_html,
            )
        })
        .collect();

    format!(
        r#"<div class="friction-categories">{}</div>"#,
        items.join("\n")
    )
}

fn render_suggestions(suggestions: &InsightsSuggestions, l: &HtmlLabels) -> String {
    let copy = html_escape(l.copy);
    let mut sections = Vec::new();

    if !suggestions.openbitfun_md_additions.is_empty() {
        let items: Vec<String> = suggestions
            .openbitfun_md_additions
            .iter()
            .enumerate()
            .map(|(i, md)| {
                format!(
                    r#"<div class="claude-md-item">
  <input type="checkbox" class="cmd-checkbox" id="md-{i}" checked>
  <div class="cmd-code">{}</div>
  <button class="copy-btn">&nbsp;{copy}&nbsp;</button>
  <div class="cmd-why">{}</div>
</div>"#,
                    html_escape(&md.content),
                    html_escape(&md.rationale),
                    i = i,
                )
            })
            .collect();

        sections.push(format!(
            r#"<div class="claude-md-section">
  <h3>{md_title}</h3>
  <div class="claude-md-actions">
    <button class="copy-all-btn">{copy_all}</button>
  </div>
  {items}
</div>"#,
            md_title = html_escape(l.md_additions),
            copy_all = html_escape(l.copy_all_checked),
            items = items.join("\n"),
        ));
    }

    if !suggestions.features_to_try.is_empty() {
        let items: Vec<String> = suggestions
            .features_to_try
            .iter()
            .map(|f| {
                let code_html = if f.example_usage.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<div class="feature-code">
  <code>{}</code>
  <button class="copy-btn">&nbsp;{copy}&nbsp;</button>
</div>"#,
                        html_escape(&f.example_usage),
                    )
                };

                format!(
                    r#"<div class="feature-card">
  <div class="feature-title">{}</div>
  <div class="feature-oneliner">{}</div>
  <div class="feature-why">{}</div>
  {}
</div>"#,
                    html_escape(&f.feature),
                    markdown_inline(&f.description),
                    markdown_inline(&f.benefit),
                    code_html,
                )
            })
            .collect();

        sections.push(format!(
            r#"<h3 id="section-features">{}</h3>
<div class="features-section">{}</div>"#,
            html_escape(l.features_to_try),
            items.join("\n")
        ));
    }

    if !suggestions.usage_patterns.is_empty() {
        let items: Vec<String> = suggestions
            .usage_patterns
            .iter()
            .map(|p| {
                let detail_html = if p.detail.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<div class="pattern-detail">{}</div>"#,
                        markdown_inline(&p.detail)
                    )
                };

                let prompt_html = if p.suggested_prompt.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#"<div class="pattern-prompt">
  <div class="prompt-label">{}</div>
  <code>{}</code>
  <button class="copy-btn">&nbsp;{copy}&nbsp;</button>
</div>"#,
                        html_escape(l.try_this_prompt),
                        html_escape(&p.suggested_prompt),
                    )
                };

                format!(
                    r#"<div class="pattern-card">
  <div class="pattern-title">{}</div>
  <div class="pattern-summary">{}</div>
  {}
  {}
</div>"#,
                    html_escape(&p.pattern),
                    markdown_inline(&p.description),
                    detail_html,
                    prompt_html,
                )
            })
            .collect();

        sections.push(format!(
            r#"<h3 id="section-patterns">{}</h3>
<div class="patterns-section">{}</div>"#,
            html_escape(l.usage_patterns),
            items.join("\n")
        ));
    }

    sections.join("\n")
}

fn render_horizon(intro: &str, workflows: &[HorizonWorkflow], l: &HtmlLabels) -> String {
    let copy = html_escape(l.copy);
    if workflows.is_empty() {
        return format!(r#"<div class="empty">{}</div>"#, html_escape(l.no_horizon));
    }

    let intro_html = if intro.is_empty() {
        String::new()
    } else {
        format!(r#"<p class="section-intro">{}</p>"#, markdown_inline(intro))
    };

    let items: Vec<String> = workflows
        .iter()
        .map(|h| {
            let how_to_try_html = if h.how_to_try.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<div class="horizon-tip">{}</div>"#,
                    markdown_inline(&h.how_to_try)
                )
            };

            let prompt_html = if h.copyable_prompt.is_empty() {
                String::new()
            } else {
                let escaped = html_escape(&h.copyable_prompt);
                format!(
                    r#"<div class="horizon-prompt">
  <div class="prompt-label">{try_prompt}</div>
  <div class="feature-code">
    <code>{code}</code>
    <button class="copy-btn">{copy}</button>
  </div>
</div>"#,
                    try_prompt = html_escape(l.try_this_prompt),
                    code = escaped,
                )
            };

            format!(
                r#"<div class="horizon-card">
  <div class="horizon-title">{}</div>
  <div class="horizon-possible">{}</div>
  {}
  {}
</div>"#,
                html_escape(&h.title),
                markdown_inline(&h.whats_possible),
                how_to_try_html,
                prompt_html,
            )
        })
        .collect();

    format!(
        r#"{}<div class="horizon-section">{}</div>"#,
        intro_html,
        items.join("\n")
    )
}

fn render_fun_ending(ending: &Option<FunEnding>) -> String {
    match ending {
        Some(fe) => format!(
            r#"<div class="fun-ending">
  <div class="fun-headline">{}</div>
  <div class="fun-detail">{}</div>
</div>"#,
            html_escape(&fe.headline),
            markdown_inline(&fe.detail),
        ),
        None => String::new(),
    }
}

// ============ Utilities ============

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Convert simple markdown inline formatting to HTML.
/// Handles **bold** and *italic* after html_escape.
fn markdown_inline(s: &str) -> String {
    let escaped = html_escape(s);
    let mut result = String::with_capacity(escaped.len() + 64);
    let chars: Vec<char> = escaped.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_closing_double_star(&chars, i + 2) {
                result.push_str("<strong>");
                for &c in &chars[i + 2..end] {
                    result.push(c);
                }
                result.push_str("</strong>");
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '*' && (i + 1 < len && chars[i + 1] != '*') {
            if let Some(end) = find_closing_single_star(&chars, i + 1) {
                result.push_str("<em>");
                for &c in &chars[i + 1..end] {
                    result.push(c);
                }
                result.push_str("</em>");
                i = end + 1;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    result
}

fn find_closing_double_star(chars: &[char], start: usize) -> Option<usize> {
    let len = chars.len();
    let mut i = start;
    while i + 1 < len {
        if chars[i] == '*' && chars[i + 1] == '*' && i > start {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_closing_single_star(chars: &[char], start: usize) -> Option<usize> {
    let len = chars.len();
    let mut i = start;
    while i < len {
        if chars[i] == '*' && (i + 1 >= len || chars[i + 1] != '*') && i > start {
            return Some(i);
        }
        i += 1;
    }
    None
}

const CSS_STYLES: &str = r#"
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body { font-family: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif; background: #f8fafc; color: #334155; line-height: 1.65; padding: 48px 24px; }
    .container { max-width: 800px; margin: 0 auto; }
    h1 { font-size: 32px; font-weight: 700; color: #0f172a; margin-bottom: 8px; }
    h2 { font-size: 20px; font-weight: 600; color: #0f172a; margin-top: 48px; margin-bottom: 16px; }
    h3 { font-size: 16px; font-weight: 600; color: #0f172a; margin-top: 24px; margin-bottom: 12px; }
    .subtitle { color: #64748b; font-size: 15px; margin-bottom: 32px; }
    .nav-toc { display: flex; flex-wrap: wrap; gap: 8px; margin: 24px 0 32px 0; padding: 16px; background: white; border-radius: 8px; border: 1px solid #e2e8f0; }
    .nav-toc a { font-size: 12px; color: #64748b; text-decoration: none; padding: 6px 12px; border-radius: 6px; background: #f1f5f9; transition: all 0.15s; }
    .nav-toc a:hover { background: #e2e8f0; color: #334155; }
    .stats-row { display: flex; gap: 24px; margin-bottom: 40px; padding: 20px 0; border-top: 1px solid #e2e8f0; border-bottom: 1px solid #e2e8f0; flex-wrap: wrap; }
    .stat { text-align: center; }
    .stat-value { font-size: 24px; font-weight: 700; color: #0f172a; }
    .stat-label { font-size: 11px; color: #64748b; text-transform: uppercase; }
    .at-a-glance { background: linear-gradient(135deg, #fef3c7 0%, #fde68a 100%); border: 1px solid #f59e0b; border-radius: 12px; padding: 20px 24px; margin-bottom: 32px; }
    .glance-title { font-size: 16px; font-weight: 700; color: #92400e; margin-bottom: 16px; }
    .glance-sections { display: flex; flex-direction: column; gap: 12px; }
    .glance-section { font-size: 14px; color: #78350f; line-height: 1.6; }
    .glance-section strong { color: #92400e; }
    .see-more { color: #b45309; text-decoration: none; font-size: 13px; white-space: nowrap; }
    .see-more:hover { text-decoration: underline; }
    .project-areas { display: flex; flex-direction: column; gap: 12px; margin-bottom: 32px; }
    .project-area { background: white; border: 1px solid #e2e8f0; border-radius: 8px; padding: 16px; }
    .area-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }
    .area-name { font-weight: 600; font-size: 15px; color: #0f172a; }
    .area-count { font-size: 12px; color: #64748b; background: #f1f5f9; padding: 2px 8px; border-radius: 4px; }
    .area-desc { font-size: 14px; color: #475569; line-height: 1.5; }
    .narrative { background: white; border: 1px solid #e2e8f0; border-radius: 8px; padding: 20px; margin-bottom: 24px; }
    .narrative p { margin-bottom: 12px; font-size: 14px; color: #475569; line-height: 1.7; }
    .key-insight { background: #f0fdf4; border: 1px solid #bbf7d0; border-radius: 8px; padding: 12px 16px; margin-top: 12px; font-size: 14px; color: #166534; }
    .section-intro { font-size: 14px; color: #475569; line-height: 1.6; margin-bottom: 16px; }
    .big-wins { display: flex; flex-direction: column; gap: 12px; margin-bottom: 24px; }
    .big-win { background: #f0fdf4; border: 1px solid #bbf7d0; border-radius: 8px; padding: 16px; }
    .big-win-title { font-weight: 600; font-size: 15px; color: #166534; margin-bottom: 8px; }
    .big-win-desc { font-size: 14px; color: #15803d; line-height: 1.5; }
    .big-win-impact { font-size: 12px; color: #166534; opacity: 0.8; font-style: italic; margin-top: 6px; }
    .friction-categories { display: flex; flex-direction: column; gap: 16px; margin-bottom: 24px; }
    .friction-category { background: #fef2f2; border: 1px solid #fca5a5; border-radius: 8px; padding: 16px; }
    .friction-title { font-weight: 600; font-size: 15px; color: #991b1b; margin-bottom: 6px; }
    .friction-desc { font-size: 13px; color: #7f1d1d; margin-bottom: 10px; }
    .friction-examples { margin: 0 0 0 20px; font-size: 13px; color: #334155; }
    .friction-examples li { margin-bottom: 4px; }
    .claude-md-section { background: #eff6ff; border: 1px solid #bfdbfe; border-radius: 8px; padding: 16px; margin-bottom: 20px; }
    .claude-md-section h3 { font-size: 14px; font-weight: 600; color: #1e40af; margin: 0 0 12px 0; }
    .claude-md-actions { margin-bottom: 12px; padding-bottom: 12px; border-bottom: 1px solid #dbeafe; }
    .copy-all-btn { background: #2563eb; color: white; border: none; border-radius: 4px; padding: 6px 12px; font-size: 12px; cursor: pointer; font-weight: 500; transition: all 0.2s; }
    .copy-all-btn:hover { background: #1d4ed8; }
    .copy-all-btn.copied { background: #16a34a; }
    .claude-md-item { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 8px; padding: 10px 0; border-bottom: 1px solid #dbeafe; }
    .claude-md-item:last-child { border-bottom: none; }
    .cmd-checkbox { margin-top: 2px; }
    .cmd-code { background: white; padding: 8px 12px; border-radius: 4px; font-size: 12px; color: #1e40af; border: 1px solid #bfdbfe; font-family: monospace; display: block; white-space: pre-wrap; word-break: break-word; flex: 1; }
    .cmd-why { font-size: 12px; color: #64748b; width: 100%; padding-left: 24px; margin-top: 4px; }
    .features-section, .patterns-section { display: flex; flex-direction: column; gap: 12px; margin: 16px 0; }
    .feature-card { background: #f0fdf4; border: 1px solid #86efac; border-radius: 8px; padding: 16px; }
    .pattern-card { background: #f0f9ff; border: 1px solid #7dd3fc; border-radius: 8px; padding: 16px; }
    .feature-title, .pattern-title { font-weight: 600; font-size: 15px; color: #0f172a; margin-bottom: 6px; }
    .feature-oneliner { font-size: 14px; color: #475569; margin-bottom: 8px; }
    .pattern-summary { font-size: 14px; color: #475569; margin-bottom: 8px; }
    .feature-why { font-size: 13px; color: #334155; line-height: 1.5; }
    .feature-code { background: #f8fafc; padding: 12px; border-radius: 6px; margin-top: 12px; border: 1px solid #e2e8f0; display: flex; align-items: flex-start; gap: 8px; }
    .feature-code code { flex: 1; font-family: monospace; font-size: 12px; color: #334155; white-space: pre-wrap; }
    .pattern-prompt { background: #f8fafc; padding: 12px; border-radius: 6px; margin-top: 12px; border: 1px solid #e2e8f0; }
    .pattern-prompt code { font-family: monospace; font-size: 12px; color: #334155; display: block; white-space: pre-wrap; margin-bottom: 8px; }
    .prompt-label { font-size: 11px; font-weight: 600; text-transform: uppercase; color: #64748b; margin-bottom: 6px; }
    .copy-btn { background: #e2e8f0; border: none; border-radius: 4px; padding: 4px 8px; font-size: 11px; cursor: pointer; color: #475569; flex-shrink: 0; }
    .copy-btn:hover { background: #cbd5e1; }
    .charts-row { display: grid; grid-template-columns: 1fr 1fr; gap: 24px; margin: 24px 0; }
    .charts-row-single { grid-template-columns: 1fr; }
    .chart-card { background: white; border: 1px solid #e2e8f0; border-radius: 8px; padding: 16px; }
    .chart-title { font-size: 12px; font-weight: 600; color: #64748b; text-transform: uppercase; margin-bottom: 12px; }
    .bar-row { display: flex; align-items: center; margin-bottom: 6px; }
    .bar-label { width: 100px; font-size: 11px; color: #475569; flex-shrink: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .bar-track { flex: 1; height: 6px; background: #f1f5f9; border-radius: 3px; margin: 0 8px; }
    .bar-fill { height: 100%; border-radius: 3px; }
    .bar-value { width: 28px; font-size: 11px; font-weight: 500; color: #64748b; text-align: right; }
    .empty { color: #94a3b8; font-size: 13px; padding: 12px 0; }
    .tz-select { font-size: 11px; padding: 2px 6px; border: 1px solid #e2e8f0; border-radius: 4px; background: #f8fafc; color: #475569; cursor: pointer; }
    .horizon-section { display: flex; flex-direction: column; gap: 16px; }
    .horizon-card { background: linear-gradient(135deg, #faf5ff 0%, #f5f3ff 100%); border: 1px solid #c4b5fd; border-radius: 8px; padding: 16px; }
    .horizon-title { font-weight: 600; font-size: 15px; color: #5b21b6; margin-bottom: 8px; }
    .horizon-possible { font-size: 14px; color: #334155; margin-bottom: 10px; line-height: 1.5; }
    .horizon-steps { margin: 0 0 0 20px; font-size: 13px; color: #6b21a8; }
    .horizon-steps li { margin-bottom: 4px; }
    .horizon-tip { font-size: 13px; color: #5b21b6; background: #ede9fe; border-radius: 6px; padding: 8px 12px; margin-top: 10px; line-height: 1.5; }
    .horizon-prompt { margin-top: 10px; }
    .fun-ending { background: linear-gradient(135deg, #fef3c7 0%, #fde68a 100%); border: 1px solid #fbbf24; border-radius: 12px; padding: 24px; margin-top: 40px; text-align: center; }
    .fun-headline { font-size: 18px; font-weight: 600; color: #78350f; margin-bottom: 8px; }
    .fun-detail { font-size: 14px; color: #92400e; }
    @media (max-width: 640px) { .charts-row { grid-template-columns: 1fr; } .stats-row { justify-content: center; } }
"#;

const JS_SCRIPT: &str = r#"
    document.querySelectorAll('.copy-btn').forEach(function(btn) {
      btn.addEventListener('click', function() {
        var item = btn.closest('.feature-code, .claude-md-item');
        var code = item && item.querySelector('.cmd-code, code');
        if (code) copyText(btn, code.textContent);
      });
    });
    document.querySelectorAll('.copy-all-btn').forEach(function(btn) {
      btn.addEventListener('click', function() { copyAllChecked(btn); });
    });

    function copyText(btn, text) {
      navigator.clipboard.writeText(text).then(function() {
        var orig = btn.textContent;
        btn.textContent = ' __COPIED__ ';
        btn.style.background = '#16a34a';
        btn.style.color = 'white';
        setTimeout(function() {
          btn.textContent = orig;
          btn.style.background = '';
          btn.style.color = '';
        }, 2000);
      });
    }

    function copyAllChecked(btn) {
      var section = btn.closest('.claude-md-section');
      var items = section.querySelectorAll('.claude-md-item');
      var texts = [];
      items.forEach(function(item) {
        var cb = item.querySelector('.cmd-checkbox');
        if (cb && cb.checked) {
          var code = item.querySelector('.cmd-code');
          if (code) texts.push(code.textContent.trim());
        }
      });
      if (texts.length === 0) return;
      navigator.clipboard.writeText(texts.join('\n\n')).then(function() {
        btn.textContent = '__COPIED__';
        btn.classList.add('copied');
        setTimeout(function() {
          btn.textContent = '__COPY_ALL_CHECKED__';
          btn.classList.remove('copied');
        }, 2000);
      });
    }

    (function initTimezoneSelector() {
      var sel = document.getElementById('tz-selector');
      var chart = document.getElementById('time-of-day-chart');
      if (!sel || !chart) return;
      var minutes = JSON.parse(chart.dataset.minutes);
      var labels = JSON.parse(chart.dataset.labels);
      var common = [
        'UTC',
        'America/New_York','America/Chicago','America/Denver','America/Los_Angeles',
        'Europe/London','Europe/Paris','Europe/Berlin',
        'Asia/Tokyo','Asia/Shanghai','Asia/Kolkata','Asia/Singapore',
        'Australia/Sydney','Pacific/Auckland'
      ];
      var localTz = Intl.DateTimeFormat().resolvedOptions().timeZone;
      if (common.indexOf(localTz) === -1) common.unshift(localTz);
      common.forEach(function(tz) {
        var opt = document.createElement('option');
        opt.value = tz;
        opt.textContent = tz.replace(/_/g,' ');
        if (tz === localTz) opt.selected = true;
        sel.appendChild(opt);
      });
      function update() { updateTimeChart(sel.value, minutes, labels); }
      sel.addEventListener('change', update);
      update();
    })();

    function updateTimeChart(tz, minutes, labels) {
      var container = document.getElementById('time-bars');
      var formatHour = new Intl.DateTimeFormat('en-GB', {
        hour: 'numeric', hourCycle: 'h23', timeZone: tz
      });
      var counts = [0, 0, 0, 0];
      Object.entries(minutes).forEach(function(entry) {
        var hour = Number(formatHour.format(new Date(Number(entry[0]) * 60000)));
        var period = (Math.floor(hour / 6) + 3) % 4;
        counts[period] += entry[1];
      });
      var periods = labels.map(function(label, i) { return {label:label, count:counts[i]}; });
      var maxVal = 0;
      periods.forEach(function(p) { maxVal = Math.max(maxVal, p.count); });
      container.replaceChildren();
      periods.forEach(function(d) {
        var row = document.createElement('div');
        row.className = 'bar-row';
        var label = document.createElement('span');
        label.className = 'bar-label';
        label.textContent = d.label;
        var track = document.createElement('div');
        track.className = 'bar-track';
        var fill = document.createElement('div');
        fill.className = 'bar-fill';
        fill.style.width = (maxVal > 0 ? d.count / maxVal * 100 : 0) + '%';
        fill.style.background = '#8b5cf6';
        track.appendChild(fill);
        var value = document.createElement('span');
        value.className = 'bar-value';
        value.textContent = d.count;
        row.append(label, track, value);
        container.appendChild(row);
      });
    }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_buttons_treat_model_output_as_text_in_every_section() {
        let labels = HtmlLabels::for_locale(LocaleId::EnUS);
        let text = "\"quotes\" & <examples> 'apostrophes'\nsecond line".to_owned();
        let suggestions = render_suggestions(
            &InsightsSuggestions {
                openbitfun_md_additions: vec![MdAddition {
                    section: "Section".to_owned(),
                    content: text.clone(),
                    rationale: String::new(),
                }],
                features_to_try: vec![FeatureRecommendation {
                    feature: "Feature".to_owned(),
                    description: String::new(),
                    example_usage: text.clone(),
                    benefit: String::new(),
                }],
                usage_patterns: vec![UsagePattern {
                    pattern: "Pattern".to_owned(),
                    description: String::new(),
                    detail: String::new(),
                    suggested_prompt: text.clone(),
                }],
            },
            &labels,
        );
        let horizon = render_horizon(
            "",
            &[HorizonWorkflow {
                title: "Workflow".to_owned(),
                whats_possible: String::new(),
                how_to_try: String::new(),
                copyable_prompt: text.clone(),
            }],
            &labels,
        );

        let html = suggestions + &horizon;
        assert_eq!(html.matches(&html_escape(&text)).count(), 4);
        assert_eq!(html.matches("class=\"copy-btn\"").count(), 4);
        assert!(!html.contains(&text));
        assert!(!html.contains("onclick="));
    }
}
