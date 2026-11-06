//! Go to a line in the active editor, following Zed's line/column picker.

use crate::ross::{OpenColumn, OpenLocation};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, WindowExt, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub(crate) fn open(
    current_line: usize,
    current_column: usize,
    line_count: usize,
    on_go: Box<dyn Fn(OpenLocation, &mut Window, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let picker = cx.new(|cx| {
        GoToLine::new(current_line, current_column, line_count, on_go, window, cx)
    });
    let focus = picker.read(cx).query.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Go to Line")
            .width(px(360.))
            .close_button(false)
            .on_ok({
                let picker = picker.clone();
                move |_, window, cx| {
                    picker.update(cx, |picker, cx| picker.confirm(window, cx));
                    false
                }
            })
            .child(picker.clone())
    });
    window.focus(&focus, cx);
}

struct GoToLine {
    query: Entity<InputState>,
    current_line: usize,
    current_column: usize,
    line_count: usize,
    error: Option<&'static str>,
    on_go: Box<dyn Fn(OpenLocation, &mut Window, &mut App)>,
    _escape_interceptor: Subscription,
}

impl GoToLine {
    fn new(
        current_line: usize,
        current_column: usize,
        line_count: usize,
        on_go: Box<dyn Fn(OpenLocation, &mut Window, &mut App)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(format!("{current_line}:{current_column}"))
        });
        let focus = query.read(cx).focus_handle(cx);
        let escape_interceptor =
            cx.intercept_keystrokes(move |event, window, cx| {
                if focus.is_focused(window)
                    && event.keystroke.key.as_str() == "escape"
                {
                    window.close_dialog(cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            });
        cx.subscribe(&query, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.error = None;
                cx.notify();
            }
        })
        .detach();
        Self {
            query,
            current_line,
            current_column,
            line_count,
            error: None,
            on_go,
            _escape_interceptor: escape_interceptor,
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.query.read(cx).value();
        let Some((line, column)) = parse_target(
            &value,
            self.current_line,
            self.current_column,
            self.line_count,
        ) else {
            self.error = Some("Enter a line, line:column, +N, or -N");
            cx.notify();
            return;
        };
        window.close_dialog(cx);
        (self.on_go)(
            OpenLocation {
                line,
                column: OpenColumn::Character(column),
                end_line: line,
                end_column: OpenColumn::Character(column),
            },
            window,
            cx,
        );
    }
}

fn parse_target(
    query: &str,
    current_line: usize,
    current_column: usize,
    line_count: usize,
) -> Option<(usize, usize)> {
    let query = query.trim();
    if query.is_empty() {
        return Some((
            current_line.saturating_sub(1),
            current_column.saturating_sub(1),
        ));
    }
    let (line, column) = query.split_once(':').unwrap_or((query, ""));
    let column = if column.is_empty() {
        1
    } else {
        column.trim().parse::<usize>().ok()?.max(1)
    };
    let line = if let Some(offset) = line.strip_prefix('+') {
        current_line.saturating_add(offset.parse::<usize>().ok()?)
    } else if let Some(offset) = line.strip_prefix('-') {
        current_line
            .saturating_sub(offset.parse::<usize>().ok()?)
            .max(1)
    } else {
        line.parse::<usize>().ok()?
    };
    if line == 0 {
        return None;
    }
    Some((line.min(line_count.max(1)) - 1, column - 1))
}

impl Render for GoToLine {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focus = self.query.read(cx).focus_handle(cx);
        let hint = self.error.map(str::to_string).unwrap_or_else(|| {
            format!(
                "Line {} of {} · +N/-N moves from here · Enter to go",
                self.current_line, self.line_count
            )
        });
        v_flex()
            .w_full()
            .track_focus(&focus)
            .gap_2()
            .child(Input::new(&self.query).aria_label("Line and column"))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(if self.error.is_some() {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(hint),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{GoToLine, parse_target};
    use crate::ross::{OpenColumn, OpenLocation};
    use gpui_kit::component::{Root, WindowExt};
    use gpui_kit::{
        AppContext, Context, Focusable, IntoElement, ParentElement, Render,
        TestAppContext, Window, div,
    };
    use std::sync::{Arc, Mutex};

    struct TestRoot;

    impl Render for TestRoot {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            div().children(Root::render_dialog_layer(window, cx))
        }
    }

    #[test]
    fn line_and_column_queries_are_one_based_and_clamped() {
        assert_eq!(parse_target("42:7", 12, 3, 100), Some((41, 6)));
        assert_eq!(parse_target("+5", 12, 3, 100), Some((16, 0)));
        assert_eq!(parse_target("-20", 12, 3, 100), Some((0, 0)));
        assert_eq!(parse_target("999", 12, 3, 100), Some((99, 0)));
        assert_eq!(parse_target("", 12, 3, 100), Some((11, 2)));
        assert_eq!(parse_target("0", 12, 3, 100), None);
        assert_eq!(parse_target("a:3", 12, 3, 100), None);
    }

    #[gpui_kit::test]
    fn enter_navigates_to_line_and_column(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let root = cx.new(|_| TestRoot);
            Root::new(root, window, cx)
        });
        let opened = Arc::new(Mutex::new(None));
        let opened_for_callback = opened.clone();
        let picker = cx.update(move |window, cx| {
            let picker = cx.new(|cx| {
                GoToLine::new(
                    12,
                    3,
                    100,
                    Box::new(move |location, _, _| {
                        *opened_for_callback.lock().expect("location mutex") =
                            Some(location);
                    }),
                    window,
                    cx,
                )
            });
            let focus = picker.read(cx).query.read(cx).focus_handle(cx);
            window.open_dialog(cx, {
                let picker = picker.clone();
                move |dialog, _, _| {
                    dialog
                        .title("Go to Line")
                        .on_ok({
                            let picker = picker.clone();
                            move |_, window, cx| {
                                picker.update(cx, |picker, cx| {
                                    picker.confirm(window, cx)
                                });
                                false
                            }
                        })
                        .child(picker.clone())
                }
            });
            window.focus(&focus, cx);
            picker
        });
        cx.update(|window, cx| {
            picker.update(cx, |picker, cx| {
                picker.query.update(cx, |query, cx| {
                    query.set_value("42:7", window, cx);
                });
            });
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("enter");

        assert_eq!(
            *opened.lock().expect("location mutex"),
            Some(OpenLocation {
                line: 41,
                column: OpenColumn::Character(6),
                end_line: 41,
                end_column: OpenColumn::Character(6),
            })
        );
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}
