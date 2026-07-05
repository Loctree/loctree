//! True duplicate: identical signature and body as render_b.rs.

pub fn render_widget(config: WidgetConfig) -> WidgetHtml {
    WidgetHtml::from_template("widget", config.theme_token)
}
