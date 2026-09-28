//! Deterministic auto-chunking for capture ingestion (D-018).
//!
//! Agent transcripts arrive as long messages; memory records are short by
//! design. [`chunk_text`] splits text into bounded, self-contained chunks —
//! paragraph boundaries preferred, then sentence enders, then a hard wrap —
//! so the same transcript always produces the same chunks on any platform.

use crate::error::{RecallError, Result};

/// Split `text` into chunks of at most `max_chars` (unicode-aware).
///
/// Greedy packing: paragraphs, then sentences (`.`/`!`/`?`/newline enders),
/// then hard character wraps; consecutive small pieces are joined with single
/// spaces until `max_chars` would overflow. Empty input yields no chunks.
pub fn chunk_text(text: &str, max_chars: usize) -> Result<Vec<String>> {
    if max_chars == 0 {
        return Err(RecallError::InvalidInput("chunk size must be > 0".into()));
    }
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    if text.chars().count() <= max_chars {
        return Ok(vec![text.to_string()]);
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for piece in pieces(text, max_chars) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let piece_len = piece.chars().count();
        if piece_len > max_chars {
            // Sentence enders were exhausted; hard-wrap the remainder.
            for part in hard_wrap(piece, max_chars) {
                finish_current(&mut current, &mut chunks);
                chunks.push(part);
            }
            continue;
        }
        let current_len = current.chars().count();
        if !current.is_empty() && current_len + 1 + piece_len > max_chars {
            finish_current(&mut current, &mut chunks);
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(piece);
    }
    finish_current(&mut current, &mut chunks);
    Ok(chunks)
}

fn finish_current(current: &mut String, chunks: &mut Vec<String>) {
    if !current.is_empty() {
        chunks.push(std::mem::take(current));
    }
}

/// Break text into pieces that individually fit `max_chars`: paragraphs,
/// then sentences, then hard wraps.
fn pieces(text: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split("\n\n") {
        if paragraph.chars().count() <= max_chars {
            out.push(paragraph.to_string());
            continue;
        }
        for sentence in sentences(paragraph) {
            if sentence.chars().count() <= max_chars {
                out.push(sentence);
            } else {
                out.extend(hard_wrap(&sentence, max_chars));
            }
        }
    }
    out
}

/// Split into sentences: run of text ending at `.`/`!`/`?`/newline.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        current.push(c);
        if matches!(c, '.' | '!' | '?' | '\n') {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Character-level hard wrap (unicode-safe, never empty parts for non-empty input).
fn hard_wrap(text: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut part = String::new();
    for c in text.chars() {
        part.push(c);
        if part.chars().count() == max_chars {
            out.push(std::mem::take(&mut part));
        }
    }
    if !part.is_empty() {
        out.push(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk_and_empty_is_none() {
        assert_eq!(chunk_text("short note", 100).unwrap(), vec!["short note"]);
        assert!(chunk_text("   \n ", 100).unwrap().is_empty());
        assert!(chunk_text("", 100).unwrap().is_empty());
        // Exactly at the boundary stays one chunk.
        let boundary = "x".repeat(100);
        assert_eq!(chunk_text(&boundary, 100).unwrap(), vec![boundary]);
    }

    #[test]
    fn zero_chunk_size_is_rejected() {
        let err = chunk_text("hello", 0).unwrap_err();
        assert!(err.to_string().contains("chunk size must be > 0"));
    }

    #[test]
    fn paragraphs_and_sentences_pack_greedily() {
        let text = "alpha beta. gamma delta!\n\nepsilon zeta? eta theta.";
        let chunks = chunk_text(text, 24).unwrap();
        assert!(chunks.iter().all(|c| c.chars().count() <= 24), "{chunks:?}");
        // Nothing lost: rejoining covers every word.
        let joined = chunks.join(" ");
        for word in [
            "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
        ] {
            assert!(joined.contains(word), "lost {word} in {chunks:?}");
        }
        assert!(chunks.len() > 1);
        assert_eq!(chunk_text(text, 24).unwrap(), chunks, "deterministic");
    }

    #[test]
    fn oversized_sentence_is_hard_wrapped() {
        let text = "w".repeat(250);
        let chunks = chunk_text(&text, 100).unwrap();
        assert_eq!(
            chunks,
            vec!["w".repeat(100), "w".repeat(100), "w".repeat(50)]
        );
    }

    #[test]
    fn unicode_is_counted_by_chars_not_bytes() {
        // 3-byte chars: a byte-based wrap would produce invalid boundaries.
        let text = "é".repeat(60);
        let chunks = chunk_text(&text, 25).unwrap();
        assert!(chunks.iter().all(|c| c.chars().count() <= 25), "{chunks:?}");
        assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), 60);
    }
}
