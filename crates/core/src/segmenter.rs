use anyhow::{Result, anyhow};

use crate::{Document, Segment};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentMode {
    Paragraph,
    Sentence,
}

impl Default for SegmentMode {
    fn default() -> Self {
        Self::Paragraph
    }
}

#[derive(Clone, Debug)]
pub struct SegmenterConfig {
    pub target_chars: usize,
    pub max_chars: usize,
    pub mode: SegmentMode,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        Self {
            target_chars: 320,
            max_chars: 550,
            mode: SegmentMode::Paragraph,
        }
    }
}

pub fn normalize_text(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn segment_document(
    title: Option<String>,
    raw: &str,
    config: &SegmenterConfig,
) -> Result<(Document, Vec<Segment>)> {
    if config.max_chars == 0 {
        return Err(anyhow!("max_chars must be positive"));
    }
    if matches!(config.mode, SegmentMode::Paragraph) && config.target_chars == 0 {
        return Err(anyhow!("target_chars must be positive in paragraph mode"));
    }
    if matches!(config.mode, SegmentMode::Paragraph) && config.target_chars > config.max_chars {
        return Err(anyhow!("target_chars must be <= max_chars"));
    }

    let normalized = normalize_text(raw);
    let document = Document::new(title, normalized);
    let segments = build_segments(&document, config);
    Ok((document, segments))
}

fn build_segments(document: &Document, config: &SegmenterConfig) -> Vec<Segment> {
    let segments = match config.mode {
        SegmentMode::Paragraph => build_paragraph_segments(document, config),
        SegmentMode::Sentence => build_sentence_segments(document, config.max_chars),
    };
    assign_segment_ids(segments)
}

fn build_paragraph_segments(document: &Document, config: &SegmenterConfig) -> Vec<Segment> {
    let text = document.text();
    let mut segments = Vec::new();

    for (paragraph_index, (paragraph_start, paragraph_end)) in
        paragraph_spans(text).into_iter().enumerate()
    {
        let paragraph = &text[paragraph_start..paragraph_end];
        let spans = sentence_spans(paragraph, paragraph_start);
        segments.extend(pack_sentence_spans(text, paragraph_index, spans, config));
    }

    segments
}

fn build_sentence_segments(document: &Document, max_chars: usize) -> Vec<Segment> {
    let text = document.text();
    let mut segments = Vec::new();

    for (paragraph_index, (paragraph_start, paragraph_end)) in
        paragraph_spans(text).into_iter().enumerate()
    {
        let paragraph = &text[paragraph_start..paragraph_end];
        let spans = sentence_spans(paragraph, paragraph_start);
        segments.extend(sentence_segments(text, paragraph_index, spans, max_chars));
    }

    segments
}

fn assign_segment_ids(mut segments: Vec<Segment>) -> Vec<Segment> {
    for (id, segment) in segments.iter_mut().enumerate() {
        segment.id = id;
    }
    segments
}

fn paragraph_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        if bytes[i] == b'\n' {
            let mut j = i;
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
                if j > i && j < len && bytes[j - 1] == b'\n' && bytes[j] == b'\n' {
                    break;
                }
            }

            if i + 1 < len && bytes[i + 1] == b'\n' {
                let end = trim_ascii_whitespace_end(text, start, i);
                if end > start {
                    spans.push((start, end));
                }
                start = skip_ascii_whitespace(text, i + 2);
                i = start;
                continue;
            }
        }
        i += 1;
    }

    let end = trim_ascii_whitespace_end(text, start, len);
    if end > start {
        spans.push((start, end));
    }
    spans
}

fn sentence_spans(paragraph: &str, paragraph_offset: usize) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = skip_ascii_whitespace(paragraph, 0);
    let mut iter = paragraph.char_indices().peekable();

    while let Some((idx, ch)) = iter.next() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }

        let mut end = idx + ch.len_utf8();
        while let Some((next_idx, next_ch)) = iter.peek().copied() {
            if matches!(next_ch, '"' | '\'' | ')' | ']' | '}') {
                end = next_idx + next_ch.len_utf8();
                iter.next();
            } else {
                break;
            }
        }

        let next = skip_ascii_whitespace(paragraph, end);
        if next == paragraph.len() || paragraph[end..].starts_with(char::is_whitespace) {
            if end > start {
                spans.push((paragraph_offset + start, paragraph_offset + end));
            }
            start = next;
        }
    }

    if start < paragraph.len() {
        spans.push((
            paragraph_offset + start,
            paragraph_offset + trim_ascii_whitespace_end(paragraph, start, paragraph.len()),
        ));
    }

    spans
        .into_iter()
        .filter(|(start, end)| end > start)
        .collect()
}

fn sentence_segments(
    text: &str,
    paragraph_index: usize,
    spans: Vec<(usize, usize)>,
    max_chars: usize,
) -> Vec<Segment> {
    let mut segments = Vec::new();
    for (span_start, span_end) in spans {
        append_sentence_segment(
            &mut segments,
            text,
            paragraph_index,
            span_start,
            span_end,
            max_chars,
        );
    }
    segments
}

fn pack_sentence_spans(
    text: &str,
    paragraph_index: usize,
    spans: Vec<(usize, usize)>,
    config: &SegmenterConfig,
) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current_start = None;
    let mut current_end = 0usize;

    for (span_start, span_end) in spans {
        let span_text = &text[span_start..span_end];
        let span_chars = span_text.chars().count();

        if span_chars > config.max_chars {
            if let Some(start) = current_start.take() {
                segments.push(Segment::new(
                    0,
                    paragraph_index,
                    start,
                    current_end,
                    text[start..current_end].to_string(),
                ));
            }
            append_sentence_segment(
                &mut segments,
                text,
                paragraph_index,
                span_start,
                span_end,
                config.max_chars,
            );
            continue;
        }

        let tentative_chars = if let Some(start) = current_start {
            text[start..span_end].chars().count()
        } else {
            span_chars
        };

        if tentative_chars > config.max_chars && current_start.is_some() {
            let start = current_start.take().unwrap();
            segments.push(Segment::new(
                0,
                paragraph_index,
                start,
                current_end,
                text[start..current_end].to_string(),
            ));
        }

        if current_start.is_none() {
            current_start = Some(span_start);
        }
        current_end = span_end;

        let current_chars = text[current_start.unwrap()..current_end].chars().count();

        if current_chars >= config.target_chars {
            let start = current_start.take().unwrap();
            segments.push(Segment::new(
                0,
                paragraph_index,
                start,
                current_end,
                text[start..current_end].to_string(),
            ));
        }
    }

    if let Some(start) = current_start {
        segments.push(Segment::new(
            0,
            paragraph_index,
            start,
            current_end,
            text[start..current_end].to_string(),
        ));
    }

    segments
}

fn append_sentence_segment(
    segments: &mut Vec<Segment>,
    text: &str,
    paragraph_index: usize,
    span_start: usize,
    span_end: usize,
    max_chars: usize,
) {
    let span_text = &text[span_start..span_end];
    if span_text.chars().count() > max_chars {
        for (split_start, split_end) in split_long_span(span_text, span_start, max_chars) {
            segments.push(Segment::new(
                0,
                paragraph_index,
                split_start,
                split_end,
                text[split_start..split_end].to_string(),
            ));
        }
        return;
    }

    segments.push(Segment::new(
        0,
        paragraph_index,
        span_start,
        span_end,
        span_text.to_string(),
    ));
}

fn split_long_span(text: &str, absolute_start: usize, max_chars: usize) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut local_start = 0usize;

    while local_start < text.len() {
        let mut count = 0usize;
        let mut last_break = None;
        let mut local_end = text.len();

        for (idx, ch) in text[local_start..].char_indices() {
            count += 1;
            if ch.is_whitespace() {
                last_break = Some(local_start + idx);
            }
            if count >= max_chars {
                local_end = last_break.unwrap_or(local_start + idx + ch.len_utf8());
                break;
            }
        }

        if local_end <= local_start {
            local_end = text.len();
        }

        let trimmed_start = absolute_start + skip_ascii_whitespace(text, local_start);
        let trimmed_end = absolute_start + trim_ascii_whitespace_end(text, local_start, local_end);
        if trimmed_end > trimmed_start {
            spans.push((trimmed_start, trimmed_end));
        }

        local_start = skip_ascii_whitespace(text, local_end);
    }

    spans
}

fn skip_ascii_whitespace(text: &str, mut idx: usize) -> usize {
    while idx < text.len() {
        let ch = text[idx..].chars().next().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        idx += ch.len_utf8();
    }
    idx
}

fn trim_ascii_whitespace_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let ch = text[..end].chars().next_back().unwrap();
        if !ch.is_whitespace() {
            break;
        }
        end -= ch.len_utf8();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::{SegmentMode, SegmenterConfig, segment_document};

    #[test]
    fn splits_long_text_into_multiple_segments() {
        let text = "First sentence. Second sentence that is reasonably long. Third sentence.\n\nNext paragraph here.";
        let (_, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 25,
                max_chars: 45,
                mode: SegmentMode::Paragraph,
            },
        )
        .unwrap();
        assert!(segments.len() >= 2);
        assert!(segments.iter().all(|segment| !segment.text().is_empty()));
    }

    #[test]
    fn multi_segment_output_respects_limits_and_offsets() {
        let text = concat!(
            "First paragraph starts here. It keeps going long enough to force chunking. ",
            "Another sentence follows so the segmenter has to make a decision.\n\n",
            "Second paragraph also contains several sentences. It should become multiple segments ",
            "without producing empty fragments or out-of-order offsets."
        );
        let (_, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 55,
                max_chars: 80,
                mode: SegmentMode::Paragraph,
            },
        )
        .unwrap();

        assert!(segments.len() >= 3);
        for (expected_id, segment) in segments.iter().enumerate() {
            assert_eq!(segment.id, expected_id);
            assert!(segment.end_byte > segment.start_byte);
            assert!(segment.len_chars() <= 80);
            assert!(!segment.text().trim().is_empty());
        }

        for pair in segments.windows(2) {
            assert!(pair[0].end_byte <= pair[1].start_byte);
            assert!(pair[0].paragraph_index <= pair[1].paragraph_index);
        }
    }

    #[test]
    fn sentence_mode_keeps_sentences_separate() {
        let text = "First sentence. Second sentence! Third sentence?";
        let (_, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 100,
                max_chars: 100,
                mode: SegmentMode::Sentence,
            },
        )
        .unwrap();

        let segment_texts = segments
            .iter()
            .map(|segment| segment.text().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            segment_texts,
            vec!["First sentence.", "Second sentence!", "Third sentence?"]
        );
    }

    #[test]
    fn sentence_mode_allows_small_max_chars_without_target_validation() {
        let text = "A much longer sentence that should be split when sentence mode is active.";
        let (_, segments) = segment_document(
            None,
            text,
            &SegmenterConfig {
                target_chars: 320,
                max_chars: 20,
                mode: SegmentMode::Sentence,
            },
        )
        .unwrap();

        assert!(segments.len() >= 2);
        assert!(segments.iter().all(|segment| segment.len_chars() <= 20));
    }
}
