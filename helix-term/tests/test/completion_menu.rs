use super::*;

use std::{cell::RefCell, rc::Rc};

use helix_term::{
    compositor::{Component, Context, Event, EventResult},
    job::Jobs,
    ui::{
        menu::{Cell, Item, Menu, Row},
        PromptEvent,
    },
};
use helix_view::{graphics::Rect, input::KeyEvent};
use tui::buffer::Buffer;

#[derive(Clone)]
struct Suggestion(usize);

impl Item for Suggestion {
    type Data = ();

    fn format(&self, _: &()) -> Row<'_> {
        Row::new([Cell::from(format!("item{}", self.0))])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn completion_menu_numbered_shortcuts() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    let mut jobs = Jobs::new();
    let mut cx = Context {
        editor: &mut app.editor,
        jobs: &mut jobs,
        scroll: None,
    };
    let accepted = Rc::new(RefCell::new(Vec::new()));
    let capture = accepted.clone();
    let mut menu = Menu::new(
        (0..20).map(Suggestion).collect(),
        (),
        move |_, item, event| {
            if event == PromptEvent::Validate {
                capture.borrow_mut().push(item.unwrap().0);
            }
        },
    )
    .numbered_shortcuts();
    let event = |key: &str| Event::Key(key.parse::<KeyEvent>().unwrap());
    let render = |menu: &mut Menu<Suggestion>, cx: &mut Context, height| {
        let (width, height) = menu.required_size((40, height)).unwrap();
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        menu.render(area, &mut buffer, cx);
        buffer
    };

    let buffer = render(&mut menu, &mut cx, 10);
    for row in 0..10 {
        assert_eq!(
            buffer[(1, row)].symbol.as_str(),
            ((row + 1) % 10).to_string()
        );
        let key = format!("C-{}", (row + 1) % 10);
        assert!(matches!(
            menu.handle_event(&event(&key), &mut cx),
            EventResult::Consumed(Some(_))
        ));
        assert_eq!(accepted.borrow().last(), Some(&(row as usize)));
    }

    // After scrolling, numbers refer to the visible page, not absolute rank.
    menu.move_down();
    let buffer = render(&mut menu, &mut cx, 10);
    assert_eq!(buffer[(1, 0)].symbol.as_str(), "1");
    assert!(matches!(
        menu.handle_event(&event("C-1"), &mut cx),
        EventResult::Consumed(Some(_))
    ));
    assert_eq!(accepted.borrow().last(), Some(&1));

    // Only five rows fit: the sixth shortcut must neither accept nor fall through.
    render(&mut menu, &mut cx, 5);
    let before = accepted.borrow().len();
    assert!(matches!(
        menu.handle_event(&event("C-6"), &mut cx),
        EventResult::Consumed(None)
    ));
    assert_eq!(accepted.borrow().len(), before);

    // Reordered/filtered matches are the source of truth, not the original options.
    let (matches, _) = menu.update_options();
    *matches = vec![(17, 0), (3, 0)];
    menu.reset_cursor();
    render(&mut menu, &mut cx, 10);
    menu.handle_event(&event("C-2"), &mut cx);
    assert_eq!(accepted.borrow().last(), Some(&3));
    let before = accepted.borrow().len();
    assert!(matches!(
        menu.handle_event(&event("C-9"), &mut cx),
        EventResult::Consumed(None)
    ));
    assert_eq!(accepted.borrow().len(), before);
    assert!(matches!(
        menu.handle_event(&event("2"), &mut cx),
        EventResult::Ignored(None)
    ));

    // Existing navigation and Enter still use the same validation path.
    menu.reset_cursor();
    menu.handle_event(&event("C-n"), &mut cx);
    menu.handle_event(&event("ret"), &mut cx);
    assert_eq!(accepted.borrow().last(), Some(&17));
    assert!(matches!(
        menu.handle_event(&event("C-c"), &mut cx),
        EventResult::Consumed(Some(_))
    ));

    // Other menus neither show numbers nor intercept Ctrl+digits.
    let mut ordinary = Menu::new(vec![Suggestion(0)], (), |_, _, _| {});
    let buffer = render(&mut ordinary, &mut cx, 10);
    assert_eq!(buffer[(1, 0)].symbol.as_str(), "i");
    assert!(matches!(
        ordinary.handle_event(&event("C-1"), &mut cx),
        EventResult::Ignored(None)
    ));

    menu.clear();
    render(&mut menu, &mut cx, 10);
    assert!(matches!(
        menu.handle_event(&event("C-1"), &mut cx),
        EventResult::Consumed(None)
    ));
    Ok(())
}
