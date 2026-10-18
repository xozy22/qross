//! Warteschlange fester Größe für genau einen schreibenden und einen lesenden Thread, ohne Sperren
//! und ohne beim Schreiben Speicher anzulegen. Gebraucht vom Hook-Thread unter Windows: Er darf nie
//! warten, sonst entfernt Windows seine Hooks (siehe `win::capture`).

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Plätze minus 1 (Zweierpotenz), damit die Zähler beliebig überlaufen dürfen
    mask: usize,
    /// Nächster zu lesender Platz; schreibt nur der Leser
    head: AtomicUsize,
    /// Nächster zu schreibender Platz; schreibt nur der Schreiber
    tail: AtomicUsize,
    /// Der Schreiber ist weg
    closed: AtomicBool,
}

// Jeder Platz gehört zu jeder Zeit genau einer Seite: von `head` bis vor `tail` dem Leser, sonst dem
// Schreiber. Übergeben wird er nur über `tail` (Release/Acquire) bzw. `head`.
unsafe impl<T: Send> Sync for Ring<T> {}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        let (mut i, tail) = (*self.head.get_mut(), *self.tail.get_mut());
        while i != tail {
            // SAFETY: Plätze von `head` bis vor `tail` sind beschrieben und noch nicht gelesen.
            unsafe { self.slots[i & self.mask].get_mut().assume_init_drop() };
            i = i.wrapping_add(1);
        }
    }
}

/// Schreibende Seite (genau eine)
pub(crate) struct Producer<T> {
    ring: Arc<Ring<T>>,
}

/// Lesende Seite (genau eine)
pub(crate) struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

/// Neue Warteschlange mit mindestens `capacity` Plätzen
pub(crate) fn queue<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let len = capacity.max(1).next_power_of_two();
    let ring = Arc::new(Ring {
        slots: (0..len).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect(),
        mask: len - 1,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
    });
    (Producer { ring: ring.clone() }, Consumer { ring })
}

impl<T> Producer<T> {
    /// Hängt `value` an; ist die Schlange voll, kommt es zurück.
    pub(crate) fn push(&mut self, value: T) -> Result<(), T> {
        let ring = &*self.ring;
        let tail = ring.tail.load(Ordering::Relaxed);
        let head = ring.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) > ring.mask {
            return Err(value);
        }
        // SAFETY: Der Platz bei `tail` gehört dem Schreiber (siehe `Ring`), und es gibt nur einen.
        unsafe { (*ring.slots[tail & ring.mask].get()).write(value) };
        ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.ring.closed.store(true, Ordering::Release);
    }
}

impl<T> Consumer<T> {
    /// Das älteste Element, falls eines wartet
    pub(crate) fn pop(&mut self) -> Option<T> {
        let ring = &*self.ring;
        let head = ring.head.load(Ordering::Relaxed);
        let tail = ring.tail.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        // SAFETY: Der Platz liegt vor `tail`, ist also beschrieben, und gehört dem (einzigen) Leser.
        let value = unsafe { (*ring.slots[head & ring.mask].get()).assume_init_read() };
        ring.head.store(head.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Der Schreiber ist weg. Was er vorher anhängte, liefert `pop` danach noch.
    pub(crate) fn is_closed(&self) -> bool {
        self.ring.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_and_refuses_when_full() {
        let (mut tx, mut rx) = queue(3);
        for i in 0..4 {
            tx.push(i).unwrap();
        }
        assert_eq!(tx.push(4), Err(4), "auf 4 Plätze aufgerundet");
        assert_eq!(rx.pop(), Some(0));
        tx.push(4).unwrap();
        assert_eq!((1..=4).map(|_| rx.pop().unwrap()).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(rx.pop(), None);
        // Über das Ende der Plätze hinweg
        for round in 0..10 {
            tx.push(round).unwrap();
            assert_eq!(rx.pop(), Some(round));
        }
    }

    #[test]
    fn reports_the_writer_gone_after_its_last_values() {
        let (mut tx, mut rx) = queue(4);
        tx.push("a").unwrap();
        assert!(!rx.is_closed());
        drop(tx);
        assert!(rx.is_closed());
        assert_eq!(rx.pop(), Some("a"));
        assert_eq!(rx.pop(), None);
    }

    #[test]
    fn drops_what_nobody_read() {
        let value = Arc::new(());
        let (mut tx, mut rx) = queue(8);
        for _ in 0..5 {
            tx.push(value.clone()).unwrap();
        }
        drop(rx.pop());
        assert_eq!(Arc::strong_count(&value), 5);
        drop((tx, rx));
        assert_eq!(Arc::strong_count(&value), 1);
    }

    #[test]
    fn hands_values_between_threads_in_order() {
        const N: u64 = 200_000;
        let (mut tx, mut rx) = queue::<u64>(16);
        let writer = std::thread::spawn(move || {
            for i in 0..N {
                let mut value = i;
                while let Err(back) = tx.push(value) {
                    value = back;
                    std::thread::yield_now();
                }
            }
        });
        let mut next = 0;
        while next < N {
            match rx.pop() {
                Some(value) => {
                    assert_eq!(value, next);
                    next += 1;
                }
                None => std::thread::yield_now(),
            }
        }
        writer.join().unwrap();
        assert!(rx.is_closed());
        assert_eq!(rx.pop(), None);
    }
}
