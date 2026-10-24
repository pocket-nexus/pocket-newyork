//! Which cells' records of `NEAR` to read, and into which slot.
//!
//! A device holds the records in slots of the largest record's size. A frame asks for the cells within the near
//! reach whose records are not in a slot ([`Slots::request`]), nearest first, into free slots, or into the slot of a
//! cell no frame has drawn for a while; the device reads each into its slot as it can and says when it has
//! ([`Slots::arrived`]). A slot is given another record only when the frames that drew from it are `held` frames
//! old: a GPU may still be drawing them.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use ny_pack::NearCell;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Reading(u32),
    Ready(u32),
}

struct Slot {
    state: State,
    used: u32,
}

/// A record to read: the cell, the slot it goes to, and where it is in `NEAR`.
#[derive(Clone, Copy, Debug)]
pub struct Read {
    pub cell: u32,
    pub slot: usize,
    pub offset: u64,
    pub size: usize,
}

pub struct Slots {
    slots: Vec<Slot>,
    of_cell: BTreeMap<u32, usize>,
    held: u32,
    /// Records read, and the bytes.
    pub reads: u32,
    pub bytes: u64,
    /// Cells asked for that found no slot.
    pub starved: u32,
}

impl Slots {
    /// `count` slots; a slot drawn from in frame `f` takes another record from frame `f + held + 1`.
    pub fn new(count: usize, held: u32) -> Slots {
        Slots { slots: (0..count).map(|_| Slot { state: State::Free, used: 0 }).collect(), of_cell: BTreeMap::new(), held, reads: 0, bytes: 0, starved: 0 }
    }

    /// A read into `slot` for `cell` is done (`ok`) or failed.
    pub fn arrived(&mut self, cell: u32, slot: usize, ok: bool) {
        let s = &mut self.slots[slot];
        if s.state == State::Reading(cell) {
            if ok {
                s.state = State::Ready(cell);
            } else {
                s.state = State::Free;
                self.of_cell.remove(&cell);
            }
        }
    }

    /// The slot holding a cell's record, once it has been read.
    pub fn ready(&self, cell: u32) -> Option<usize> {
        let &slot = self.of_cell.get(&cell)?;
        (self.slots[slot].state == State::Ready(cell)).then_some(slot)
    }

    /// Notes that frame `frame` draws from a cell's record.
    pub fn touch(&mut self, cell: u32, frame: u32) {
        if let Some(&slot) = self.of_cell.get(&cell) {
            self.slots[slot].used = frame;
        }
    }

    /// Asks for the records of `wanted` cells (with their distances), nearest first: `read` hears each one to start.
    pub fn request(&mut self, wanted: &mut [(u32, f32)], cells: &[NearCell], frame: u32, mut read: impl FnMut(Read)) {
        wanted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(core::cmp::Ordering::Equal));
        for &(cell, _) in wanted.iter() {
            if self.of_cell.contains_key(&cell) {
                continue;
            }
            let rec = cells[cell as usize];
            if rec.size == 0 {
                continue;
            }
            // A free slot, or the one least lately drawn from that the GPU is done with.
            let slot = match self.slots.iter().position(|s| s.state == State::Free) {
                Some(i) => Some(i),
                None => self
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s.state, State::Ready(_)) && s.used + self.held < frame)
                    .min_by_key(|(_, s)| s.used)
                    .map(|(i, _)| i),
            };
            let Some(slot) = slot else {
                self.starved += 1;
                return;
            };
            if let State::Ready(old) = self.slots[slot].state {
                self.of_cell.remove(&old);
            }
            let s = &mut self.slots[slot];
            s.state = State::Reading(cell);
            s.used = frame;
            self.of_cell.insert(cell, slot);
            self.reads += 1;
            self.bytes += rec.size as u64;
            read(Read { cell, slot, offset: rec.offset as u64, size: rec.size as usize });
        }
    }

    /// Records being read.
    pub fn reading(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.state, State::Reading(_))).count()
    }

    /// Slots holding a record (or being read into), of all.
    pub fn filled(&self) -> (usize, usize) {
        (self.slots.iter().filter(|s| s.state != State::Free).count(), self.slots.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn cells(n: usize) -> Vec<NearCell> {
        (0..n).map(|i| NearCell { offset: i as u32 * 100, size: 100, ..Default::default() }).collect()
    }

    #[test]
    fn records_go_nearest_first_into_free_slots_then_the_least_lately_drawn() {
        let cells = cells(4);
        let mut slots = Slots::new(2, 3);
        let mut reads = Vec::new();
        slots.request(&mut vec![(2, 50.0), (0, 10.0), (1, 30.0)], &cells, 1, |r| reads.push(r));
        // Two slots: the two nearest, and the third starved.
        assert_eq!(reads.iter().map(|r| (r.cell, r.slot, r.offset)).collect::<Vec<_>>(), [(0, 0, 0), (1, 1, 100)]);
        assert_eq!(slots.starved, 1);
        assert_eq!((slots.ready(0), slots.reading()), (None, 2));
        slots.arrived(0, 0, true);
        slots.arrived(1, 1, false);
        assert_eq!((slots.ready(0), slots.ready(1), slots.filled()), (Some(0), None, (1, 2)));
        // A slot drawn from in frame 5 is not given away before frame 9.
        slots.touch(0, 5);
        reads.clear();
        slots.request(&mut vec![(3, 5.0), (2, 9.0)], &cells, 8, |r| reads.push(r));
        assert_eq!(reads.iter().map(|r| (r.cell, r.slot)).collect::<Vec<_>>(), [(3, 1)]);
        slots.arrived(3, 1, true);
        reads.clear();
        slots.request(&mut vec![(2, 9.0)], &cells, 9, |r| reads.push(r));
        assert_eq!(reads.iter().map(|r| (r.cell, r.slot)).collect::<Vec<_>>(), [(2, 0)]);
        assert_eq!(slots.ready(0), None);
    }
}
