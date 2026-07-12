//! Bounded text accumulation for streams whose producers must always be drained.

use std::collections::VecDeque;

/// Retain the beginning and end of a stream while counting everything between.
#[derive(Debug)]
pub struct BoundedText {
    head: String,
    tail: VecDeque<char>,
    head_chars: usize,
    head_len: usize,
    tail_chars: usize,
    total_chars: usize,
}

impl BoundedText {
    pub fn new(max_chars: usize) -> Self {
        let head_chars = max_chars.saturating_mul(2) / 3;
        Self {
            head: String::with_capacity(head_chars),
            tail: VecDeque::with_capacity(max_chars.saturating_sub(head_chars)),
            head_chars,
            head_len: 0,
            tail_chars: max_chars.saturating_sub(head_chars),
            total_chars: 0,
        }
    }

    pub fn push(&mut self, text: &str) {
        for ch in text.chars() {
            self.total_chars += 1;
            if self.head_len < self.head_chars {
                self.head.push(ch);
                self.head_len += 1;
            } else if self.tail_chars > 0 {
                if self.tail.len() == self.tail_chars {
                    self.tail.pop_front();
                }
                self.tail.push_back(ch);
            }
        }
    }

    pub fn total_chars(&self) -> usize {
        self.total_chars
    }

    pub fn into_string(self) -> String {
        let kept = self.head_len + self.tail.len();
        if self.total_chars <= kept {
            return self.head + &self.tail.into_iter().collect::<String>();
        }
        let dropped = self.total_chars - kept;
        format!(
            "{}\n… [{dropped} characters omitted] …\n{}",
            self.head,
            self.tail.into_iter().collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_bounded_head_and_tail_while_counting_the_whole_stream() {
        let mut text = BoundedText::new(12);
        text.push("abcdefghij");
        text.push("klmnopqrst");
        let rendered = text.into_string();
        assert!(rendered.starts_with("abcdefgh"));
        assert!(rendered.ends_with("qrst"));
        assert!(rendered.contains("8 characters omitted"));
    }

    #[test]
    fn short_stream_is_unchanged() {
        let mut text = BoundedText::new(20);
        text.push("hello");
        assert_eq!(text.into_string(), "hello");
    }
}
