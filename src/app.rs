use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::deck::{Deck, Slide, load_deck};
use crate::graphics::{ImageBackend, ImageCompositor, detect_backend, placements_for_view};
use crate::layout::{SearchMatch, SlideLayout, Viewport, build_layout, viewport_lines};
use crate::tmux::{TmuxRuntime, VisibilityState};

pub fn run() -> Result<()> {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let deck = load_deck(&dir)?;
    let tmux = TmuxRuntime::detect();
    let backend = detect_backend(&tmux);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)?;
    let backend_term = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_term)?;
    let result = App::new(dir, deck, tmux, backend).run(&mut terminal);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), Show, LeaveAlternateScreen).ok();
    result
}

struct App {
    dir: PathBuf,
    deck: Deck,
    current: usize,
    status: String,
    help: bool,
    outline: bool,
    search_focus: bool,
    outline_search_focus: bool,
    search: String,
    search_matches: Vec<SearchMatch>,
    search_match_index: usize,
    outline_query: String,
    text_scroll: usize,
    body_rows: usize,
    text_rect: Option<Rect>,
    layout_cache: HashMap<(usize, u16), SlideLayout>,
    outline_filtered: Vec<usize>,
    outline_index: usize,
    outline_scroll: usize,
    tmux: TmuxRuntime,
    visibility: VisibilityState,
    image_backend: Box<dyn ImageBackend>,
    compositor: ImageCompositor,
    image_debug: String,
    last_visibility_poll: Instant,
}

impl App {
    fn new(
        dir: PathBuf,
        deck: Deck,
        tmux: TmuxRuntime,
        image_backend: Box<dyn ImageBackend>,
    ) -> Self {
        let mut app = Self {
            dir,
            deck,
            current: 0,
            status: String::new(),
            help: false,
            outline: false,
            search_focus: false,
            outline_search_focus: false,
            search: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            outline_query: String::new(),
            text_scroll: 0,
            body_rows: 0,
            text_rect: None,
            layout_cache: HashMap::new(),
            outline_filtered: Vec::new(),
            outline_index: 0,
            outline_scroll: 0,
            visibility: VisibilityState::default(),
            compositor: ImageCompositor::new(tmux.runtime_id.clone()),
            tmux,
            image_backend,
            image_debug: String::new(),
            last_visibility_poll: Instant::now(),
        };
        app.recompute_outline();
        app
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        let mut dirty = true;
        loop {
            if self.poll_visibility(terminal.backend_mut())? {
                dirty = true;
            }
            if dirty {
                terminal.draw(|frame| self.draw(frame))?;
                self.draw_images(terminal.backend_mut())?;
                dirty = false;
            }
            if event::poll(Duration::from_millis(80))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.handle_key(key, terminal.backend_mut())? {
                            break;
                        }
                        dirty = true;
                    }
                    Event::Resize(_, _) => dirty = true,
                    _ => {}
                }
            }
        }
        self.clear_images(terminal.backend_mut())?;
        Ok(())
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        if self.help {
            self.text_rect = None;
            self.draw_help(frame);
            return;
        }
        if self.outline {
            self.text_rect = None;
            self.draw_outline(frame);
            return;
        }

        let size = frame.area();
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(size);
        let slide = self.current_slide();
        let slide_title = slide.title.clone();
        let slide_name = slide.name.clone();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " ss ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(slide_title, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(slide_name, Style::default().fg(Color::DarkGray)),
            ])),
            vertical[0],
        );

        self.text_rect = Some(vertical[1]);
        self.body_rows = vertical[1].height as usize;
        let scroll = self.text_scroll;
        let body_rows = self.body_rows;
        let matches = self.search_matches.clone();
        let selected_match = (!matches.is_empty()).then_some(self.search_match_index);
        let layout = self.layout_for_current(vertical[1].width);
        let viewport = viewport_lines(layout, scroll, body_rows, &matches, selected_match);
        frame.render_widget(
            Paragraph::new(viewport).wrap(Wrap { trim: false }),
            vertical[1],
        );
        frame.render_widget(self.footer(), vertical[2]);
    }

    fn draw_help(&self, frame: &mut ratatui::Frame) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("ss help"),
                Line::from(""),
                Line::from("Navigation: arrows, h j k l, g/G, ctrl-u, ctrl-d, r, q"),
                Line::from("Search: / current slide, n/N next or previous hit"),
                Line::from("Outline: o, / filter, enter open"),
                Line::from("Graphics: explicit image ownership and tmux visibility gating"),
            ]),
            frame.area(),
        );
    }

    fn draw_outline(&mut self, frame: &mut ratatui::Frame) {
        let size = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(size);
        let query = if self.outline_search_focus {
            format!("/ {}", self.outline_query)
        } else {
            format!("  {}", self.outline_query)
        };
        frame.render_widget(Paragraph::new(query), chunks[0]);

        let visible = chunks[1].height as usize;
        let end = self
            .outline_filtered
            .len()
            .min(self.outline_scroll + visible);
        let mut lines = Vec::new();
        for (offset, slide_index) in self.outline_filtered[self.outline_scroll..end]
            .iter()
            .enumerate()
        {
            let absolute = self.outline_scroll + offset;
            let slide = &self.deck.slides[*slide_index];
            let active = absolute == self.outline_index;
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {:>2} ", slide_index + 1),
                    if active {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::raw(" "),
                Span::styled(
                    slide.title.clone(),
                    if active {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
            ]));
        }
        if lines.is_empty() {
            lines.push(Line::from("no slides matched"));
        }
        frame.render_widget(Paragraph::new(lines), chunks[1]);
        frame.render_widget(
            Paragraph::new(format!(
                "{}/{} slides",
                self.outline_filtered.len(),
                self.deck.slides.len()
            )),
            chunks[2],
        );
    }

    fn footer(&self) -> Paragraph<'static> {
        let slide = self.current_slide();
        let mode_status = if self.search_focus {
            format!("/{}", self.search)
        } else if self.outline_search_focus {
            format!("/{}", self.outline_query)
        } else {
            self.status.clone()
        };
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {}/{} ", self.current + 1, self.deck.slides.len()),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("backend:{}", self.image_backend.name()),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  "),
            Span::styled(
                format!("images:{}", slide.assets.len()),
                Style::default().fg(Color::Blue),
            ),
            Span::raw("  "),
            Span::styled(self.scroll_status(), Style::default().fg(Color::Magenta)),
            Span::raw("  "),
            Span::styled(
                self.image_debug.clone(),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled(mode_status, Style::default().fg(Color::DarkGray)),
        ]))
    }

    fn handle_key(&mut self, key: KeyEvent, stdout: &mut CrosstermBackend<Stdout>) -> Result<bool> {
        if self.search_focus {
            return Ok(self.handle_search_key(key));
        }
        if self.outline_search_focus {
            return Ok(self.handle_outline_search_key(key));
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('?') => {
                self.help = !self.help;
                self.outline = false;
            }
            KeyCode::Esc => {
                if self.help {
                    self.help = false;
                } else if self.outline {
                    self.outline = false;
                } else if !self.search.is_empty() {
                    self.clear_search();
                }
            }
            KeyCode::Char('o') => {
                self.outline = !self.outline;
                self.recompute_outline();
            }
            KeyCode::Char('/') => {
                if self.outline {
                    self.outline_search_focus = true;
                } else {
                    self.search_focus = true;
                    self.update_search_matches();
                }
            }
            KeyCode::Char('r') => {
                self.clear_images(stdout)?;
                self.deck = load_deck(&self.dir)?;
                self.layout_cache.clear();
                self.current = self.current.min(self.deck.slides.len().saturating_sub(1));
                self.update_search_matches();
                self.recompute_outline();
                self.status = format!("reloaded {} slides", self.deck.slides.len());
            }
            KeyCode::Char('g') => {
                self.current = 0;
                self.text_scroll = 0;
                self.update_search_matches();
            }
            KeyCode::Char('G') => {
                self.current = self.deck.slides.len().saturating_sub(1);
                self.text_scroll = 0;
                self.update_search_matches();
            }
            KeyCode::Char('n') if !self.outline => {
                self.advance_search_match(1);
            }
            KeyCode::Char('N') if !self.outline => {
                self.advance_search_match(-1);
            }
            KeyCode::Enter
            | KeyCode::Right
            | KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Char('l')
            | KeyCode::Char(' ') => {
                if self.outline {
                    if let Some(index) = self.outline_filtered.get(self.outline_index).copied() {
                        self.current = index;
                        self.outline = false;
                        self.text_scroll = 0;
                        self.update_search_matches();
                    }
                } else if self.current + 1 < self.deck.slides.len() {
                    self.current += 1;
                    self.text_scroll = 0;
                    self.update_search_matches();
                }
            }
            KeyCode::Left
            | KeyCode::Up
            | KeyCode::Char('h')
            | KeyCode::Char('k')
            | KeyCode::Backspace => {
                if self.outline {
                    if self.outline_index > 0 {
                        self.outline_index -= 1;
                        self.ensure_outline_visible();
                    }
                } else if self.current > 0 {
                    self.current -= 1;
                    self.text_scroll = 0;
                    self.update_search_matches();
                }
            }
            KeyCode::Char('d')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.scroll_text((self.body_rows.max(2) / 2) as isize);
            }
            KeyCode::Char('u')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.scroll_text(-((self.body_rows.max(2) / 2) as isize));
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.search_focus = false;
                if self.search.is_empty() {
                    self.clear_search();
                }
            }
            KeyCode::Enter => self.search_focus = false,
            KeyCode::Backspace => {
                self.search.pop();
                self.update_search_matches();
            }
            KeyCode::Char(ch) => {
                self.search.push(ch);
                self.update_search_matches();
            }
            _ => {}
        }
        false
    }

    fn handle_outline_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.outline_search_focus = false,
            KeyCode::Backspace => {
                self.outline_query.pop();
                self.recompute_outline();
            }
            KeyCode::Char(ch) => {
                self.outline_query.push(ch);
                self.recompute_outline();
            }
            _ => {}
        }
        false
    }

    fn draw_images(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        if !self.image_backend.available() || !self.visibility.safe_to_draw_images() {
            self.clear_images(stdout)?;
            self.image_debug = format!("visible:{}", self.visibility.safe_to_draw_images());
            return Ok(());
        }
        let Some(text_rect) = self.text_rect else {
            self.clear_images(stdout)?;
            return Ok(());
        };
        let slide = self.current_slide().clone();
        let scroll = self.text_scroll;
        let layout = self.layout_for_current(text_rect.width).clone();
        let placements = placements_for_view(
            &slide,
            &layout.images,
            scroll,
            text_rect.y,
            text_rect.x,
            text_rect.height,
        );
        let diff = self.compositor.reconcile(placements);
        if !diff.retire.is_empty() {
            stdout.execute(crossterm::style::Print(
                self.image_backend.delete_sequence(&diff.retire),
            ))?;
        }
        if !diff.draw.is_empty() {
            stdout.execute(crossterm::style::Print(
                self.image_backend.draw_sequence(&diff.draw),
            ))?;
        }
        stdout.flush()?;
        self.image_debug = format!(
            "owned:{} tmux:{}",
            diff.draw.len(),
            self.visibility.safe_to_draw_images()
        );
        Ok(())
    }

    fn clear_images(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        let handles = self.compositor.clear();
        if !handles.is_empty() {
            stdout.execute(crossterm::style::Print(
                self.image_backend.delete_sequence(&handles),
            ))?;
            stdout.flush()?;
        }
        Ok(())
    }

    fn poll_visibility(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<bool> {
        if self.last_visibility_poll.elapsed() < Duration::from_millis(150) {
            return Ok(false);
        }
        self.last_visibility_poll = Instant::now();
        let next = self.tmux.poll_visibility().unwrap_or(self.visibility);
        let changed = next.safe_to_draw_images() != self.visibility.safe_to_draw_images();
        self.visibility = next;
        if changed && !self.visibility.safe_to_draw_images() {
            self.clear_images(stdout)?;
            self.status = "images cleared while tmux pane inactive".to_string();
        }
        Ok(changed)
    }

    fn layout_for_current(&mut self, width: u16) -> &SlideLayout {
        let key = (self.current, width);
        self.layout_cache.entry(key).or_insert_with(|| {
            build_layout(
                &self.deck.slides[self.current],
                Viewport {
                    width,
                    height: self.body_rows as u16,
                },
            )
        })
    }

    fn update_search_matches(&mut self) {
        self.search_matches.clear();
        self.search_match_index = 0;
        self.status.clear();

        if self.search.is_empty() {
            return;
        }

        let width = self.text_rect.map(|rect| rect.width).unwrap_or(80);
        let query = self.search.to_lowercase();
        let lines = self.layout_for_current(width).lines.clone();
        for line in &lines {
            let haystack = line.search_text.to_lowercase();
            let mut offset = 0usize;
            while let Some(position) = haystack[offset..].find(&query) {
                let start = offset + position;
                self.search_matches.push(SearchMatch {
                    row: line.row,
                    start,
                    len: query.chars().count(),
                });
                offset = start.saturating_add(1);
            }
        }

        if self.search_matches.is_empty() {
            self.status = format!("no matches for /{}", self.search);
            return;
        }

        self.focus_search_match();
    }

    fn advance_search_match(&mut self, delta: isize) {
        if self.search_matches.is_empty() {
            return;
        }

        let len = self.search_matches.len() as isize;
        let current = self.search_match_index as isize;
        self.search_match_index = (current + delta).rem_euclid(len) as usize;
        self.focus_search_match();
    }

    fn focus_search_match(&mut self) {
        if self.search_matches.is_empty() || self.body_rows == 0 {
            return;
        }

        let row = self.search_matches[self.search_match_index].row;
        if row < self.text_scroll {
            self.text_scroll = row;
        } else if row >= self.text_scroll + self.body_rows {
            self.text_scroll = row.saturating_sub(self.body_rows.saturating_sub(1));
        }
        self.status = format!(
            "match {}/{} for /{}",
            self.search_match_index + 1,
            self.search_matches.len(),
            self.search
        );
    }

    fn clear_search(&mut self) {
        self.search_focus = false;
        self.search.clear();
        self.search_matches.clear();
        self.search_match_index = 0;
        self.status.clear();
    }

    fn scroll_text(&mut self, delta: isize) {
        let total = self
            .layout_for_current(self.text_rect.map(|rect| rect.width).unwrap_or(80))
            .total_rows;
        let max_scroll = total.saturating_sub(self.body_rows);
        let next = if delta < 0 {
            self.text_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.text_scroll.saturating_add(delta as usize)
        };
        self.text_scroll = next.min(max_scroll);
    }

    fn scroll_status(&self) -> String {
        let total = self
            .layout_cache
            .get(&(
                self.current,
                self.text_rect.map(|rect| rect.width).unwrap_or(80),
            ))
            .map(|layout| layout.total_rows)
            .unwrap_or(0);
        if total <= self.body_rows {
            return String::new();
        }
        let below = total.saturating_sub(self.text_scroll + self.body_rows);
        format!("scroll:{} down:{}", self.text_scroll, below)
    }

    fn recompute_outline(&mut self) {
        let query = self.outline_query.trim().to_lowercase();
        self.outline_filtered = self
            .deck
            .slides
            .iter()
            .enumerate()
            .filter(|(_, slide)| {
                let text = slide.title.to_lowercase();
                let body = self
                    .layout_cache
                    .get(&(
                        slide.id,
                        self.text_rect.map(|rect| rect.width).unwrap_or(80),
                    ))
                    .map(|layout| layout.searchable_text.to_lowercase())
                    .unwrap_or_else(|| {
                        build_layout(
                            slide,
                            Viewport {
                                width: 80,
                                height: 24,
                            },
                        )
                        .searchable_text
                        .to_lowercase()
                    });
                query.is_empty()
                    || slide.name.to_lowercase().contains(&query)
                    || text.contains(&query)
                    || body.contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        if self.outline_index >= self.outline_filtered.len() {
            self.outline_index = self.outline_filtered.len().saturating_sub(1);
        }
        self.ensure_outline_visible();
    }

    fn ensure_outline_visible(&mut self) {
        if self.outline_index < self.outline_scroll {
            self.outline_scroll = self.outline_index;
        }
        let height = 12usize;
        if self.outline_index >= self.outline_scroll + height {
            self.outline_scroll = self.outline_index - height + 1;
        }
    }

    fn current_slide(&self) -> &Slide {
        &self.deck.slides[self.current]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::deck::model::{Block, DeckMetadata, ParagraphBlock, Slide};
    use crate::graphics::NoopBackend;

    use super::*;

    fn sample_deck() -> Deck {
        Deck {
            root: PathBuf::from("."),
            metadata: DeckMetadata {
                title: "deck".to_string(),
            },
            slides: vec![Slide {
                id: 0,
                title: "title".to_string(),
                name: "00.md".to_string(),
                blocks: vec![Block::Paragraph(ParagraphBlock {
                    id: 0,
                    content: vec![crate::deck::model::Inline::Text("hello world".to_string())],
                })],
                ..Slide::default()
            }],
        }
    }

    #[test]
    fn search_scroll_moves_to_match() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
        );
        app.body_rows = 1;
        app.text_rect = Some(Rect::new(0, 0, 20, 1));
        app.search = "world".to_string();
        app.update_search_matches();
        assert_eq!(app.text_scroll, 0);
    }

    #[test]
    fn search_navigation_cycles_matches() {
        let mut deck = sample_deck();
        deck.slides[0].blocks = vec![Block::Paragraph(ParagraphBlock {
            id: 0,
            content: vec![crate::deck::model::Inline::Text(
                "world hello world hello world".to_string(),
            )],
        })];
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
        );
        app.body_rows = 1;
        app.text_rect = Some(Rect::new(0, 0, 12, 1));
        app.search = "world".to_string();
        app.update_search_matches();

        assert_eq!(app.search_matches.len(), 3);
        assert_eq!(app.search_match_index, 0);

        app.advance_search_match(1);
        assert_eq!(app.search_match_index, 1);

        app.advance_search_match(1);
        app.advance_search_match(1);
        assert_eq!(app.search_match_index, 0);

        app.advance_search_match(-1);
        assert_eq!(app.search_match_index, 2);
    }

    #[test]
    fn clear_search_resets_search_state() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
        );
        app.search = "world".to_string();
        app.search_matches.push(SearchMatch {
            row: 0,
            start: 6,
            len: 5,
        });
        app.status = "match 1/1 for /world".to_string();

        app.clear_search();

        assert!(app.search.is_empty());
        assert!(app.search_matches.is_empty());
        assert!(app.status.is_empty());
    }
}
