use cursive::{
    Cursive, event,
    theme::{Color, Style},
    utils::markup::StyledString,
    views::TextView,
};

/// Shows a prompt below the status bar line prompting the user if they
/// want to quit the application.
pub(crate) fn show_quit_prompt(siv: &mut Cursive) {
    siv.call_on_name("prompt_bar", |v: &mut TextView| {
        let style = Style::from(Color::Rgb(255, 238, 140));
        v.set_content(StyledString::styled("Quit [y/N]?", style));
    });

    siv.add_global_callback('y', |s| s.quit());
    siv.add_global_callback('Y', |s| s.quit());

    let cancel_prompt = |siv: &mut cursive::Cursive| {
        siv.clear_global_callbacks('y');
        siv.clear_global_callbacks('Y');
        siv.clear_global_callbacks('n');
        siv.clear_global_callbacks('N');
        siv.call_on_name("prompt_bar", |v: &mut TextView| {
            v.set_content("");
        });
    };

    siv.add_global_callback('n', cancel_prompt);
    siv.add_global_callback('N', cancel_prompt);
    siv.add_global_callback(event::Key::Esc, cancel_prompt);
}
