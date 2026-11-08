mod app;
mod claude;
mod code_view;
mod files;
mod fonts;
mod highlight;
mod selectable_text;
mod settings;
mod style;
mod tracker;
mod view;
mod walkthrough;

fn main() -> iced::Result {
    // The UI font can only be set when the application starts.
    let settings = settings::Settings::load();
    fonts::set_ui(&settings.ui_font);
    fonts::set_code(&settings.code_font, settings.code_font_size);
    iced::application(app::App::new, app::App::update, app::App::view)
        .default_font(fonts::ui())
        .scale_factor(app::App::scale_factor)
        .title(app::App::title)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window_size((1500.0, 950.0))
        .antialiasing(true)
        .run()
}
