//! What opens over the conversation (a list to choose from, a sign-in, a side answer, a job's details, the help), as a
//! stack. Only the top one takes input; those under it wait. They draw bottom first, so the top one is over the rest.
//! An overlay handles its input, draws itself and gives the box's hint. What it does to the app goes through the app
//! it's handed, and what becomes of it (stay, close, or close and hand the input on) it says when it hands back.
use std::{any::Any, cell::RefCell, rc::Rc};

use super::{keys::InputEvent, render::Cursor, screen::Screen};

/// What an overlay did with an input event.
#[derive(Debug, PartialEq)]
pub enum Handled {
    /// It took it, and stays open.
    Kept,
    /// It closes: esc, or it's done.
    Close,
    /// It closes, and the event goes on as if it hadn't been open (a `/` typed over a list starts a command).
    Pass(InputEvent),
}

/// Where overlays draw: above the input box, whose top row is `box_top`, in the `left` columns the conversation has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub box_top: i32,
    pub left: i32,
}

/// One overlay, for an app of type `App`.
///
/// `handle` and `draw` run while the stack holds this overlay mutably borrowed. So neither may reach the stack's top
/// one again before it returns (in Kumi's app: close_panel, top_as, top_is, answers_open, signing_in, same_picker,
/// put_in_box): an overlay that's done says so with `Handled::Close` or `Handled::Pass`, and work that needs it later
/// runs in a task, after it returns.
pub trait Overlay<App>: Any {
    /// An input event while it's the top one: a key, typed text or a paste. Esc closes it.
    fn handle(&mut self, app: &App, event: InputEvent) -> Handled;
    /// Draws itself in `area`, and says where the cursor goes, when it shows one.
    fn draw(&mut self, app: &App, screen: &mut Screen, area: Area) -> Option<Cursor>;
    /// The box's hint while it's the top one.
    fn hint(&self, app: &App) -> String;
    /// Whether its hint stays when the box has something to say for a moment (a toast): a countdown to Kumi's pick
    /// can't wait for it to go.
    fn hint_stays(&self, _app: &App) -> bool {
        false
    }
    /// It's closed: it lets go of what it holds (a sign-in under way, a key half typed).
    fn closed(&mut self, _app: &App) {}
    /// It's in the middle of something talking mustn't cut short (a sign-in, an answer being written): ctrl+t leaves it
    /// open.
    fn busy(&self, _app: &App) -> bool {
        false
    }
    /// The mouse wheel while it's the top one: `rows` down, or up when negative. True when it scrolled; otherwise what's
    /// under it scrolls, as without it.
    fn scroll(&mut self, _app: &App, _rows: i32) -> bool {
        false
    }
}

impl<App: 'static> dyn Overlay<App> {
    /// The overlay as what it is, when it's a `T`: for the code that opened it.
    pub fn downcast_ref<T: Overlay<App>>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
    pub fn downcast_mut<T: Overlay<App>>(&mut self) -> Option<&mut T> {
        (self as &mut dyn Any).downcast_mut()
    }
}

/// An overlay as the stack holds it. Whoever opened it may keep it as its own type: the stack's is the same one.
pub type Shared<App> = Rc<RefCell<dyn Overlay<App>>>;

/// The overlays open, bottom first.
pub struct Overlays<App> {
    open: Vec<Shared<App>>,
}
impl<App> Default for Overlays<App> {
    fn default() -> Self {
        Self { open: vec![] }
    }
}
impl<App> Overlays<App> {
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
    pub fn top(&self) -> Option<Shared<App>> {
        self.open.last().cloned()
    }
    /// Opens `overlay` in place of the top one, or as the first: one panel opening over another has always replaced it.
    /// The one replaced isn't told it closed.
    pub fn open(&mut self, overlay: Shared<App>) {
        self.open.pop();
        self.open.push(overlay);
    }
    /// Opens `overlay` over the others, which wait under it.
    pub fn push(&mut self, overlay: Shared<App>) {
        self.open.push(overlay);
    }
    /// Takes `overlay` out, wherever it is: false when it isn't open.
    pub fn remove<T: ?Sized>(&mut self, overlay: &Rc<RefCell<T>>) -> bool {
        let before = self.open.len();
        self.open.retain(|open| !same(open, overlay));
        self.open.len() < before
    }
    /// Whether `overlay` is open, under others or not.
    pub fn is_open<T: ?Sized>(&self, overlay: &Rc<RefCell<T>>) -> bool {
        self.open.iter().any(|open| same(open, overlay))
    }
    /// Whether `overlay` is the top one.
    pub fn is_top<T: ?Sized>(&self, overlay: &Rc<RefCell<T>>) -> bool {
        self.open.last().is_some_and(|open| same(open, overlay))
    }
    /// The overlays open, bottom first, as they draw.
    pub fn all(&self) -> Vec<Shared<App>> {
        self.open.clone()
    }
    /// Takes every one out (Kumi closing), bottom first.
    pub fn drain(&mut self) -> Vec<Shared<App>> {
        std::mem::take(&mut self.open)
    }
}

/// The same overlay: one allocation, whatever type each holder sees it as.
fn same<T: ?Sized, U: ?Sized>(a: &Rc<RefCell<T>>, b: &Rc<RefCell<U>>) -> bool {
    std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app that keeps what its overlays were told.
    #[derive(Default)]
    struct App {
        said: RefCell<Vec<String>>,
    }
    struct Note(&'static str);
    impl Overlay<App> for Note {
        fn handle(&mut self, app: &App, event: InputEvent) -> Handled {
            app.said.borrow_mut().push(format!("{} took {event:?}", self.0));
            Handled::Kept
        }
        fn draw(&mut self, app: &App, _: &mut Screen, _: Area) -> Option<Cursor> {
            app.said.borrow_mut().push(format!("{} drawn", self.0));
            None
        }
        fn hint(&self, _: &App) -> String {
            format!("{}'s hint", self.0)
        }
    }
    fn note(name: &'static str) -> Rc<RefCell<Note>> {
        Rc::new(RefCell::new(Note(name)))
    }
    fn names(overlays: &Overlays<App>) -> Vec<&'static str> {
        overlays.all().iter().map(|open| open.borrow().downcast_ref::<Note>().unwrap().0).collect()
    }

    #[test]
    fn opening_replaces_the_top_one_and_pushing_stacks_over_it() {
        let mut overlays = Overlays::<App>::default();
        assert!(overlays.is_empty() && overlays.top().is_none());
        let (list, sign_in, help) = (note("list"), note("sign-in"), note("help"));
        overlays.open(list.clone());
        overlays.open(sign_in.clone());
        assert_eq!(names(&overlays), ["sign-in"], "a panel opening over another replaces it");
        overlays.push(help.clone());
        assert_eq!(names(&overlays), ["sign-in", "help"]);
        assert!(overlays.is_top(&help) && !overlays.is_top(&sign_in) && overlays.is_open(&sign_in) && !overlays.is_open(&list));
        overlays.open(list.clone());
        assert_eq!(names(&overlays), ["sign-in", "list"], "only the top one is replaced");
    }

    #[test]
    fn an_overlay_comes_out_by_itself_wherever_it_is() {
        let mut overlays = Overlays::<App>::default();
        let (under, over) = (note("under"), note("over"));
        overlays.push(under.clone());
        overlays.push(over.clone());
        assert!(overlays.remove(&under));
        assert!(!overlays.remove(&under), "it's out already");
        assert_eq!(names(&overlays), ["over"]);
        // The stack's own handle is the same overlay as its opener's.
        let top: Shared<App> = overlays.top().unwrap();
        assert!(overlays.is_top(&top) && overlays.remove(&top) && overlays.is_empty());
        overlays.push(under);
        overlays.push(over);
        assert_eq!(overlays.drain().len(), 2);
        assert!(overlays.is_empty());
    }

    #[test]
    fn the_top_one_takes_input_and_all_draw_bottom_first() {
        let app = App::default();
        let mut overlays = Overlays::<App>::default();
        overlays.push(note("list"));
        overlays.push(note("help"));
        let top = overlays.top().unwrap();
        assert_eq!(top.borrow_mut().handle(&app, InputEvent::text("x")), Handled::Kept);
        assert_eq!(top.borrow().hint(&app), "help's hint");
        let mut screen = Screen::new(10, 4);
        for open in overlays.all() {
            open.borrow_mut().draw(&app, &mut screen, Area { box_top: 3, left: 10 });
        }
        assert_eq!(*app.said.borrow(), ["help took Text { text: \"x\" }", "list drawn", "help drawn"]);
    }
}
