//! JavaScript Set callback identity and iteration, including removal and re-addition during delivery.
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
pub(super) struct Listeners<T: ?Sized> {
    next: Cell<u64>,
    rows: RefCell<Vec<(u64, Rc<T>)>>,
}
impl<T: ?Sized> Default for Listeners<T> {
    fn default() -> Self {
        Self { next: Cell::new(0), rows: RefCell::new(vec![]) }
    }
}
impl<T: ?Sized> Listeners<T> {
    pub fn add(&self, listener: Rc<T>) {
        if self.rows.borrow().iter().any(|(_, v)| Rc::ptr_eq(v, &listener)) {
            return;
        }
        let id = self.next.get() + 1;
        self.next.set(id);
        self.rows.borrow_mut().push((id, listener));
    }
    pub fn remove(&self, listener: &Rc<T>) {
        self.rows.borrow_mut().retain(|(_, v)| !Rc::ptr_eq(v, listener));
    }
    pub fn each(&self, mut invoke: impl FnMut(&Rc<T>)) {
        let mut cursor = 0;
        loop {
            let next = self.rows.borrow().iter().find(|(id, _)| *id > cursor).cloned();
            let Some((id, listener)) = next else { break };
            cursor = id;
            invoke(&listener);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deleting_prior_and_current_listeners_does_not_skip_next_and_new_callbacks_are_visited() {
        let set: Rc<Listeners<dyn Fn()>> = Rc::new(Listeners::default());
        let calls = Rc::new(RefCell::new(vec![]));
        let logged = calls.clone();
        let first: Rc<dyn Fn()> = Rc::new(move || logged.borrow_mut().push(1));
        set.add(first.clone());
        let holder: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let own = holder.clone();
        let callbacks = set.clone();
        let logged = calls.clone();
        let second: Rc<dyn Fn()> = Rc::new(move || {
            logged.borrow_mut().push(2);
            callbacks.remove(&first);
            callbacks.remove(own.borrow().as_ref().unwrap());
            let logged = logged.clone();
            callbacks.add(Rc::new(move || logged.borrow_mut().push(4)));
        });
        *holder.borrow_mut() = Some(second.clone());
        set.add(second);
        let logged = calls.clone();
        set.add(Rc::new(move || logged.borrow_mut().push(3)));
        set.each(|listener| listener());
        assert_eq!(*calls.borrow(), vec![1, 2, 3, 4]);
        holder.take();
    }
}
