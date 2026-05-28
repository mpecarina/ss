use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Terminal;

use crate::images::{build_placement_at, detect_backend, ImageBackend, ImagePlacement};
use crate::markdown::{render_markdown, ImageSlot, RenderedMarkdown};
use crate::slides::{load_slides, Slide};
use crate::tmux::TmuxContext;

pub fn run() -> Result<()> {
    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let slides = load_slides(&dir)?;
    let tmux = TmuxContext::detect();
    let backend = detect_backend(tmux.clone());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend_term = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_term)?;
    let result = App::new(dir, slides, tmux, backend).run(&mut terminal);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    result
}

struct App {
    dir: PathBuf,
    slides: Vec<Slide>,
    current: usize,
    status: String,
    help: bool,
    outline: bool,
    search_focus: bool,
    outline_search_focus: bool,
    search: String,
    outline_query: String,
    match_index: usize,
    matches: Vec<(usize, usize)>,
    text_scroll: usize,
    body_rows: usize,
    outline_filtered: Vec<usize>,
    outline_index: usize,
    outline_scroll: usize,
    tmux: TmuxContext,
    image_backend: Box<dyn ImageBackend>,
    images_visible: bool,
    image_rect: Option<Rect>,
    text_rect: Option<Rect>,
    inline_image_slots: Vec<ImageSlot>,
    image_debug: String,
    last_image_signature: String,
    last_image_ids: Vec<u32>,
    last_focus_poll: Instant,
    pending_g: bool,
}

impl App {
    fn new(dir: PathBuf, slides: Vec<Slide>, tmux: TmuxContext, image_backend: Box<dyn ImageBackend>) -> Self {
        let mut app = Self {
            dir,
            slides,
            current: 0,
            status: String::new(),
            help: false,
            outline: false,
            search_focus: false,
            outline_search_focus: false,
            search: String::new(),
            outline_query: String::new(),
            match_index: 0,
            matches: Vec::new(),
            text_scroll: 0,
            body_rows: 0,
            outline_filtered: Vec::new(),
            outline_index: 0,
            outline_scroll: 0,
            tmux,
            image_backend,
            images_visible: false,
            image_rect: None,
            text_rect: None,
            inline_image_slots: Vec::new(),
            image_debug: String::new(),
            last_image_signature: String::new(),
            last_image_ids: Vec::new(),
            last_focus_poll: Instant::now(),
            pending_g: false,
        };
        app.refresh_matches();
        app.recompute_outline();
        app
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        loop {
            self.poll_tmux_focus(terminal.backend_mut())?;
            terminal.draw(|frame| self.draw(frame))?;
            self.draw_images(terminal.backend_mut())?;
            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    if self.handle_key(key, terminal.backend_mut())? {
                        break;
                    }
                }
            }
        }

        self.clear_images(terminal.backend_mut())?;
        Ok(())
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
            KeyCode::Esc => {
                if self.help {
                    self.help = false;
                } else if self.outline {
                    self.outline = false;
                } else if !self.search.is_empty() {
                    self.search.clear();
                    self.refresh_matches();
                }
            }
            KeyCode::Char('?') => {
                self.help = !self.help;
                self.outline = false;
            }
            KeyCode::Char('o') => {
                self.outline = !self.outline;
                if self.outline {
                    self.recompute_outline();
                    self.outline_index = self.outline_filtered.iter().position(|idx| *idx == self.current).unwrap_or(0);
                }
            }
            KeyCode::Char('/') => {
                if self.outline {
                    self.outline_search_focus = true;
                } else {
                    self.search_focus = true;
                }
            }
            KeyCode::Char('n') => {
                self.jump_search(1);
            }
            KeyCode::Char('N') => {
                self.jump_search(-1);
            }
            KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                if self.outline {
                    self.outline_scroll = self.outline_scroll.saturating_add(self.outline_page_size() / 2);
                    self.ensure_outline_visible();
                } else {
                    self.scroll_text((self.body_rows.max(2) / 2) as isize);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                if self.outline {
                    self.outline_scroll = self.outline_scroll.saturating_sub(self.outline_page_size() / 2);
                    self.ensure_outline_visible();
                } else {
                    self.scroll_text(-((self.body_rows.max(2) / 2) as isize));
                }
            }
            KeyCode::Char('r') => {
                self.delete_last_images(stdout)?;
                self.reload_slides()?;
            }
            KeyCode::Char('g') => {
                if self.outline {
                    if self.pending_g {
                        self.outline_index = 0;
                        self.outline_scroll = 0;
                        self.pending_g = false;
                    } else {
                        self.pending_g = true;
                    }
                } else if self.pending_g {
                    self.delete_last_images(stdout)?;
                    self.current = 0;
                    self.pending_g = false;
                    self.text_scroll = 0;
                    self.refresh_matches();
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                self.pending_g = false;
                if self.outline {
                    if !self.outline_filtered.is_empty() {
                        self.outline_index = self.outline_filtered.len() - 1;
                    }
                } else {
                    self.delete_last_images(stdout)?;
                    self.current = self.slides.len().saturating_sub(1);
                    self.text_scroll = 0;
                    self.refresh_matches();
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('l') | KeyCode::Char(' ') => {
                self.pending_g = false;
                if self.outline {
                    if !self.outline_filtered.is_empty() {
                        self.delete_last_images(stdout)?;
                        self.current = self.outline_filtered[self.outline_index.min(self.outline_filtered.len() - 1)];
                        self.outline = false;
                        self.text_scroll = 0;
                        self.refresh_matches();
                    }
                } else if self.current + 1 < self.slides.len() {
                    self.delete_last_images(stdout)?;
                    self.current += 1;
                    self.text_scroll = 0;
                    self.refresh_matches();
                }
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') | KeyCode::Backspace => {
                self.pending_g = false;
                if self.outline {
                    if self.outline_index > 0 {
                        self.outline_index -= 1;
                        self.ensure_outline_visible();
                    }
                } else if self.current > 0 {
                    self.delete_last_images(stdout)?;
                    self.current -= 1;
                    self.text_scroll = 0;
                    self.refresh_matches();
                }
            }
            _ => {
                self.pending_g = false;
            }
        }

        Ok(false)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.search_focus = false;
            }
            KeyCode::Enter => {
                self.search_focus = false;
                self.match_index = 0;
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.refresh_matches();
            }
            KeyCode::Char(ch) => {
                self.search.push(ch);
                self.refresh_matches();
            }
            _ => {}
        }
        false
    }

    fn handle_outline_search_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.outline_search_focus = false;
            }
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

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        if self.help {
            self.image_rect = None;
            self.text_rect = None;
            self.draw_help(frame);
            return;
        }
        if self.outline {
            self.image_rect = None;
            self.text_rect = None;
            self.draw_outline(frame);
            return;
        }

        let size = frame.area();
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(2)])
            .split(size);

        let slide = &self.slides[self.current];
        let header = Paragraph::new(Line::from(vec![
            Span::styled(" ss ", Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(slide.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(slide.name.clone(), Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(header, vertical[0]);

        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100), Constraint::Percentage(0)])
            .split(vertical[1]);

        self.image_rect = None;
        self.text_rect = Some(body_chunks[0]);

        self.body_rows = body_chunks[0].height as usize;

        let rendered = self.highlighted_markdown(&slide.content);
        self.inline_image_slots = rendered.image_slots.clone();
        let viewport = self.viewport_lines(rendered.lines, self.body_rows);
        let body = Paragraph::new(viewport).wrap(Wrap { trim: false });
        frame.render_widget(body, body_chunks[0]);

        frame.render_widget(self.footer(slide), vertical[1]);
    }

    fn draw_outline(&mut self, frame: &mut ratatui::Frame) {
        let size = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
            .split(size);

        let query = if self.outline_search_focus {
            format!("/ {}", self.outline_query)
        } else {
            format!("  {}", self.outline_query)
        };
        frame.render_widget(Paragraph::new(query), chunks[0]);

        let mut lines = Vec::new();
        let visible = chunks[1].height as usize;
        let end = self.outline_filtered.len().min(self.outline_scroll + visible);
        for (visible_index, slide_index) in self.outline_filtered[self.outline_scroll..end].iter().enumerate() {
            let absolute = self.outline_scroll + visible_index;
            let slide = &self.slides[*slide_index];
            let index_style = if absolute == self.outline_index {
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let title_style = if absolute == self.outline_index {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {:>2} ", slide_index + 1), index_style),
                Span::raw(" "),
                Span::styled(slide.title.clone(), title_style),
                Span::raw("  "),
                Span::styled(slide.name.clone(), Style::default().fg(Color::DarkGray)),
            ]));
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled("no slides matched", Style::default().fg(Color::DarkGray))));
        }
        frame.render_widget(Paragraph::new(lines), chunks[1]);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{}/{} slides  / filter  enter open  j/k move  esc close", self.outline_filtered.len(), self.slides.len()),
                Style::default().fg(Color::DarkGray),
            )),
            chunks[2],
        );
    }

    fn draw_help(&self, frame: &mut ratatui::Frame) {
        let text = vec![
            Line::from("ss help"),
            Line::from(""),
            Line::from("Navigation: arrows, h j k l, g/G, r, q"),
            Line::from("Search: / current slide, n/N next prev hit"),
            Line::from("Outline: o, / filter, enter open"),
            Line::from("General: ? toggles help"),
            Line::from(""),
            Line::from("tmux-aware popup: images clear when pane/window loses focus"),
        ];
        frame.render_widget(Paragraph::new(text), frame.area());
    }

    fn footer(&self, slide: &Slide) -> Paragraph<'static> {
        let search_status = if self.search.is_empty() {
            String::new()
        } else if self.matches.is_empty() {
            format!("search:{} 0 hits", self.search)
        } else {
            format!("search:{} hit {}/{}", self.search, self.match_index + 1, self.matches.len())
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {}/{} ", self.current + 1, self.slides.len()),
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(format!("backend:{}", self.image_backend.name()), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(format!("images:{}", slide.images.len()), Style::default().fg(Color::Blue)),
            Span::raw("  "),
            Span::styled(search_status, Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(self.scroll_status(), Style::default().fg(Color::Magenta)),
            Span::raw("  "),
            Span::styled(self.image_debug.clone(), Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(self.status.clone(), Style::default().fg(Color::DarkGray)),
        ]);
        Paragraph::new(line)
    }

    fn highlighted_markdown(&self, text: &str) -> RenderedMarkdown {
        let rendered = render_markdown(text);
        if self.search.is_empty() || self.matches.is_empty() {
            return rendered;
        }

        let mut flat = String::new();
        let mut ranges = Vec::new();
        for line in &rendered.lines {
            let line_start = flat.len();
            for span in line.spans.iter() {
                flat.push_str(span.content.as_ref());
            }
            ranges.push((line_start, flat.len()));
            flat.push('\n');
        }

        let mut highlighted = Vec::new();
        for (line_index, line) in rendered.lines.into_iter().enumerate() {
            let (line_start, line_end) = ranges[line_index];
            let mut line_matches = Vec::new();
            for (index, (start, end)) in self.matches.iter().enumerate() {
                if *start < line_end && *end > line_start {
                    line_matches.push((index, start.saturating_sub(line_start), end.saturating_sub(line_start)));
                }
            }
            if line_matches.is_empty() {
                highlighted.push(line);
                continue;
            }

            highlighted.push(Line::from(highlight_line_spans(line.spans, line_matches, self.match_index)));
        }
        RenderedMarkdown {
            lines: highlighted,
            image_slots: rendered.image_slots,
        }
    }

    fn refresh_matches(&mut self) {
        self.matches = find_matches(&self.slides[self.current].content, &self.search);
        if self.matches.is_empty() {
            self.match_index = 0;
        } else if self.match_index >= self.matches.len() {
            self.match_index = 0;
        }
        self.ensure_current_match_visible();
    }

    fn jump_search(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let len = self.matches.len() as isize;
        let mut next = self.match_index as isize + delta;
        if next < 0 {
            next = len - 1;
        }
        if next >= len {
            next = 0;
        }
        self.match_index = next as usize;
        self.ensure_current_match_visible();
    }

    fn viewport_lines(&self, lines: Vec<Line<'static>>, body_rows: usize) -> Vec<Line<'static>> {
        if body_rows == 0 || lines.len() <= body_rows {
            return lines;
        }
        let max_scroll = lines.len().saturating_sub(body_rows);
        let scroll = self.text_scroll.min(max_scroll);
        lines.into_iter().skip(scroll).take(body_rows).collect()
    }

    fn scroll_text(&mut self, delta: isize) {
        if self.body_rows == 0 {
            return;
        }
        let total = self.highlighted_markdown(&self.slides[self.current].content).lines.len();
        let max_scroll = total.saturating_sub(self.body_rows);
        let next = if delta < 0 {
            self.text_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.text_scroll.saturating_add(delta as usize)
        };
        self.text_scroll = next.min(max_scroll);
    }

    fn scroll_status(&self) -> String {
        if self.body_rows == 0 {
            return String::new();
        }
        let total = self.highlighted_markdown(&self.slides[self.current].content).lines.len();
        if total <= self.body_rows {
            return String::new();
        }
        let below = total.saturating_sub(self.text_scroll + self.body_rows);
        format!("scroll:{} down:{}", self.text_scroll, below)
    }

    fn ensure_current_match_visible(&mut self) {
        if self.matches.is_empty() || self.body_rows == 0 {
            return;
        }
        let rendered = render_markdown(&self.slides[self.current].content);
        let mut flat = String::new();
        let mut line_ranges = Vec::new();
        for line in &rendered.lines {
            let start = flat.len();
            for span in line.spans.iter() {
                flat.push_str(span.content.as_ref());
            }
            line_ranges.push((start, flat.len()));
            flat.push('\n');
        }
        let current = self.matches[self.match_index];
        let mut line_index = 0usize;
        for (idx, (start, end)) in line_ranges.iter().enumerate() {
            if current.0 < *end && current.1 > *start {
                line_index = idx;
                break;
            }
        }
        if line_index < self.text_scroll {
            self.text_scroll = line_index;
        } else if line_index >= self.text_scroll + self.body_rows {
            self.text_scroll = line_index.saturating_sub(self.body_rows - 1);
        }
    }

    fn outline_page_size(&self) -> usize {
        12
    }

    fn recompute_outline(&mut self) {
        let query = self.outline_query.trim().to_lowercase();
        self.outline_filtered = self
            .slides
            .iter()
            .enumerate()
            .filter(|(_, slide)| {
                query.is_empty()
                    || slide.name.to_lowercase().contains(&query)
                    || slide.title.to_lowercase().contains(&query)
                    || slide.content.to_lowercase().contains(&query)
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

    fn reload_slides(&mut self) -> Result<()> {
        let current_path = self.slides.get(self.current).map(|slide| slide.path.clone());
        self.slides = load_slides(&self.dir)?;
        if let Some(path) = current_path {
            if let Some(index) = self.slides.iter().position(|slide| slide.path == path) {
                self.current = index;
            } else if self.current >= self.slides.len() {
                self.current = self.slides.len().saturating_sub(1);
            }
        }
        self.refresh_matches();
        self.recompute_outline();
        self.status = format!("reloaded {} slides", self.slides.len());
        Ok(())
    }

    fn draw_images(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        if !self.image_backend.available() || !self.tmux_visible() {
            if self.images_visible {
                self.delete_last_images(stdout)?;
            }
            self.image_debug = format!(
                "slots:{} backend:{} visible:{}",
                self.inline_image_slots.len(),
                self.image_backend.name(),
                self.tmux_visible()
            );
            return Ok(());
        }
        let first_image = self.slides[self.current]
            .images
            .first()
            .map(|image| image.path.clone())
            .unwrap_or_else(|| "none".to_string());
        let placements = {
            let slide = &self.slides[self.current];
            self.inline_image_placements(slide)
        };
        if placements.is_empty() {
            if self.images_visible {
                self.delete_last_images(stdout)?;
            }
            self.image_debug = format!(
                "slots:{} placements:0 first:{}",
                self.inline_image_slots.len(),
                first_image
            );
            return Ok(());
        }

        if let Some(first) = placements.first() {
            self.image_debug = format!(
                "slots:{} placements:{} r{} c{} {}x{}",
                self.inline_image_slots.len(),
                placements.len(),
                first.row,
                first.col,
                first.cols,
                first.rows
            );
        }

        let signature = image_signature(&placements);
        if self.images_visible && self.last_image_signature == signature {
            return Ok(());
        }

        if self.images_visible {
            self.delete_last_images(stdout)?;
        }

        stdout.execute(crossterm::style::Print(self.image_backend.draw_sequence(&placements)))?;
        stdout.flush()?;
        self.images_visible = true;
        self.last_image_signature = signature;
        self.last_image_ids = placements.iter().map(|placement| placement.image_id).collect();
        Ok(())
    }

    fn inline_image_placements(&self, slide: &Slide) -> Vec<ImagePlacement> {
        if self.inline_image_slots.is_empty() {
            return Vec::new();
        }

        let Some(text_rect) = self.text_rect else {
            return Vec::new();
        };

        let mut placements = Vec::new();
        let col = text_rect.x.saturating_add(1);
        let cols = text_rect.width.saturating_sub(1).max(10);
        for slot in &self.inline_image_slots {
            if slot.start_line < self.text_scroll {
                continue;
            }
            let Some(image) = slide.images.get(slot.image_index) else {
                continue;
            };
            let local_line = (slot.start_line - self.text_scroll) as u16;
            let row = text_rect.y.saturating_add(local_line).saturating_add(1);
            if let Some(mut placement) = build_placement_at(image, row, col, cols, slot.rows as u16) {
                placement.image_id = (slot.image_index + 1) as u32;
                placement.placement_id = (slot.image_index + 1) as u32;
                placements.push(placement);
            }
        }
        placements
    }

    fn poll_tmux_focus(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        if self.last_focus_poll.elapsed() < Duration::from_millis(200) {
            return Ok(());
        }
        self.last_focus_poll = Instant::now();
        if self.images_visible && !self.tmux_visible() {
            self.delete_last_images(stdout)?;
            self.status = "images hidden while pane inactive".to_string();
        }
        Ok(())
    }

    fn tmux_visible(&self) -> bool {
        self.tmux.poll_active().map(|active| active.visible()).unwrap_or(true)
    }

    fn clear_images(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        if self.images_visible {
            stdout.execute(crossterm::style::Print(self.image_backend.clear_sequence()))?;
            stdout.flush()?;
            self.images_visible = false;
        }
        self.last_image_signature.clear();
        self.last_image_ids.clear();
        Ok(())
    }

    fn delete_last_images(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        if !self.last_image_ids.is_empty() {
            stdout.execute(crossterm::style::Print(self.image_backend.delete_sequence(&self.last_image_ids)))?;
            stdout.flush()?;
        } else if self.images_visible {
            stdout.execute(crossterm::style::Print(self.image_backend.clear_sequence()))?;
            stdout.flush()?;
        }
        self.images_visible = false;
        self.last_image_signature.clear();
        self.last_image_ids.clear();
        Ok(())
    }
}

fn image_signature(placements: &[ImagePlacement]) -> String {
    let mut out = String::new();
    for placement in placements {
        out.push_str(&format!(
            "{}:{}:{}:{}:{}:{}|",
            placement.path,
            placement.row,
            placement.col,
            placement.cols,
            placement.rows,
            placement.image_id
        ));
    }
    out
}

fn highlight_line_spans(
    spans: Vec<Span<'static>>,
    matches: Vec<(usize, usize, usize)>,
    active_match_index: usize,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut global_offset = 0usize;

    for span in spans {
        let text = span.content.to_string();
        let span_start = global_offset;
        let span_end = global_offset + text.len();
        let relevant = matches
            .iter()
            .filter(|(_, start, end)| *start < span_end && *end > span_start)
            .cloned()
            .collect::<Vec<_>>();

        if relevant.is_empty() {
            out.push(span.clone());
            global_offset = span_end;
            continue;
        }

        let mut cursor = 0usize;
        for (match_id, start, end) in relevant {
            let local_start = start.saturating_sub(span_start).min(text.len());
            let local_end = end.saturating_sub(span_start).min(text.len());
            if local_start > cursor {
                out.push(Span::styled(text[cursor..local_start].to_string(), span.style));
            }
            let highlight_style = if match_id == active_match_index {
                span.style.fg(Color::Black).bg(Color::Yellow)
            } else {
                span.style.add_modifier(Modifier::REVERSED)
            };
            out.push(Span::styled(text[local_start..local_end].to_string(), highlight_style));
            cursor = local_end;
        }
        if cursor < text.len() {
            out.push(Span::styled(text[cursor..].to_string(), span.style));
        }
        global_offset = span_end;
    }

    out
}

fn find_matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < text.len() {
        let Some(found) = lower_text[start..].find(&lower_query) else {
            break;
        };
        let index = start + found;
        out.push((index, index + query.len()));
        start = index + query.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::NoopBackend;

    #[test]
    fn search_matches_case_insensitive() {
        let matches = find_matches("Tone ton TO", "to");
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn jump_search_wraps() {
        let mut app = App::new(PathBuf::from("."), vec![Slide::default()], TmuxContext::default(), Box::new(NoopBackend));
        app.matches = vec![(0, 2), (3, 5)];
        app.match_index = 1;
        app.jump_search(1);
        assert_eq!(app.match_index, 0);
    }

    #[test]
    fn viewport_lines_scrolls() {
        let app = App::new(PathBuf::from("."), vec![Slide::default()], TmuxContext::default(), Box::new(NoopBackend));
        let lines = vec![
            Line::from("one"),
            Line::from("two"),
            Line::from("three"),
        ];
        let mut app = app;
        app.text_scroll = 1;
        let view = app.viewport_lines(lines, 2);
        assert_eq!(view.len(), 2);
    }

    #[test]
    fn ensure_current_match_visible_moves_scroll() {
        let mut app = App::new(
            PathBuf::from("."),
            vec![Slide {
                content: "line1\nline2\nline3\nline4\nline5".to_string(),
                ..Slide::default()
            }],
            TmuxContext::default(),
            Box::new(NoopBackend),
        );
        app.search = "line4".to_string();
        app.body_rows = 2;
        app.refresh_matches();
        assert!(app.text_scroll > 0);
    }

    #[test]
    fn highlight_line_spans_preserves_non_match_segments() {
        let spans = vec![
            Span::styled("abc".to_string(), Style::default().fg(Color::Red)),
            Span::styled("def".to_string(), Style::default().fg(Color::Blue)),
        ];
        let highlighted = highlight_line_spans(spans, vec![(0, 2, 4)], 0);
        assert!(highlighted.len() >= 3);
        assert_eq!(highlighted[0].content, "ab");
    }
}
