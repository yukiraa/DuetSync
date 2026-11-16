//! Bounded, preallocated single-producer / single-consumer ring buffer.
//! Lock-free and allocation-free on both sides after construction; safe for audio threads.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner<T> {
    buf: Box<[UnsafeCell<T>]>,
    head: AtomicUsize, // next write (producer)
    tail: AtomicUsize, // next read (consumer)
    dropped: AtomicU64,
}

unsafe impl<T: Send> Sync for Inner<T> {}
unsafe impl<T: Send> Send for Inner<T> {}

pub struct Producer<T> {
    inner: Arc<Inner<T>>,
}
pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
}

/// Create a ring holding up to `capacity` items.
pub fn ring<T: Copy + Default + Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let cap = capacity.max(1) + 1;
    let buf: Box<[UnsafeCell<T>]> = (0..cap).map(|_| UnsafeCell::new(T::default())).collect();
    let inner = Arc::new(Inner {
        buf,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        dropped: AtomicU64::new(0),
    });
    (
        Producer {
            inner: inner.clone(),
        },
        Consumer { inner },
    )
}

impl<T: Copy> Producer<T> {
    /// Push as many items as fit; the rest are counted as dropped. Returns number written.
    pub fn push_slice(&mut self, items: &[T]) -> usize {
        let i = &*self.inner;
        let cap = i.buf.len();
        let head = i.head.load(Ordering::Relaxed);
        let tail = i.tail.load(Ordering::Acquire);
        let free = (tail + cap - head - 1) % cap;
        let n = items.len().min(free);
        for (k, v) in items[..n].iter().enumerate() {
            unsafe { *i.buf[(head + k) % cap].get() = *v };
        }
        i.head.store((head + n) % cap, Ordering::Release);
        if n < items.len() {
            i.dropped
                .fetch_add((items.len() - n) as u64, Ordering::Relaxed);
        }
        n
    }

    pub fn push(&mut self, v: T) -> bool {
        self.push_slice(std::slice::from_ref(&v)) == 1
    }
}

impl<T: Copy> Consumer<T> {
    /// Pop up to `out.len()` items into `out`; returns count.
    pub fn pop_slice(&mut self, out: &mut [T]) -> usize {
        let i = &*self.inner;
        let cap = i.buf.len();
        let tail = i.tail.load(Ordering::Relaxed);
        let head = i.head.load(Ordering::Acquire);
        let avail = (head + cap - tail) % cap;
        let n = out.len().min(avail);
        for (k, o) in out[..n].iter_mut().enumerate() {
            *o = unsafe { *i.buf[(tail + k) % cap].get() };
        }
        i.tail.store((tail + n) % cap, Ordering::Release);
        n
    }

    pub fn pop(&mut self) -> Option<T>
    where
        T: Default,
    {
        let mut v = [T::default()];
        (self.pop_slice(&mut v) == 1).then_some(v[0])
    }

    /// Copy of the next item without consuming it.
    pub fn peek(&self) -> Option<T> {
        let i = &*self.inner;
        let tail = i.tail.load(Ordering::Relaxed);
        (i.head.load(Ordering::Acquire) != tail).then(|| unsafe { *i.buf[tail].get() })
    }

    pub fn len(&self) -> usize {
        let i = &*self.inner;
        let cap = i.buf.len();
        (i.head.load(Ordering::Acquire) + cap - i.tail.load(Ordering::Relaxed)) % cap
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Discard everything currently buffered.
    pub fn clear(&mut self) {
        let i = &*self.inner;
        i.tail
            .store(i.head.load(Ordering::Acquire), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_and_ordered() {
        let (mut p, mut c) = ring::<u32>(4);
        assert_eq!(p.push_slice(&[1, 2, 3, 4, 5, 6]), 4);
        assert_eq!(c.dropped(), 2);
        let mut out = [0u32; 8];
        assert_eq!(c.pop_slice(&mut out), 4);
        assert_eq!(&out[..4], &[1, 2, 3, 4]);
        assert!(p.push(7));
        assert_eq!(c.pop(), Some(7));
        assert_eq!(c.pop(), None);
    }

    #[test]
    fn threaded() {
        let (mut p, mut c) = ring::<u64>(1024);
        let t = std::thread::spawn(move || {
            let mut next = 0u64;
            while next < 100_000 {
                if p.push(next) {
                    next += 1;
                } else {
                    std::thread::yield_now();
                }
            }
        });
        let mut expect = 0u64;
        while expect < 100_000 {
            if let Some(v) = c.pop() {
                assert_eq!(v, expect);
                expect += 1;
            } else {
                std::thread::yield_now();
            }
        }
        t.join().unwrap();
    }
}
