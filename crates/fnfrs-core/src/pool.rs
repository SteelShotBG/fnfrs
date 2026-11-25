//! Fixed-capacity, zero-allocation object pool for hot-loop entities
//! (notes, sustain trails, splashes, hold covers, and judgement popups).

/// Handle identifying an active slot in a [`FixedPool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PoolHandle {
    pub index: usize,
    pub generation: u32,
}

#[derive(Debug, Clone)]
struct PoolSlot<T> {
    value: T,
    alive: bool,
    generation: u32,
}

/// Pre-allocated object pool that never grows or allocates during gameplay frames.
#[derive(Debug)]
pub struct FixedPool<T> {
    slots: Vec<PoolSlot<T>>,
    free_stack: Vec<usize>,
    active_count: usize,
}

impl<T> FixedPool<T> {
    /// Creates a pool of `capacity` pre-initialized slots using `factory`.
    #[must_use]
    pub fn with_capacity(capacity: usize, mut factory: impl FnMut(usize) -> T) -> Self {
        let mut slots = Vec::with_capacity(capacity);
        let mut free_stack = Vec::with_capacity(capacity);

        for i in 0..capacity {
            slots.push(PoolSlot {
                value: factory(i),
                alive: false,
                generation: 0,
            });
        }
        // Push in reverse so lowest indices are acquired first.
        for i in (0..capacity).rev() {
            free_stack.push(i);
        }

        Self {
            slots,
            free_stack,
            active_count: 0,
        }
    }

    /// Total capacity of the pool.
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Number of currently alive objects.
    #[inline]
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.active_count
    }

    /// Acquires an inactive slot without allocating, returning its [`PoolHandle`]
    /// and a mutable reference so the caller can reset its fields.
    #[inline]
    pub fn acquire(&mut self) -> Option<(PoolHandle, &mut T)> {
        let index = self.free_stack.pop()?;
        let slot = &mut self.slots[index];
        slot.alive = true;
        slot.generation = slot.generation.wrapping_add(1);
        self.active_count += 1;
        let handle = PoolHandle {
            index,
            generation: slot.generation,
        };
        Some((handle, &mut slot.value))
    }

    /// Releases an active slot back into the pool if the handle matches.
    #[inline]
    pub fn release(&mut self, handle: PoolHandle) -> bool {
        let Some(slot) = self.slots.get_mut(handle.index) else {
            return false;
        };
        if !slot.alive || slot.generation != handle.generation {
            return false;
        }
        slot.alive = false;
        self.active_count -= 1;
        self.free_stack.push(handle.index);
        true
    }

    /// Releases all active slots without freeing memory.
    pub fn clear(&mut self) {
        self.free_stack.clear();
        for (i, slot) in self.slots.iter_mut().enumerate().rev() {
            slot.alive = false;
            self.free_stack.push(i);
        }
        self.active_count = 0;
    }

    /// Returns an immutable reference if `handle` is alive and generation matches.
    #[inline]
    #[must_use]
    pub fn get(&self, handle: PoolHandle) -> Option<&T> {
        let slot = self.slots.get(handle.index)?;
        if slot.alive && slot.generation == handle.generation {
            Some(&slot.value)
        } else {
            None
        }
    }

    /// Returns a mutable reference if `handle` is alive and generation matches.
    #[inline]
    pub fn get_mut(&mut self, handle: PoolHandle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index)?;
        if slot.alive && slot.generation == handle.generation {
            Some(&mut slot.value)
        } else {
            None
        }
    }

    /// Iterates over all alive items with their handles without allocating.
    #[inline]
    pub fn iter_alive(&self) -> impl Iterator<Item = (PoolHandle, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.alive.then_some((
                PoolHandle {
                    index,
                    generation: slot.generation,
                },
                &slot.value,
            ))
        })
    }

    /// Mutably iterates over all alive items with their handles without allocating.
    #[inline]
    pub fn iter_alive_mut(&mut self) -> impl Iterator<Item = (PoolHandle, &mut T)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.alive.then_some((
                    PoolHandle {
                        index,
                        generation: slot.generation,
                    },
                    &mut slot.value,
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_and_generation_safety() {
        let mut pool = FixedPool::with_capacity(2, |i| i * 10);
        assert_eq!(pool.capacity(), 2);
        assert_eq!(pool.active_count(), 0);

        let (h0, v0) = pool.acquire().unwrap();
        *v0 = 42;
        let (h1, _) = pool.acquire().unwrap();
        assert!(pool.acquire().is_none());
        assert_eq!(pool.active_count(), 2);

        assert!(pool.release(h0));
        assert!(!pool.release(h0)); // double-free ignored
        assert!(pool.get(h0).is_none());

        let (h0_new, v0_new) = pool.acquire().unwrap();
        assert_eq!(h0_new.index, h0.index);
        assert_ne!(h0_new.generation, h0.generation);
        assert!(pool.get(h0).is_none()); // stale handle rejected
        assert_eq!(*v0_new, 42);

        pool.clear();
        assert_eq!(pool.active_count(), 0);
        assert!(pool.get(h1).is_none());
    }
}
