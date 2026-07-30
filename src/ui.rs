use fltk::{
    app,
    button::Button,
    draw,
    enums::{Align, Color, Event, Font, FrameType},
    frame::Frame,
    group::{Pack, PackType},
    input::{Input, MultilineInput},
    prelude::*,
    window::DoubleWindow,
};

use crate::{
    model::{Config, Orientation, Prompt, WindowPosition},
    platform,
};

const STRIP_HEIGHT: i32 = 31;
const EDGE: i32 = 3;
const GRIP_WIDTH: i32 = 24;
const SPACING: i32 = 2;
const EDITOR_GAP: i32 = 8;
const BUTTON_HORIZONTAL_PADDING: i32 = 18;
const MIN_BUTTON_WIDTH: i32 = 48;
const MAX_BUTTON_WIDTH: i32 = 180;
const SCREEN_MARGIN: i32 = 12;
const TOPMOST_REFRESH_SECONDS: f64 = 0.25;

#[derive(Clone, Debug)]
pub enum Message {
    Insert(usize),
    Edit(usize),
    Create,
    ShowConfig,
    ToggleOrientation,
    Save {
        index: usize,
        title: String,
        text: String,
    },
    Duplicate(usize),
    AddAfter(usize),
    Delete(usize),
    Moved(WindowPosition),
    CloseEditor,
    Quit,
}

pub struct Strip {
    window: DoubleWindow,
    pack: Pack,
    sender: app::Sender<Message>,
}

impl Strip {
    pub fn new(sender: app::Sender<Message>) -> Self {
        let mut window = DoubleWindow::new(0, 0, 360, STRIP_HEIGHT, "Promplet");
        window.set_border(false);
        window.set_color(Color::from_rgb(192, 192, 192));

        let mut pack = Pack::new(
            EDGE,
            EDGE,
            window.width() - EDGE * 2,
            STRIP_HEIGHT - EDGE * 2,
            "",
        );
        pack.set_type(PackType::Horizontal);
        pack.set_spacing(SPACING);
        pack.end();
        window.end();

        Self {
            window,
            pack,
            sender,
        }
    }

    pub fn window(&self) -> DoubleWindow {
        self.window.clone()
    }

    pub fn start_topmost_keeper(&self) {
        let window = self.window.clone();
        let mut reported_error = false;

        app::add_timeout3(0.0, move |handle| {
            if !window.shown() {
                return;
            }

            match platform::maintain_strip_z_order(&window) {
                Ok(()) => reported_error = false,
                Err(error) if !reported_error => {
                    eprintln!("Could not keep Promplet on top: {error}");
                    reported_error = true;
                }
                Err(_) => {}
            }
            app::repeat_timeout3(TOPMOST_REFRESH_SECONDS, handle);
        });
    }

    pub fn rebuild(&mut self, config: &Config) {
        let orientation = config.orientation;

        self.pack.clear();
        self.pack.set_type(match orientation {
            Orientation::Horizontal => PackType::Horizontal,
            Orientation::Vertical => PackType::Vertical,
        });
        self.pack.begin();

        self.add_drag_grip(orientation);
        for (index, prompt) in config.prompts.iter().enumerate() {
            self.add_prompt_button(index, prompt, orientation);
        }

        self.pack.end();

        let (width, height) = strip_dimensions(config);
        let old_right = self.window.x() + self.window.width();
        let old_bottom = self.window.y() + self.window.height();

        self.window
            .resize(old_right - width, old_bottom - height, width, height);
        self.pack
            .resize(EDGE, EDGE, width - EDGE * 2, height - EDGE * 2);
        self.pack.redraw();
        self.window.redraw();
    }

    pub fn position(&self) -> WindowPosition {
        WindowPosition {
            x: self.window.x(),
            y: self.window.y(),
        }
    }

    pub fn show(
        &mut self,
        saved_position: Option<WindowPosition>,
    ) -> Result<WindowPosition, String> {
        if let Some(position) = saved_position {
            self.window.set_pos(position.x, position.y);
        }
        self.window.show();
        self.window.set_on_top();

        platform::configure_strip_window(&self.window)?;
        if saved_position.is_none() {
            platform::position_bottom_right(&self.window, SCREEN_MARGIN)?;
        }
        let (x, y) = platform::clamp_to_work_area(&self.window, SCREEN_MARGIN)?;
        Ok(WindowPosition { x, y })
    }

    fn add_drag_grip(&mut self, orientation: Orientation) {
        let inner_thickness = STRIP_HEIGHT - EDGE * 2;
        let (width, height) = match orientation {
            Orientation::Horizontal => (GRIP_WIDTH, inner_thickness),
            Orientation::Vertical => (inner_thickness, GRIP_WIDTH),
        };
        let mut grip = Frame::new(0, 0, width, height, "");
        grip.set_frame(FrameType::NoBox);
        grip.draw(move |grip| {
            draw::set_draw_color(Color::from_rgb(192, 192, 192));
            draw::draw_rectf(grip.x(), grip.y(), grip.w(), grip.h());

            let (columns, rows) = match orientation {
                Orientation::Horizontal => (3, 4),
                Orientation::Vertical => (4, 3),
            };
            const DOT_SIZE: i32 = 3;
            const DOT_GAP: i32 = 2;
            let texture_width = columns * DOT_SIZE + (columns - 1) * DOT_GAP;
            let texture_height = rows * DOT_SIZE + (rows - 1) * DOT_GAP;
            let start_x = grip.x() + (grip.w() - texture_width) / 2;
            let start_y = grip.y() + (grip.h() - texture_height) / 2;

            for row in 0..rows {
                for column in 0..columns {
                    let x = start_x + column * (DOT_SIZE + DOT_GAP);
                    let y = start_y + row * (DOT_SIZE + DOT_GAP);

                    draw::set_draw_color(Color::from_rgb(96, 96, 96));
                    draw::draw_rectf(x + 1, y + 1, 2, 2);
                    draw::set_draw_color(Color::from_rgb(232, 232, 232));
                    draw::draw_rectf(x, y, 2, 2);
                }
            }
        });
        grip.set_tooltip("Drag Promplet · Right-click for menu");

        let mut window = self.window.clone();
        let sender = self.sender;
        let mut drag_origin: Option<(i32, i32, i32, i32)> = None;
        let mut right_button_down = false;
        grip.handle(move |_, event| match event {
            Event::Push if app::event_button() == 1 => {
                drag_origin = Some((
                    app::event_x_root(),
                    app::event_y_root(),
                    window.x(),
                    window.y(),
                ));
                true
            }
            Event::Push if app::event_button() == 3 => {
                right_button_down = true;
                true
            }
            Event::Drag if right_button_down => true,
            Event::Drag => {
                if let Some((mouse_x, mouse_y, window_x, window_y)) = drag_origin {
                    window.set_pos(
                        window_x + app::event_x_root() - mouse_x,
                        window_y + app::event_y_root() - mouse_y,
                    );
                    true
                } else {
                    false
                }
            }
            Event::Released if drag_origin.take().is_some() => {
                if let Err(error) = platform::maintain_strip_z_order(&window) {
                    eprintln!("Could not keep Promplet on top: {error}");
                }
                sender.send(Message::Moved(WindowPosition {
                    x: window.x(),
                    y: window.y(),
                }));
                true
            }
            Event::Released if right_button_down => {
                right_button_down = false;
                match platform::show_strip_menu(
                    &window,
                    app::event_x_root(),
                    app::event_y_root(),
                    orientation,
                ) {
                    Ok(Some(platform::StripMenuAction::Create)) => sender.send(Message::Create),
                    Ok(Some(platform::StripMenuAction::ToggleOrientation)) => {
                        sender.send(Message::ToggleOrientation)
                    }
                    Ok(Some(platform::StripMenuAction::ShowConfig)) => {
                        sender.send(Message::ShowConfig)
                    }
                    Ok(Some(platform::StripMenuAction::Quit)) => sender.send(Message::Quit),
                    Ok(None) => {}
                    Err(error) => eprintln!("Could not show the grip menu: {error}"),
                }
                true
            }
            _ => false,
        });
    }

    fn add_prompt_button(&mut self, index: usize, prompt: &Prompt, orientation: Orientation) {
        let extent = button_width(&prompt.title);
        let inner_thickness = STRIP_HEIGHT - EDGE * 2;
        let (width, height) = match orientation {
            Orientation::Horizontal => (extent, inner_thickness),
            Orientation::Vertical => (inner_thickness, extent),
        };
        let mut button = Button::new(0, 0, width, height, prompt.title.as_str());
        style_button(&mut button);
        if orientation == Orientation::Vertical {
            draw_vertical_button(&mut button, prompt.title.clone());
        }
        button.set_tooltip("Click to insert · Right-click to edit");

        let sender = self.sender;
        button.set_callback(move |_| sender.send(Message::Insert(index)));

        let sender = self.sender;
        let mut right_button_down = false;
        button.super_handle_first(false);
        button.handle(move |_, event| match event {
            Event::Push if app::event_button() == 3 => {
                right_button_down = true;
                true
            }
            Event::Drag if right_button_down => true,
            Event::Released if right_button_down => {
                right_button_down = false;
                sender.send(Message::Edit(index));
                true
            }
            _ => false,
        });
    }
}

pub struct Editor {
    window: DoubleWindow,
    title: Input,
    anchor: DoubleWindow,
    text: MultilineInput,
    current_index: std::rc::Rc<std::cell::Cell<Option<usize>>>,
}

impl Editor {
    pub fn new(sender: app::Sender<Message>, anchor: DoubleWindow) -> Self {
        const WIDTH: i32 = 520;
        const HEIGHT: i32 = 390;
        const MARGIN: i32 = 16;
        const LABEL_WIDTH: i32 = 74;

        let mut window = DoubleWindow::new(0, 0, WIDTH, HEIGHT, "Edit Promplet");
        window.set_color(Color::from_rgb(192, 192, 192));

        let mut title_label = Frame::new(MARGIN, 18, LABEL_WIDTH, 26, "Title:");
        title_label.set_align(Align::Inside | Align::Right);

        let mut title = Input::new(
            MARGIN + LABEL_WIDTH + 8,
            18,
            WIDTH - MARGIN * 2 - LABEL_WIDTH - 8,
            26,
            "",
        );
        title.set_text_size(14);

        let mut prompt_label = Frame::new(MARGIN, 58, LABEL_WIDTH, 24, "Prompt:");
        prompt_label.set_align(Align::Inside | Align::Right);

        let mut text = MultilineInput::new(
            MARGIN + LABEL_WIDTH + 8,
            58,
            WIDTH - MARGIN * 2 - LABEL_WIDTH - 8,
            244,
            "",
        );
        text.set_text_size(14);
        text.set_wrap(true);
        text.set_tab_nav(false);

        let button_y = HEIGHT - MARGIN - 30;
        let mut delete = Button::new(MARGIN, button_y, 72, 30, "Delete");
        let mut duplicate = Button::new(MARGIN + 80, button_y, 86, 30, "Duplicate");
        let mut add_after = Button::new(MARGIN + 174, button_y, 86, 30, "Add after");
        let mut cancel = Button::new(WIDTH - MARGIN - 174, button_y, 76, 30, "Cancel");
        let mut save = Button::new(WIDTH - MARGIN - 90, button_y, 90, 30, "Save");
        for button in [
            &mut delete,
            &mut duplicate,
            &mut add_after,
            &mut cancel,
            &mut save,
        ] {
            style_button(button);
        }

        window.end();
        window.make_resizable(false);

        let current_index = std::rc::Rc::new(std::cell::Cell::new(None));

        let index = current_index.clone();
        let title_for_save = title.clone();
        let text_for_save = text.clone();
        save.set_callback(move |_| {
            if let Some(index) = index.get() {
                sender.send(Message::Save {
                    index,
                    title: title_for_save.value(),
                    text: text_for_save.value(),
                });
            }
        });

        let index = current_index.clone();
        duplicate.set_callback(move |_| {
            if let Some(index) = index.get() {
                sender.send(Message::Duplicate(index));
            }
        });

        let index = current_index.clone();
        add_after.set_callback(move |_| {
            if let Some(index) = index.get() {
                sender.send(Message::AddAfter(index));
            }
        });

        let index = current_index.clone();
        delete.set_callback(move |_| {
            if let Some(index) = index.get() {
                sender.send(Message::Delete(index));
            }
        });

        cancel.set_callback(move |_| sender.send(Message::CloseEditor));
        window.set_callback(move |_| sender.send(Message::CloseEditor));

        Self {
            window,
            anchor,
            title,
            text,
            current_index,
        }
    }

    pub fn open(&mut self, index: usize, prompt: &Prompt, orientation: Orientation) {
        self.current_index.set(Some(index));
        self.title.set_value(&prompt.title);
        self.text.set_value(&prompt.text);
        self.window.set_label(&format!("Edit “{}”", prompt.title));

        if !self.window.shown() {
            self.window.show();
        }
        self.window.set_on_top();
        self.reposition(orientation);
        if let Err(error) = platform::activate_window(&self.window) {
            eprintln!("Could not activate editor window: {error}");
        }

        self.window.take_focus().ok();
        self.title.take_focus().ok();
        self.title
            .set_position(self.title.value().len() as i32)
            .ok();
    }

    pub fn reposition(&mut self, orientation: Orientation) {
        if !self.window.shown() {
            return;
        }

        if let Err(error) =
            platform::position_editor(&self.window, &self.anchor, EDITOR_GAP, orientation)
        {
            eprintln!("Could not position editor window: {error}");
        }
    }
    pub fn hide(&mut self) {
        self.current_index.set(None);
        self.window.hide();
    }
}

fn style_button(button: &mut Button) {
    button.set_frame(FrameType::UpBox);
    button.set_down_frame(FrameType::DownBox);
    button.set_color(Color::from_rgb(204, 204, 204));
    button.set_selection_color(Color::from_rgb(160, 160, 160));
    button.set_label_font(Font::Helvetica);
    button.set_label_size(13);
}

fn draw_vertical_button(button: &mut Button, title: String) {
    button.draw(move |button| {
        let (frame, color) = if button.value() {
            (button.down_frame(), button.selection_color())
        } else {
            (button.frame(), button.color())
        };
        draw::draw_box(frame, button.x(), button.y(), button.w(), button.h(), color);

        draw::push_clip(button.x(), button.y(), button.w(), button.h());
        draw::set_font(button.label_font(), button.label_size());
        draw::set_draw_color(button.label_color());
        let (text_width, _) = draw::measure(&title, false);
        let baseline_x = button.x() + (button.w() - draw::height()) / 2 + draw::descent();
        let baseline_y = button.y() + (button.h() - text_width) / 2;
        draw::draw_text_angled(-90, &title, baseline_x, baseline_y);
        draw::pop_clip();
    });
}

fn button_width(title: &str) -> i32 {
    let estimated_text_width = title.chars().count() as i32 * 7;
    (estimated_text_width + BUTTON_HORIZONTAL_PADDING).clamp(MIN_BUTTON_WIDTH, MAX_BUTTON_WIDTH)
}

fn strip_dimensions(config: &Config) -> (i32, i32) {
    let prompt_extent: i32 = config
        .prompts
        .iter()
        .map(|prompt| button_width(&prompt.title))
        .sum();
    let child_count = config.prompts.len() as i32 + 1;
    let length = EDGE * 2 + GRIP_WIDTH + prompt_extent + SPACING * child_count.saturating_sub(1);

    match config.orientation {
        Orientation::Horizontal => (length, STRIP_HEIGHT),
        Orientation::Vertical => (STRIP_HEIGHT, length),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_dimensions_are_a_clockwise_quarter_turn() {
        let horizontal = Config::default();
        let mut vertical = horizontal.clone();
        vertical.orientation = Orientation::Vertical;

        let horizontal_size = strip_dimensions(&horizontal);
        let vertical_size = strip_dimensions(&vertical);

        assert_eq!(vertical_size, (horizontal_size.1, horizontal_size.0));
    }
}
