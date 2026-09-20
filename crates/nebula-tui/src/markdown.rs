//! MARKDOWN — CommonMark plus GitHub's tables, strikethrough, task lists
//! and alerts, laid out as styled ratatui lines for one width: what the
//! FILE TABS and the TREE BROWSER show for a `.md` file, and what the
//! ISSUES and PR reading panes make of a GitHub body.
//!
//! The whole document is flowed up front against the pane width (the PR
//! PREVIEW's rule): scrolling is a slice and the line count is exact, so
//! the scroller always knows how far down it may go. Nothing here draws —
//! the lines go straight into a `Paragraph`, and every one of them fits
//! the width, because ratatui clips an overwide line silently.
//!
//! A cell grid can do bold, italic, underline, colour and box drawing, and
//! that is the whole vocabulary: headings bold in the accent with a rule
//! under the first two levels, lists re-bulleted and renumbered with
//! hanging indents, quotes behind a bar, fenced code on a raised surface
//! with the TREE BROWSER's highlighting, tables in aligned columns, links
//! underlined with the address dim beside them. Images are their alt text
//! in brackets — a terminal has no picture to show — and raw HTML stays
//! raw, dimmed, rather than being guessed at.

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::syntax::Highlighter;
use crate::theme::Theme;

/// Bullets by nesting depth, cycling past the third level.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];
/// The bar a block quote sits behind.
const QUOTE_BAR: &str = "▎ ";
/// Left inset of a code block's raised surface.
const CODE_PAD: &str = " ";
/// Narrowest a table column is squeezed to before the table is allowed to
/// overflow the pane.
const MIN_COL_W: usize = 3;
/// What sits between two table columns.
const COL_SEP: &str = " │ ";

/// Is this a file the preview panes render as markdown?
pub fn is_markdown_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(
        name.rsplit_once('.').map(|(_, ext)| ext),
        Some("md" | "markdown" | "mdown" | "mkd" | "mkdn")
    )
}

/// What a newline inside a paragraph means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breaks {
    /// CommonMark: a soft break is a space and the paragraph reflows — a
    /// file its author wrapped at 80 columns.
    Reflow,
    /// GitHub's comment rule: every newline is a line break — an issue or
    /// pull request body typed into the browser, where a list without
    /// markers reads as a list only if its rows stay rows.
    Hard,
}

/// Lay `text` out as markdown in `width` columns. `base` is the style of
/// ordinary prose (the reading panes keep their body muted); everything
/// else is built on top of it from the theme.
pub fn render(
    text: &str,
    width: usize,
    breaks: Breaks,
    base: Style,
    th: Theme,
) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_GFM);
    opts.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    let mut r = Renderer::new(width, breaks, base, th);
    for event in Parser::new_ext(text, opts) {
        r.event(event);
    }
    r.finish()
}

/// A document flowed for one width, kept on its view between draws so a
/// frame that changed nothing else — a scroll, the running-row sweep —
/// doesn't lay the whole file out again. The view drops it when the text
/// or the mode changes; a draw at another width flows afresh.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub width: u16,
    pub lines: Vec<Line<'static>>,
}

impl Rendered {
    /// `cached` when it was flowed for `width`, else a fresh flow of `text`.
    pub fn for_width(
        cached: Option<Rendered>,
        text: &str,
        width: u16,
        breaks: Breaks,
        th: Theme,
    ) -> Rendered {
        match cached {
            Some(r) if r.width == width => r,
            _ => Rendered {
                width,
                lines: render(text, width as usize, breaks, Style::default(), th),
            },
        }
    }
}

/// The lines with `by` in front of each — the reading panes' inset. The
/// caller flowed them `by` narrower to make room.
pub fn indent(lines: Vec<Line<'static>>, by: &str) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::raw(by.to_string()));
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// One piece of a paragraph before it is flowed: a run of characters in
/// one style, and how it joins what came before it.
#[derive(Debug, Clone)]
struct Atom {
    text: String,
    style: Style,
    kind: Kind,
    /// An image's stand-in (`[alt]`), so a link that is only a badge can
    /// keep its address to itself.
    image: bool,
}

/// A flowed row: its spans and their width in columns.
type Row = (Vec<Span<'static>>, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A space came before it: the line may break here.
    Word,
    /// Glued to the previous atom (`**bo**ld` is one word).
    Glue,
    /// A forced line break.
    Break,
}

/// One entry of the left gutter every emitted line starts with.
#[derive(Debug, Clone)]
enum Gutter {
    /// A block quote's bar, in the quote's colour.
    Quote(Style),
    /// A list item's hanging indent — the marker's width.
    Indent(usize),
}

#[derive(Debug, Clone)]
struct ListState {
    /// The next number of an ordered list; `None` for bullets.
    next: Option<u64>,
    /// Items hold paragraphs (blank lines between them in the source), so
    /// a blank line goes between them in the output too.
    loose: bool,
}

/// A table under construction: every cell keeps its atoms, to be flowed
/// once the columns have their widths.
#[derive(Debug, Clone)]
struct Table {
    aligns: Vec<Alignment>,
    rows: Vec<Vec<Vec<Atom>>>,
    row: Vec<Vec<Atom>>,
    /// How many leading rows are the header — one, or none.
    head_rows: usize,
}

struct Renderer {
    th: Theme,
    base: Style,
    breaks: Breaks,
    width: usize,
    out: Vec<Line<'static>>,
    /// Left gutter, innermost last.
    gutter: Vec<Gutter>,
    /// A list item's marker, drawn in place of its indent on the next
    /// content line only.
    marker: Option<Vec<Span<'static>>>,
    /// The paragraph, heading or cell being built.
    inline: Vec<Atom>,
    /// A space was seen since the last atom: the next one is a `Word`.
    space: bool,
    bold: usize,
    italic: usize,
    strike: usize,
    link: usize,
    heading: Option<HeadingLevel>,
    table_head: bool,
    lists: Vec<ListState>,
    /// The block before ended: the next content line gets a blank above.
    need_blank: bool,
    /// A code block being collected: its info string and text.
    code: Option<(String, String)>,
    /// Inside an HTML or metadata block, whose text is shown raw.
    raw_block: bool,
    table: Option<Table>,
    /// Open links: the address and where in `inline` the text began.
    links: Vec<(String, usize)>,
    /// Open images: where in `inline` the alt text began.
    images: Vec<usize>,
}

impl Renderer {
    fn new(width: usize, breaks: Breaks, base: Style, th: Theme) -> Self {
        Self {
            th,
            base,
            breaks,
            width: width.max(1),
            out: Vec::new(),
            gutter: Vec::new(),
            marker: None,
            inline: Vec::new(),
            space: false,
            bold: 0,
            italic: 0,
            strike: 0,
            link: 0,
            heading: None,
            table_head: false,
            lists: Vec::new(),
            need_blank: false,
            code: None,
            raw_block: false,
            table: None,
            links: Vec::new(),
            images: Vec::new(),
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline();
        while self
            .out
            .last()
            .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
        {
            self.out.pop();
        }
        self.out
    }

    // ---- styles ----

    /// The style prose takes right now, from the enclosing tags.
    fn style(&self) -> Style {
        let mut s = self.base;
        if self.gutter.iter().any(|g| matches!(g, Gutter::Quote(_))) {
            s = s.fg(self.th.muted);
        }
        if let Some(level) = self.heading {
            s = s.add_modifier(Modifier::BOLD);
            if (level as usize) <= 3 {
                s = s.fg(self.th.accent);
            }
        }
        if self.table_head || self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link > 0 {
            s = s.fg(self.th.accent).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn dim(&self) -> Style {
        self.base.fg(self.th.dim)
    }

    fn edge(&self) -> Style {
        Style::default().fg(self.th.edge)
    }

    // ---- geometry ----

    fn gutter_width(&self) -> usize {
        self.gutter
            .iter()
            .map(|g| match g {
                Gutter::Quote(_) => QUOTE_BAR.width(),
                Gutter::Indent(n) => *n,
            })
            .sum()
    }

    /// Columns left for content once the gutter has its share.
    fn avail(&self) -> usize {
        self.width.saturating_sub(self.gutter_width()).max(1)
    }

    /// The gutter as spans; with `with_marker`, a pending list marker
    /// takes the place of its indent and is spent.
    fn gutter_spans(&mut self, with_marker: bool) -> Vec<Span<'static>> {
        let marker_at = if with_marker && self.marker.is_some() {
            self.gutter
                .iter()
                .rposition(|g| matches!(g, Gutter::Indent(_)))
        } else {
            None
        };
        let mut spans = Vec::with_capacity(self.gutter.len());
        for (i, g) in self.gutter.iter().enumerate() {
            match g {
                Gutter::Quote(style) => spans.push(Span::styled(QUOTE_BAR, *style)),
                Gutter::Indent(n) => {
                    if Some(i) == marker_at {
                        spans.extend(self.marker.take().unwrap_or_default());
                    } else {
                        spans.push(Span::raw(" ".repeat(*n)));
                    }
                }
            }
        }
        spans
    }

    // ---- output ----

    /// The blank line the block before asked for, if any — never as the
    /// very first row.
    fn settle_blank(&mut self) {
        if self.need_blank && !self.out.is_empty() {
            self.blank();
        }
        self.need_blank = false;
    }

    /// One content line, behind the gutter, after the blank line the
    /// block before asked for.
    fn emit(&mut self, content: Vec<Span<'static>>) {
        self.settle_blank();
        let mut spans = self.gutter_spans(true);
        spans.extend(content);
        self.out.push(fit_width(spans, self.width));
    }

    /// A blank line — still behind the quote bars, so a quote reads as one
    /// block across its paragraphs.
    fn blank(&mut self) {
        let spans = self.gutter_spans(false);
        self.out.push(fit_width(spans, self.width));
    }

    // ---- inline collection ----

    fn push_text(&mut self, text: &str, style: Style) {
        let text = text.replace('\t', "    ");
        for (i, piece) in text.split(' ').enumerate() {
            if i > 0 {
                self.space = true;
            }
            if piece.is_empty() {
                continue;
            }
            let at_start = matches!(
                self.inline.last(),
                None | Some(Atom {
                    kind: Kind::Break,
                    ..
                })
            );
            let kind = if self.space || at_start {
                Kind::Word
            } else {
                Kind::Glue
            };
            self.inline.push(Atom {
                text: piece.to_string(),
                style,
                kind,
                image: false,
            });
            self.space = false;
        }
    }

    fn push_break(&mut self) {
        self.inline.push(Atom {
            text: String::new(),
            style: Style::default(),
            kind: Kind::Break,
            image: false,
        });
        self.space = false;
    }

    /// The plain text of the atoms from `start` on.
    fn inline_text(&self, start: usize) -> String {
        let mut s = String::new();
        for (i, a) in self.inline.iter().skip(start).enumerate() {
            match a.kind {
                Kind::Word if i > 0 => s.push(' '),
                Kind::Break => s.push(' '),
                _ => {}
            }
            s.push_str(&a.text);
        }
        s
    }

    /// Flow the collected paragraph into lines at the width left beside
    /// the gutter (see [`flow`]). Returns the widest content row.
    fn flush_inline(&mut self) -> usize {
        let atoms = std::mem::take(&mut self.inline);
        self.space = false;
        if atoms.is_empty() {
            return 0;
        }
        let rows = flow(&atoms, self.avail(), self.base);
        let widest = rows.iter().map(|(_, w)| *w).max().unwrap_or(0);
        for (row, _) in rows {
            self.emit(row);
        }
        widest
    }

    // ---- blocks ----

    /// A code block: every source line on the raised surface, highlighted
    /// by the fence's language, broken at the edge rather than reflowed
    /// (code has no words to wrap on), never numbered.
    fn code_block(&mut self, lang: &str, text: &str) {
        let avail = self.avail();
        let inner = avail.saturating_sub(CODE_PAD.width()).max(1);
        let surface = Style::default().bg(self.th.sel_bg_dim);
        let mut hl = if lang.trim().is_empty() {
            Highlighter::plain()
        } else {
            Highlighter::for_lang(lang)
        };
        let text = text.replace('\t', "    ");
        let mut lines: Vec<&str> = text.lines().collect();
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        for line in lines {
            let runs: Vec<(String, Style)> = hl
                .line(line)
                .into_iter()
                .map(|(kind, t)| {
                    (
                        t,
                        crate::ui::token_style(kind, self.th).bg(self.th.sel_bg_dim),
                    )
                })
                .collect();
            for chunk in chunk_runs(&runs, inner) {
                let used: usize = chunk.iter().map(|s| s.width()).sum();
                let mut spans = vec![Span::styled(CODE_PAD, surface)];
                spans.extend(chunk);
                let pad = avail.saturating_sub(CODE_PAD.width() + used);
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), surface));
                }
                self.emit(spans);
            }
        }
    }

    /// Raw HTML or front matter: as written, dim, each source line its
    /// own row, wrapped on words when it is too wide.
    fn raw_lines(&mut self, text: &str) {
        let dim = self.dim();
        for line in text.lines() {
            self.push_text(line, dim);
            self.push_break();
        }
        self.flush_inline();
    }

    /// A table: columns sized the way a browser sizes them (see
    /// [`column_widths`]), every cell wrapped to its column so nothing is
    /// cut, a rule under the header — and no header at all when its cells
    /// are all blank, the `| | |` that only wanted the rule.
    fn table_lines(&mut self, t: Table) {
        let cols = t
            .aligns
            .len()
            .max(t.rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if cols == 0 || t.rows.is_empty() {
            return;
        }
        let base = self.base;
        let mut natural = vec![1usize; cols];
        let mut minimum = vec![1usize; cols];
        for row in &t.rows {
            for (c, cell) in row.iter().enumerate() {
                natural[c] = natural[c].max(natural_width(cell, base));
                minimum[c] = minimum[c].max(longest_word(cell));
            }
        }
        let seps = COL_SEP.width() * (cols - 1);
        let widths = column_widths(&natural, &minimum, self.avail().saturating_sub(seps));

        let mut rows = t.rows;
        let mut head_rows = t.head_rows;
        let blank_head = head_rows == 1
            && rows[0]
                .iter()
                .all(|cell| cell.iter().all(|a| a.text.trim().is_empty()));
        if blank_head {
            rows.remove(0);
            head_rows = 0;
        }
        let edge = self.edge();
        for (r, row) in rows.iter().enumerate() {
            let cells: Vec<Vec<Row>> = widths
                .iter()
                .enumerate()
                .map(|(c, w)| {
                    let atoms = row.get(c).map(Vec::as_slice).unwrap_or(&[]);
                    flow(atoms, *w, base)
                })
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(0).max(1);
            for line in 0..height {
                let mut spans = Vec::new();
                for (c, cell) in cells.iter().enumerate() {
                    if c > 0 {
                        spans.push(Span::styled(COL_SEP, edge));
                    }
                    let (content, used) = cell.get(line).cloned().unwrap_or_default();
                    let align = t.aligns.get(c).copied().unwrap_or(Alignment::None);
                    spans.extend(aligned(content, used, widths[c], align, base));
                }
                self.emit(spans);
            }
            if r + 1 == head_rows {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                self.emit(vec![Span::styled(rule.join("─┼─"), edge)]);
            }
        }
    }

    // ---- events ----

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => {
                if let Some((_, code)) = &mut self.code {
                    code.push_str(&t);
                } else if self.raw_block {
                    self.raw_lines(&t);
                } else {
                    let style = self.style();
                    self.push_text(&t, style);
                }
            }
            Event::Code(t) => {
                let style = self.style().fg(self.th.special);
                self.push_text(&t, style);
            }
            Event::Html(t) => self.raw_lines(&t),
            Event::InlineHtml(t) => {
                if t.trim_start().to_ascii_lowercase().starts_with("<br") {
                    self.push_break();
                } else {
                    let style = self.dim();
                    self.push_text(&t, style);
                }
            }
            Event::SoftBreak => match self.breaks {
                Breaks::Reflow => self.space = true,
                Breaks::Hard => self.push_break(),
            },
            Event::HardBreak => self.push_break(),
            Event::Rule => {
                self.flush_inline();
                self.need_blank = true;
                let rule = "─".repeat(self.avail());
                let edge = self.edge();
                self.emit(vec![Span::styled(rule, edge)]);
                self.need_blank = true;
            }
            Event::TaskListMarker(done) => {
                let (glyph, style) = if done {
                    ("☑", self.base.fg(self.th.ok))
                } else {
                    ("☐", self.dim())
                };
                self.push_text(glyph, style);
                self.space = true;
            }
            Event::FootnoteReference(label) => {
                let style = self.dim();
                self.push_text(&format!("[^{label}]"), style);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if let Some(list) = self.lists.last_mut() {
                    list.loose = true;
                }
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.need_blank = true;
                self.heading = Some(level);
            }
            Tag::BlockQuote(kind) => {
                self.flush_inline();
                // The gap above a quote is the page's, not the quote's: the
                // bar starts on its first row.
                self.need_blank = true;
                self.settle_blank();
                let (label, color) = match kind {
                    None => (None, self.th.edge),
                    Some(BlockQuoteKind::Note) => (Some("Note"), self.th.accent),
                    Some(BlockQuoteKind::Tip) => (Some("Tip"), self.th.ok),
                    Some(BlockQuoteKind::Important) => (Some("Important"), self.th.special),
                    Some(BlockQuoteKind::Warning) => (Some("Warning"), self.th.warn),
                    Some(BlockQuoteKind::Caution) => (Some("Caution"), self.th.err),
                };
                self.gutter.push(Gutter::Quote(Style::default().fg(color)));
                if let Some(label) = label {
                    self.emit(vec![Span::styled(
                        label,
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )]);
                }
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.need_blank = true;
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => {
                self.flush_inline();
                self.need_blank = true;
                self.raw_block = true;
            }
            Tag::List(start) => {
                self.flush_inline();
                if self.lists.is_empty() {
                    self.need_blank = true;
                }
                self.lists.push(ListState {
                    next: start,
                    loose: false,
                });
            }
            Tag::Item => {
                self.flush_inline();
                // An item that opens straight into a nested list: its own
                // marker gets a row of its own rather than being lost.
                if self.marker.is_some() {
                    self.emit(Vec::new());
                }
                let depth = self.lists.len().saturating_sub(1);
                let text = match self.lists.last_mut() {
                    Some(ListState { next: Some(n), .. }) => {
                        let label = format!("{n}. ");
                        *n += 1;
                        label
                    }
                    _ => format!("{} ", BULLETS[depth % BULLETS.len()]),
                };
                let w = text.width();
                self.marker = Some(vec![Span::styled(
                    text,
                    Style::default().fg(self.th.accent),
                )]);
                self.gutter.push(Gutter::Indent(w));
            }
            Tag::Table(aligns) => {
                self.flush_inline();
                self.need_blank = true;
                self.table = Some(Table {
                    aligns,
                    rows: Vec::new(),
                    row: Vec::new(),
                    head_rows: 0,
                });
            }
            Tag::TableHead => self.table_head = true,
            Tag::TableRow => {}
            Tag::TableCell => {
                self.inline.clear();
                self.space = false;
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.link += 1;
                self.links.push((dest_url.to_string(), self.inline.len()));
            }
            Tag::Image { .. } => self.images.push(self.inline.len()),
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_inline();
                self.need_blank = true;
            }
            TagEnd::Heading(level) => {
                let w = self.flush_inline();
                self.heading = None;
                let rule = match level {
                    HeadingLevel::H1 => Some("━"),
                    HeadingLevel::H2 => Some("─"),
                    _ => None,
                };
                if let (Some(ch), true) = (rule, w > 0) {
                    let edge = self.edge();
                    self.emit(vec![Span::styled(ch.repeat(w.min(self.avail())), edge)]);
                }
                self.need_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.gutter.pop();
                self.need_blank = true;
            }
            TagEnd::CodeBlock => {
                let (lang, text) = self.code.take().unwrap_or_default();
                self.code_block(&lang, &text);
                self.need_blank = true;
            }
            TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {
                self.raw_block = false;
                self.need_blank = true;
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.need_blank = true;
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                // An empty item still shows its marker.
                if self.marker.is_some() {
                    self.emit(Vec::new());
                }
                self.gutter.pop();
                if self.lists.last().is_some_and(|l| l.loose) {
                    self.need_blank = true;
                }
            }
            TagEnd::Table => {
                self.table_head = false;
                if let Some(t) = self.table.take() {
                    self.table_lines(t);
                }
                self.need_blank = true;
            }
            TagEnd::TableHead => {
                self.table_head = false;
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                    t.head_rows = 1;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                self.space = false;
                if let Some(t) = &mut self.table {
                    t.row.push(cell);
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.link = self.link.saturating_sub(1);
                if let Some((dest, start)) = self.links.pop() {
                    let text = self.inline_text(start);
                    // A badge — a link that is nothing but an image — is
                    // read, not followed: its address stays out of the way.
                    let badge =
                        start < self.inline.len() && self.inline[start..].iter().all(|a| a.image);
                    if !badge && shows_address(&dest, &text) {
                        let style = self.dim();
                        self.space = true;
                        self.push_text(&format!("({dest})"), style);
                    }
                }
            }
            TagEnd::Image => {
                if let Some(start) = self.images.pop() {
                    let alt = self.inline_text(start);
                    let kind = self.inline.get(start).map_or(Kind::Word, |a| a.kind);
                    self.inline.truncate(start);
                    let alt = alt.trim();
                    let label = if alt.is_empty() {
                        "[image]".to_string()
                    } else {
                        format!("[{alt}]")
                    };
                    let style = self.dim();
                    self.inline.push(Atom {
                        text: label,
                        style,
                        kind,
                        image: true,
                    });
                }
            }
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }
}

/// Is a link's address worth showing beside its text? Not when the text
/// already is the address, and not for a jump within the page.
fn shows_address(dest: &str, text: &str) -> bool {
    let dest = dest.trim();
    if dest.is_empty() || dest.starts_with('#') {
        return false;
    }
    let bare = dest.strip_prefix("mailto:").unwrap_or(dest);
    let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
    !same(bare, text.trim()) && !same(dest, text.trim())
}

/// Append one character, extending the last span when its style matches.
fn push_char(line: &mut Vec<Span<'static>>, ch: char, style: Style) {
    match line.last_mut() {
        Some(span) if span.style == style => span.content.to_mut().push(ch),
        _ => line.push(Span::styled(ch.to_string(), style)),
    }
}

/// Append a run, extending the last span when its style matches, so a
/// link or a bold phrase is one span rather than one per word.
fn push_run(line: &mut Vec<Span<'static>>, text: &str, style: Style) {
    match line.last_mut() {
        Some(span) if span.style == style => span.content.to_mut().push_str(text),
        _ => line.push(Span::styled(text.to_string(), style)),
    }
}

/// Split styled runs into rows of at most `width` columns, at characters —
/// for code and raw text, which have no word gaps to prefer. Always at
/// least one row, so an empty line stays a line.
fn chunk_runs(runs: &[(String, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut cur = 0usize;
    for (text, style) in runs {
        for ch in text.chars() {
            let cw = ch.width().unwrap_or(0);
            if cur > 0 && cur + cw > width {
                rows.push(std::mem::take(&mut row));
                cur = 0;
            }
            push_char(&mut row, ch, *style);
            cur += cw;
        }
    }
    rows.push(row);
    rows
}

/// Flow atoms into rows no wider than `avail`, breaking on word gaps and,
/// for a word that can never fit, at the edge. A gap between two words in
/// one style keeps the style (an underlined link stays underlined across
/// it); any other gap is `base`. A forced break ends a row even when the
/// row is empty.
fn flow(atoms: &[Atom], avail: usize, base: Style) -> Vec<Row> {
    let avail = avail.max(1);
    let mut rows: Vec<Row> = Vec::new();
    let mut line: Vec<Span<'static>> = Vec::new();
    let mut cur = 0usize;
    let mut i = 0;
    while i < atoms.len() {
        if atoms[i].kind == Kind::Break {
            rows.push((std::mem::take(&mut line), cur));
            cur = 0;
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < atoms.len() && atoms[j].kind == Kind::Glue {
            j += 1;
        }
        let word = &atoms[i..j];
        let w: usize = word.iter().map(|a| a.text.width()).sum();
        if cur > 0 && cur + 1 + w > avail {
            rows.push((std::mem::take(&mut line), cur));
            cur = 0;
        }
        if cur > 0 {
            let prev = line.last().map(|s| s.style);
            let gap = match word.first() {
                Some(a) if prev == Some(a.style) => a.style,
                _ => base,
            };
            push_run(&mut line, " ", gap);
            cur += 1;
        }
        if w > avail {
            for a in word {
                for ch in a.text.chars() {
                    let cw = ch.width().unwrap_or(0);
                    if cur > 0 && cur + cw > avail {
                        rows.push((std::mem::take(&mut line), cur));
                        cur = 0;
                    }
                    push_char(&mut line, ch, a.style);
                    cur += cw;
                }
            }
        } else {
            for a in word {
                push_run(&mut line, &a.text, a.style);
            }
            cur += w;
        }
        i = j;
    }
    if !line.is_empty() {
        rows.push((line, cur));
    }
    rows
}

/// The widest word in `atoms` — what a table column can't go under
/// without breaking words.
fn longest_word(atoms: &[Atom]) -> usize {
    let mut widest = 0usize;
    let mut cur = 0usize;
    for a in atoms {
        match a.kind {
            Kind::Glue => cur += a.text.width(),
            Kind::Word => {
                widest = widest.max(cur);
                cur = a.text.width();
            }
            Kind::Break => {
                widest = widest.max(cur);
                cur = 0;
            }
        }
    }
    widest.max(cur)
}

/// How wide `atoms` would be with no wrapping at all — the widest of its
/// forced-break rows.
fn natural_width(atoms: &[Atom], base: Style) -> usize {
    flow(atoms, usize::MAX, base)
        .iter()
        .map(|(_, w)| *w)
        .max()
        .unwrap_or(0)
}

/// Column widths in `avail` columns, given what each column would like
/// (`natural`, its widest cell unwrapped) and the least it can take
/// without breaking a word (`minimum`, its longest word) — the way a
/// browser lays a table out. Everything it wants when that fits; else
/// every column at least its longest word, the slack shared in proportion
/// to what each wanted beyond that; and when even the longest words don't
/// fit, the widest columns give way first, down to a floor, and words
/// break at the edge.
fn column_widths(natural: &[usize], minimum: &[usize], avail: usize) -> Vec<usize> {
    let want: usize = natural.iter().sum();
    if want <= avail {
        return natural.to_vec();
    }
    let minimum: Vec<usize> = natural
        .iter()
        .zip(minimum)
        .map(|(n, m)| (*m).min(*n).max(1))
        .collect();
    let need: usize = minimum.iter().sum();
    if need <= avail {
        let slack = avail - need;
        let wants: usize = natural.iter().zip(&minimum).map(|(n, m)| n - m).sum();
        let mut widths: Vec<usize> = natural
            .iter()
            .zip(&minimum)
            .map(|(n, m)| {
                m + if wants == 0 {
                    0
                } else {
                    slack * (n - m) / wants
                }
            })
            .collect();
        // Integer shares leave a column or two of slack over: the columns
        // that wanted the most take it, one each.
        let mut left = avail - widths.iter().sum::<usize>();
        let mut order: Vec<usize> = (0..widths.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(natural[i]));
        for i in order {
            if left == 0 {
                break;
            }
            if widths[i] < natural[i] {
                widths[i] += 1;
                left -= 1;
            }
        }
        return widths;
    }
    let mut widths = minimum;
    let mut total = need;
    while total > avail {
        let Some((i, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > MIN_COL_W)
            .max_by_key(|(_, w)| **w)
        else {
            break;
        };
        widths[i] -= 1;
        total -= 1;
    }
    widths
}

/// One flowed table row padded out to its column's width and alignment.
fn aligned(
    mut spans: Vec<Span<'static>>,
    used: usize,
    width: usize,
    align: Alignment,
    base: Style,
) -> Vec<Span<'static>> {
    let pad = width.saturating_sub(used);
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        Alignment::Left | Alignment::None => (0, pad),
    };
    if left > 0 {
        spans.insert(0, Span::styled(" ".repeat(left), base));
    }
    if right > 0 {
        spans.push(Span::styled(" ".repeat(right), base));
    }
    spans
}

/// The safety net under every emitted line: nothing wider than the pane
/// leaves this module, whatever the flow above decided.
fn fit_width(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let total: usize = spans.iter().map(|s| s.width()).sum();
    if total <= width {
        return Line::from(spans);
    }
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    'outer: for span in spans {
        for ch in span.content.chars() {
            let cw = ch.width().unwrap_or(0);
            if used + cw > width {
                break 'outer;
            }
            push_char(&mut kept, ch, span.style);
            used += cw;
        }
    }
    Line::from(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn th() -> Theme {
        Theme::default()
    }

    fn lines(text: &str, width: usize) -> Vec<Line<'static>> {
        render(text, width, Breaks::Reflow, Style::default(), th())
    }

    fn plain(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn span_with<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Span<'static> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(text))
            .unwrap_or_else(|| panic!("no span containing {text:?} in {:?}", plain(lines)))
    }

    #[test]
    fn markdown_files_are_told_by_extension() {
        assert!(is_markdown_path("docs/keys.md"));
        assert!(is_markdown_path("/abs/README.MD"));
        assert!(is_markdown_path("notes.markdown"));
        assert!(!is_markdown_path("src/main.rs"));
        assert!(!is_markdown_path("README"));
        assert!(!is_markdown_path("md"));
    }

    #[test]
    fn headings_are_bold_accent_with_a_rule_under_the_first_two_levels() {
        let out = lines(
            "# Title\n\nbody\n\n## Section\n\n### Sub\n\n#### Minor\n",
            40,
        );
        assert_eq!(
            plain(&out),
            [
                "Title",
                "━━━━━",
                "",
                "body",
                "",
                "Section",
                "───────",
                "",
                "Sub",
                "",
                "Minor"
            ]
        );
        let title = span_with(&out, "Title");
        assert_eq!(title.style.fg, Some(th().accent));
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        let minor = span_with(&out, "Minor");
        assert_eq!(minor.style.fg, None, "H4+ is bold but not accent");
        assert!(minor.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(span_with(&out, "━").style.fg, Some(th().edge));
    }

    #[test]
    fn paragraphs_reflow_on_words_and_soft_breaks_follow_the_mode() {
        let text = "one two three\nfour five six seven";
        assert_eq!(
            plain(&lines(text, 12)),
            ["one two", "three four", "five six", "seven"]
        );
        let hard = render(text, 40, Breaks::Hard, Style::default(), th());
        assert_eq!(plain(&hard), ["one two three", "four five six seven"]);
        // A trailing double space is a hard break in either mode.
        assert_eq!(plain(&lines("a  \nb", 40)), ["a", "b"]);
        // Paragraph breaks survive; runs of spaces collapse.
        assert_eq!(plain(&lines("a\n\nb   c", 40)), ["a", "", "b c"]);
    }

    #[test]
    fn a_word_wider_than_the_pane_breaks_at_the_edge() {
        let out = lines("see https://example.dev/a/very/long/path/indeed now", 12);
        assert_eq!(
            plain(&out),
            [
                "see",
                "https://exam",
                "ple.dev/a/ve",
                "ry/long/path",
                "/indeed now"
            ]
        );
    }

    #[test]
    fn emphasis_maps_to_modifiers_and_inline_code_to_the_keyword_colour() {
        let out = lines("**bold** *it* ~~gone~~ `code` **bo**ld", 40);
        assert_eq!(plain(&out), ["bold it gone code bold"]);
        assert!(span_with(&out, "bold")
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert!(span_with(&out, "it")
            .style
            .add_modifier
            .contains(Modifier::ITALIC));
        assert!(span_with(&out, "gone")
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT));
        assert_eq!(span_with(&out, "code").style.fg, Some(th().special));
        // `**bo**ld` is one word: its two spans sit on one row unbroken.
        let last = out.last().unwrap();
        let texts: Vec<&str> = last.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(texts.ends_with(&["bo", "ld"]), "{texts:?}");
    }

    #[test]
    fn lists_get_bullets_numbers_and_hanging_indents() {
        let text =
            "- alpha beta gamma delta\n- two\n  - nested item\n    - deeper\n3. three\n4. four\n";
        let out = lines(text, 16);
        assert_eq!(
            plain(&out),
            [
                "• alpha beta",
                "  gamma delta",
                "• two",
                "  ◦ nested item",
                "    ▪ deeper",
                "",
                "3. three",
                "4. four",
            ]
        );
        assert_eq!(span_with(&out, "•").style.fg, Some(th().accent));
        assert_eq!(span_with(&out, "3. ").style.fg, Some(th().accent));
    }

    #[test]
    fn loose_lists_keep_a_blank_between_items_and_tight_ones_do_not() {
        assert_eq!(plain(&lines("- a\n- b\n", 20)), ["• a", "• b"]);
        assert_eq!(
            plain(&lines("- a\n\n- b\n\n  second para\n", 20)),
            ["• a", "", "• b", "", "  second para"]
        );
        // A paragraph before a list gets its blank; the list's items don't.
        assert_eq!(
            plain(&lines("intro\n- a\n- b\n\nafter", 20)),
            ["intro", "", "• a", "• b", "", "after"]
        );
    }

    #[test]
    fn task_lists_show_their_boxes() {
        let out = lines("- [x] done\n- [ ] todo\n", 20);
        assert_eq!(plain(&out), ["• ☑ done", "• ☐ todo"]);
        assert_eq!(span_with(&out, "☑").style.fg, Some(th().ok));
        assert_eq!(span_with(&out, "☐").style.fg, Some(th().dim));
    }

    #[test]
    fn fenced_code_sits_on_a_surface_highlighted_and_broken_at_the_edge() {
        let text = "before\n\n```rust\nfn main() {\n    let x = \"a very long string literal here\";\n}\n```\n\nafter\n";
        let out = lines(text, 24);
        assert_eq!(
            plain(&out),
            [
                "before",
                "",
                " fn main() {",
                "     let x = \"a very lon",
                " g string literal here\";",
                " }",
                "",
                "after",
            ]
        );
        let code_rows = &out[2..6];
        for row in code_rows {
            assert!(
                row.spans
                    .iter()
                    .all(|s| s.style.bg == Some(th().sel_bg_dim)),
                "every span on a code row carries the surface: {row:?}"
            );
            assert_eq!(row.width(), 24, "code rows fill the width: {row:?}");
        }
        assert_eq!(span_with(&out, "fn").style.fg, Some(th().special));
        assert_eq!(span_with(&out, "\"a very").style.fg, Some(th().ok));
        assert_eq!(span_with(&out, "before").style.bg, None);
    }

    #[test]
    fn an_indented_or_unlabelled_block_is_plain_on_the_surface() {
        let out = lines("```\nplain text\n```\n", 20);
        assert_eq!(plain(&out), [" plain text"]);
        let out = lines("    four spaces\n", 20);
        assert_eq!(plain(&out), [" four spaces"]);
        assert_eq!(span_with(&out, "four").style.fg, None);
    }

    #[test]
    fn quotes_sit_behind_a_bar_and_alerts_get_a_label() {
        let out = lines("> quoted words\n> go on\n>\n> second\n\nplain\n", 18);
        assert_eq!(
            plain(&out),
            ["▎ quoted words go", "▎ on", "▎", "▎ second", "", "plain"]
        );
        assert_eq!(span_with(&out, "▎").style.fg, Some(th().edge));
        assert_eq!(span_with(&out, "quoted").style.fg, Some(th().muted));
        assert_eq!(span_with(&out, "plain").style.fg, None);

        let out = lines("> [!WARNING]\n> careful now\n", 30);
        assert_eq!(plain(&out), ["▎ Warning", "▎ careful now"]);
        assert_eq!(span_with(&out, "Warning").style.fg, Some(th().warn));
        assert_eq!(span_with(&out, "▎").style.fg, Some(th().warn));
    }

    #[test]
    fn a_list_inside_a_quote_and_a_quote_inside_a_list_keep_both_gutters() {
        assert_eq!(plain(&lines("> - a\n> - b\n", 20)), ["▎ • a", "▎ • b"]);
        assert_eq!(
            plain(&lines("- item\n\n  > quoted\n", 20)),
            ["• item", "", "  ▎ quoted"]
        );
    }

    #[test]
    fn rules_span_the_content_width() {
        let out = lines("a\n\n---\n\nb\n", 10);
        assert_eq!(plain(&out), ["a", "", "──────────", "", "b"]);
        let out = lines("- x\n\n  ---\n", 10);
        assert_eq!(plain(&out), ["• x", "", "  ────────"]);
    }

    #[test]
    fn tables_align_their_columns_and_rule_under_the_header() {
        let text =
            "| Key | Value | N |\n|:----|:-----:|--:|\n| a | middle | 1 |\n| longer | b | 22 |\n";
        let out = lines(text, 40);
        assert_eq!(
            plain(&out),
            [
                "Key    │ Value  │  N",
                "───────┼────────┼───",
                "a      │ middle │  1",
                "longer │   b    │ 22",
            ]
        );
        assert!(span_with(&out, "Key")
            .style
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(span_with(&out, "│").style.fg, Some(th().edge));
        assert_eq!(
            span_with(&out, "middle").style.add_modifier,
            Modifier::empty()
        );
    }

    #[test]
    fn a_wide_table_wraps_its_cells_rather_than_cutting_them() {
        let text = "| a | b |\n|---|---|\n| short | a much longer cell than fits |\n";
        let out = lines(text, 20);
        assert_eq!(
            plain(&out),
            [
                "a     │ b",
                "──────┼─────────────",
                "short │ a much",
                "      │ longer cell",
                "      │ than fits",
            ]
        );
        for row in &out {
            assert!(row.width() <= 20, "{row:?}");
        }
    }

    #[test]
    fn a_blank_header_row_is_dropped_with_its_rule() {
        assert_eq!(
            plain(&lines("| | |\n|---|---|\n| a | b |\n", 20)),
            ["a │ b"]
        );
    }

    #[test]
    fn columns_get_what_they_want_then_their_longest_word_then_the_floor() {
        assert_eq!(column_widths(&[5, 10], &[3, 4], 30), [5, 10], "it all fits");
        // 40 wanted, 20 available: each column at least its longest word
        // (6 + 4 = 10), the other 10 shared 3:1 by what they wanted beyond.
        assert_eq!(column_widths(&[36, 4], &[6, 4], 20), [16, 4]);
        // Slack the integer shares leave over goes to the widest wanter.
        assert_eq!(column_widths(&[10, 10, 10], &[2, 2, 2], 13), [5, 4, 4]);
        // Even the longest words don't fit: the widest give way, evenly.
        assert_eq!(column_widths(&[20, 20], &[20, 20], 10), [5, 5]);
        assert_eq!(column_widths(&[20, 8], &[20, 8], 12), [6, 6]);
        assert_eq!(
            column_widths(&[20, 20], &[20, 20], 2),
            [3, 3],
            "the floor holds; the row is clipped rather than a column erased"
        );
    }

    #[test]
    fn links_show_the_address_dim_unless_it_is_the_text_or_an_anchor() {
        let out = lines(
            "see [the docs](https://example.dev/docs) and <https://x.y> and [top](#top)",
            80,
        );
        assert_eq!(
            plain(&out),
            ["see the docs (https://example.dev/docs) and https://x.y and top"]
        );
        let text = span_with(&out, "the docs");
        assert_eq!(text.style.fg, Some(th().accent));
        assert!(text.style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(
            span_with(&out, "(https://example.dev/docs)").style.fg,
            Some(th().dim)
        );
        assert!(!shows_address("mailto:a@b.c", "a@b.c"));
        assert!(!shows_address("https://a.b/", "https://a.b"));
        assert!(shows_address("https://a.b/x", "a.b"));
    }

    #[test]
    fn images_are_their_alt_text_in_brackets() {
        let out = lines(
            "![CI status](https://img.example/ci.svg) and ![](x.png)",
            80,
        );
        assert_eq!(plain(&out), ["[CI status] and [image]"]);
        assert_eq!(span_with(&out, "[CI status]").style.fg, Some(th().dim));
        // A badge — a link that is only an image — keeps its address to
        // itself; a link with words beside the image still shows it.
        let out = lines(
            "[![b](i.svg)](https://ci.example) [![c](i.svg) docs](https://d.example)",
            80,
        );
        assert_eq!(plain(&out), ["[b] [c] docs (https://d.example)"]);
    }

    #[test]
    fn html_stays_raw_and_dim_and_br_breaks_the_line() {
        let out = lines(
            "<p align=\"center\">\n  <img src=\"x.png\">\n</p>\n\ntext<br>more <b>bold</b>\n",
            40,
        );
        assert_eq!(
            plain(&out),
            [
                "<p align=\"center\">",
                "<img src=\"x.png\">",
                "</p>",
                "",
                "text",
                "more <b>bold</b>",
            ]
        );
        assert_eq!(span_with(&out, "<p").style.fg, Some(th().dim));
        assert_eq!(span_with(&out, "<b>").style.fg, Some(th().dim));
        // A long tag wraps on its words rather than mid-attribute.
        let out = lines("<img src=\"long.png\" alt=\"some words here\">\n", 24);
        assert_eq!(
            plain(&out),
            ["<img src=\"long.png\"", "alt=\"some words here\">"]
        );
    }

    #[test]
    fn front_matter_is_shown_dim_as_written() {
        let out = lines("---\nname: x\ntags: [a, b]\n---\n\n# Doc\n", 40);
        assert_eq!(plain(&out), ["name: x", "tags: [a, b]", "", "Doc", "━━━"]);
        assert_eq!(span_with(&out, "name: x").style.fg, Some(th().dim));
    }

    #[test]
    fn the_base_style_carries_into_prose_but_not_headings_or_code() {
        let muted = Style::default().fg(Color::Gray);
        let out = render("# H\n\nprose `code`\n", 40, Breaks::Reflow, muted, th());
        assert_eq!(span_with(&out, "prose").style.fg, Some(Color::Gray));
        assert_eq!(span_with(&out, "H").style.fg, Some(th().accent));
        assert_eq!(span_with(&out, "code").style.fg, Some(th().special));
    }

    #[test]
    fn empty_and_blank_input_render_nothing_and_trailing_blanks_are_trimmed() {
        assert!(lines("", 40).is_empty());
        assert!(lines("  \n\n", 40).is_empty());
        assert_eq!(plain(&lines("a\n\n\n\n", 40)), ["a"]);
    }

    #[test]
    fn the_cache_is_reused_for_its_width_only_and_indent_insets_every_row() {
        let first = Rendered::for_width(None, "# T\n\nbody", 40, Breaks::Reflow, th());
        assert_eq!(first.width, 40);
        let again = Rendered::for_width(Some(first.clone()), "ignored", 40, Breaks::Reflow, th());
        assert_eq!(
            plain(&again.lines),
            plain(&first.lines),
            "same width: the cache"
        );
        let other = Rendered::for_width(Some(first), "fresh", 30, Breaks::Reflow, th());
        assert_eq!(
            (other.width, plain(&other.lines)),
            (30, vec!["fresh".to_string()])
        );
        assert_eq!(
            plain(&indent(lines("a\n\nb", 10), "  ")),
            ["  a", "", "  b"]
        );
    }

    /// Every rendered row fits the pane at every width, for a document
    /// that exercises each construct — the one promise the scroller and
    /// ratatui both depend on.
    #[test]
    fn no_row_is_ever_wider_than_the_pane() {
        let text = "# A heading that goes on for a while\n\n\
            Some prose with **bold**, a [link](https://example.dev/quite/a/long/address/really) and `code`.\n\n\
            - a bullet with enough words to wrap a couple of times over\n  - nested\n1. one\n\n\
            > quoted text that is also long enough to need wrapping at narrow widths\n\n\
            ```rust\nfn main() { println!(\"a fairly long line of code that will not fit\"); }\n```\n\n\
            | col one | column two | three |\n|---|---|---|\n| x | a longer cell | yy |\n\n\
            ---\n\n<div>raw html that is long enough to wrap around the edge</div>\n\n\
            日本語のテキストと絵文字 🎉 mixed with ascii words here\n";
        for width in [4usize, 7, 12, 20, 33, 48, 80, 120] {
            for row in lines(text, width) {
                assert!(
                    row.width() <= width,
                    "width {width}: {} cols in {:?}",
                    row.width(),
                    plain(std::slice::from_ref(&row))
                );
            }
        }
        for width in [12usize, 40] {
            for row in render(text, width, Breaks::Hard, Style::default(), th()) {
                assert!(row.width() <= width, "hard {width}: {row:?}");
            }
        }
    }
}
