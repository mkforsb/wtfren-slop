//! Keeps track of which file the user appears to be looking at.
//!
//! Wraps the page's scrollable. On every mouse event and redraw it asks the page where
//! the file sections are, and publishes a message only when the answer changes. Redraws
//! cover everything else that moves sections around: scrolling, new content, resizing.

use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Operation, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector};

type Renderer = iced::Renderer;

/// In order: the section under the mouse, the first section whose header is visible,
/// the section filling the view. `sections` are in page order and, like `cursor`, in
/// the coordinates of `view`.
pub fn looked_at(cursor: Option<Point>, view: Rectangle, sections: &[(usize, Rectangle)]) -> Option<usize> {
    let hovered = cursor.filter(|p| view.contains(*p)).and_then(|p| sections.iter().find(|(_, s)| s.contains(p)));
    let header = || sections.iter().find(|(_, s)| s.y >= view.y && s.y < view.y + view.height);
    let filling = || sections.iter().find(|(_, s)| s.y < view.y && s.y + s.height > view.y);
    hovered.or_else(header).or_else(filling).map(|(i, _)| *i)
}

pub struct Tracker<'a, Message> {
    content: Element<'a, Message>,
    scrollable: widget::Id,
    /// File index and container id of each section, in page order.
    sections: Vec<(usize, widget::Id)>,
    on_change: Box<dyn Fn(Option<usize>) -> Message + 'a>,
}

impl<'a, Message> Tracker<'a, Message> {
    pub fn new(
        content: impl Into<Element<'a, Message>>,
        scrollable: impl Into<widget::Id>,
        sections: Vec<(usize, widget::Id)>,
        on_change: impl Fn(Option<usize>) -> Message + 'a,
    ) -> Self {
        Self { content: content.into(), scrollable: scrollable.into(), sections, on_change: Box::new(on_change) }
    }
}

#[derive(Default)]
struct State {
    current: Option<usize>,
}

/// Collects the view and the sections' bounds in page coordinates.
struct Measure<'a> {
    scrollable: &'a widget::Id,
    ids: &'a [(usize, widget::Id)],
    view: Option<(Rectangle, Vector)>,
    found: Vec<(usize, Rectangle)>,
}

impl Operation for Measure<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
        if let Some(id) = id
            && let Some((i, _)) = self.ids.iter().find(|(_, s)| s == id)
        {
            self.found.push((*i, bounds));
        }
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn widget::operation::Scrollable,
    ) {
        if id == Some(self.scrollable) {
            self.view = Some((bounds, translation));
        }
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Tracker<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(&mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport);

        if !matches!(event, Event::Mouse(_) | Event::Window(iced::window::Event::RedrawRequested(_))) {
            return;
        }
        let mut measure = Measure { scrollable: &self.scrollable, ids: &self.sections, view: None, found: Vec::new() };
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, &mut measure);
        let Some((view, translation)) = measure.view else { return };
        let on_screen: Vec<_> = measure.found.iter().map(|(i, b)| (*i, *b - translation)).collect();
        let current = looked_at(cursor.position(), view, &on_screen);
        let state = tree.state.downcast_mut::<State>();
        if state.current != current {
            state.current = current;
            shell.publish((self.on_change)(current));
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

impl<'a, Message: 'a> From<Tracker<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(t: Tracker<'a, Message>) -> Self {
        Element::new(t)
    }
}

#[cfg(test)]
mod tests {
    use super::looked_at;
    use iced::{Point, Rectangle, Size};

    fn rect(y: f32, height: f32) -> Rectangle {
        Rectangle::new(Point::new(0.0, y), Size::new(100.0, height))
    }

    #[test]
    fn hovered_section_comes_first() {
        let sections = [(0, rect(10.0, 100.0)), (1, rect(120.0, 100.0))];
        assert_eq!(looked_at(Some(Point::new(5.0, 150.0)), rect(0.0, 300.0), &sections), Some(1));
    }

    #[test]
    fn a_cursor_outside_the_view_is_ignored() {
        let sections = [(0, rect(-500.0, 600.0)), (1, rect(120.0, 100.0))];
        let cursor = Point::new(5.0, -100.0);
        assert_eq!(looked_at(Some(cursor), rect(0.0, 300.0), &sections), Some(1));
    }

    #[test]
    fn then_the_first_visible_header() {
        let sections = [(0, rect(-50.0, 100.0)), (1, rect(60.0, 100.0)), (2, rect(170.0, 100.0))];
        assert_eq!(looked_at(None, rect(0.0, 300.0), &sections), Some(1));
    }

    #[test]
    fn then_the_section_filling_the_view() {
        let sections = [(0, rect(-50.0, 1000.0)), (1, rect(960.0, 100.0))];
        assert_eq!(looked_at(None, rect(0.0, 300.0), &sections), Some(0));
    }

    #[test]
    fn nothing_on_an_empty_page() {
        assert_eq!(looked_at(None, rect(0.0, 300.0), &[]), None);
    }
}
