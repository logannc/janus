//! Fake prompter for testing.
//!
//! Pre-loaded with a queue of responses. Each `select()` call pops the next
//! response from the front. Errors if the queue is exhausted or a response
//! index is out of range for the given items.

use anyhow::{bail, Result};
use std::cell::RefCell;
use std::collections::VecDeque;

use super::Prompter;

/// Mock prompter — returns pre-configured responses in order.
pub struct FakePrompter {
    /// FIFO queue of selection indices to return.
    responses: RefCell<VecDeque<usize>>,
}

impl FakePrompter {
    /// Create a prompter that will return the given responses in order.
    ///
    /// Each call to `select()` pops the front of the queue.
    pub fn new(responses: Vec<usize>) -> Self {
        Self {
            responses: RefCell::new(responses.into()),
        }
    }

    /// How many unconsumed responses remain.
    pub fn remaining(&self) -> usize {
        self.responses.borrow().len()
    }
}

impl Prompter for FakePrompter {
    fn select(&self, prompt: &str, items: &[&str], _default: usize) -> Result<usize> {
        let response = self.responses.borrow_mut().pop_front();
        match response {
            Some(idx) => {
                if idx >= items.len() {
                    bail!(
                        "FakePrompter: response index {} out of range for {} items (prompt: \"{}\")",
                        idx,
                        items.len(),
                        prompt
                    );
                }
                Ok(idx)
            }
            None => bail!(
                "FakePrompter: no more responses queued (prompt: \"{}\")",
                prompt
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_returns_responses_in_order() {
        let prompter = FakePrompter::new(vec![0, 2, 1]);
        assert_eq!(
            prompter.select("q1", &["a", "b", "c"], 0).unwrap(),
            0
        );
        assert_eq!(
            prompter.select("q2", &["a", "b", "c"], 0).unwrap(),
            2
        );
        assert_eq!(
            prompter.select("q3", &["a", "b", "c"], 0).unwrap(),
            1
        );
    }

    #[test]
    fn test_exhausted_queue_fails() {
        let prompter = FakePrompter::new(vec![0]);
        prompter.select("q1", &["a"], 0).unwrap();
        assert!(prompter.select("q2", &["a"], 0).is_err());
    }

    #[test]
    fn test_out_of_range_fails() {
        let prompter = FakePrompter::new(vec![5]);
        assert!(prompter.select("q1", &["a", "b"], 0).is_err());
    }

    #[test]
    fn test_remaining() {
        let prompter = FakePrompter::new(vec![0, 1]);
        assert_eq!(prompter.remaining(), 2);
        prompter.select("q", &["a", "b"], 0).unwrap();
        assert_eq!(prompter.remaining(), 1);
    }
}
