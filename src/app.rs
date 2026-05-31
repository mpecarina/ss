use std::collections::HashMap;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::window_size;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::deck::{Deck, Slide, load_deck};
use crate::graphics::{ImageBackend, ImageCompositor, detect_backend, placements_for_view};
use crate::layout::{SearchMatch, SlideLayout, Viewport, build_layout, viewport_lines};
use crate::tmux::{TmuxRuntime, VisibilityState};

const LINE_GUTTER_WIDTH: u16 = 2;

pub fn run() -> Result<()> {
    let mut watch = false;
    let mut dir = None;
    for arg in std::env::args().skip(1) {
        if arg == "--watch" || arg == "-w" {
            watch = true;
        } else if dir.is_none() {
            dir = Some(PathBuf::from(arg));
        }
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    let deck = load_deck(&dir)?;
    let tmux = TmuxRuntime::detect();
    let backend = detect_backend(&tmux);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide, EnableBracketedPaste)?;
    let keyboard_enhancement = matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
    if keyboard_enhancement {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )
        .ok();
    }
    let backend_term = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend_term)?;
    let result = App::new(dir, deck, tmux, backend, watch).run(&mut terminal);
    disable_raw_mode().ok();
    if keyboard_enhancement {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags).ok();
    }
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        Show,
        LeaveAlternateScreen
    )
    .ok();
    result
}

struct App {
    dir: PathBuf,
    deck: Deck,
    watch: bool,
    watched_paths: Vec<PathBuf>,
    watched_mtime: Option<SystemTime>,
    current: usize,
    status: String,
    help: bool,
    outline: bool,
    search_focus: bool,
    outline_search_focus: bool,
    command_focus: bool,
    search: String,
    search_matches: Vec<SearchMatch>,
    search_match_index: usize,
    outline_query: String,
    command: String,
    text_scroll: usize,
    line_cursor: usize,
    body_rows: usize,
    text_rect: Option<Rect>,
    layout_cache: HashMap<(usize, u16, u16, u16), SlideLayout>,
    outline_filtered: Vec<usize>,
    outline_index: usize,
    outline_scroll: usize,
    visual_anchor: Option<usize>,
    visual_cursor: usize,
    tmux: TmuxRuntime,
    visibility: VisibilityState,
    image_backend: Box<dyn ImageBackend>,
    compositor: ImageCompositor,
    image_debug: String,
    last_visibility_poll: Instant,
    escape_sequence: EscapeSequence,
    csi_buffer: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EscapeSequence {
    #[default]
    None,
    Esc,
    Csi,
}

impl App {
    fn new(
        dir: PathBuf,
        deck: Deck,
        tmux: TmuxRuntime,
        image_backend: Box<dyn ImageBackend>,
        watch: bool,
    ) -> Self {
        let mut app = Self {
            dir,
            deck,
            watch,
            watched_paths: Vec::new(),
            watched_mtime: None,
            current: 0,
            status: String::new(),
            help: false,
            outline: false,
            search_focus: false,
            outline_search_focus: false,
            command_focus: false,
            search: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
            outline_query: String::new(),
            command: String::new(),
            text_scroll: 0,
            line_cursor: 0,
            body_rows: 0,
            text_rect: None,
            layout_cache: HashMap::new(),
            outline_filtered: Vec::new(),
            outline_index: 0,
            outline_scroll: 0,
            visual_anchor: None,
            visual_cursor: 0,
            visibility: VisibilityState::default(),
            compositor: ImageCompositor::new(tmux.runtime_id.clone()),
            tmux,
            image_backend,
            image_debug: String::new(),
            last_visibility_poll: Instant::now(),
            escape_sequence: EscapeSequence::None,
            csi_buffer: String::new(),
        };
        app.refresh_watch_state();
        app.recompute_outline();
        app
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        let mut dirty = true;
        loop {
            if self.poll_visibility(terminal.backend_mut())? {
                dirty = true;
            }
            if self.poll_reload(terminal.backend_mut())? {
                dirty = true;
            }
            if dirty {
                terminal.draw(|frame| self.draw(frame))?;
                self.draw_images(terminal.backend_mut())?;
                dirty = false;
            }
            if event::poll(Duration::from_millis(80))? {
                match event::read()? {
                    Event::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        if self.handle_key(key, terminal.backend_mut())? {
                            break;
                        }
                        dirty = true;
                    }
                    Event::Paste(data) => {
                        self.handle_paste(data);
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
        self.draw_present(frame);
        if self.outline {
            self.draw_outline(frame);
        }
        if self.help {
            self.draw_help(frame);
        }
    }

    fn draw_present(&mut self, frame: &mut ratatui::Frame) {
        let size = frame.area();
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(size);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(8)])
            .split(vertical[0]);
        frame.render_widget(
            Paragraph::new(self.deck.metadata.title.clone())
                .style(Style::default().fg(Color::DarkGray)),
            top[0],
        );
        frame.render_widget(
            Paragraph::new(format!("{}/{}", self.current + 1, self.deck.slides.len()))
                .style(Style::default().fg(Color::Gray))
                .alignment(Alignment::Right),
            top[1],
        );

        let stage = self.presentation_stage(vertical[1]);
        self.text_rect = Some(stage);
        self.body_rows = stage.height as usize;
        let scroll = self.text_scroll;
        let body_rows = self.body_rows;
        let matches = self.search_matches.clone();
        let selected_match = (!matches.is_empty()).then_some(self.search_match_index);
        let active_row = self.active_row();
        let selection = self.visual_selection_range();
        let layout = self.layout_for_current(content_width(stage.width));
        let viewport = viewport_lines(
            layout,
            scroll,
            body_rows,
            &matches,
            selected_match,
            active_row,
            selection,
        );
        frame.render_widget(
            Paragraph::new(viewport)
                .wrap(Wrap { trim: false })
                .alignment(Alignment::Left),
            stage,
        );

        let mut bottom = String::new();
        let mode_status = self.operational_status();
        if !mode_status.is_empty() {
            if !bottom.is_empty() {
                bottom.push_str("  ");
            }
            bottom.push_str(&mode_status);
        }
        if diagnostics_enabled() {
            let scroll_status = self.scroll_status();
            if !scroll_status.is_empty() {
                if !bottom.is_empty() {
                    bottom.push_str("  ");
                }
                bottom.push_str(&scroll_status);
            }
            if !self.status.is_empty() {
                if !bottom.is_empty() {
                    bottom.push_str("  ");
                }
                bottom.push_str(&self.status);
            }
        } else if !self.status.is_empty() {
            if !bottom.is_empty() {
                bottom.push_str("  ");
            }
            bottom.push_str(&self.status);
        }
        frame.render_widget(
            Paragraph::new(bottom)
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            vertical[2],
        );
    }

    fn draw_help(&self, frame: &mut ratatui::Frame) {
        let area = centered_rect(
            frame.area(),
            frame.area().width.min(84),
            frame.area().height.min(10),
        );
        let block = Block::default().borders(Borders::ALL).title(" help ");
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("ss help"),
                Line::from(""),
                Line::from("Navigation: arrows, h j k l, g/G, [ ], ctrl-u, ctrl-d, r, q"),
                Line::from("Links: move to a linked line and press enter to open it"),
                Line::from("Search: / current slide, n/N next or previous hit, [ ] heading jumps"),
                Line::from("Outline: o, / filter, enter open"),
                Line::from("Visuals: presentation mode stays active during search and overlays"),
                Line::from("Graphics: explicit image ownership and tmux visibility gating"),
            ]),
            inner,
        );
    }

    fn draw_outline(&mut self, frame: &mut ratatui::Frame) {
        let popup = centered_rect(
            frame.area(),
            frame.area().width.saturating_sub(12).min(88),
            frame.area().height.saturating_sub(6).min(24),
        );
        let block = Block::default().borders(Borders::ALL).title(" outline ");
        let inner = block.inner(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
        let size = inner;
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

    fn operational_status(&self) -> String {
        if self.search_focus {
            format!("/{}", self.search)
        } else if self.outline_search_focus {
            format!("/{}", self.outline_query)
        } else if self.command_focus {
            format!(":{}", self.command)
        } else {
            String::new()
        }
    }

    fn presentation_stage(&self, area: Rect) -> Rect {
        let layout = self
            .current_slide()
            .frontmatter
            .layout
            .as_deref()
            .unwrap_or_default();
        let width = match layout {
            "image" | "hero" => area.width.saturating_sub(4),
            _ if area.width > 120 => 96,
            _ if area.width > 90 => area.width.saturating_sub(10),
            _ => area.width.saturating_sub(4),
        }
        .max(10)
        .min(area.width);
        let height = match layout {
            "image" | "hero" => area.height.saturating_sub(1),
            _ => area.height.saturating_sub(2),
        }
        .max(3)
        .min(area.height);
        centered_rect(area, width, height)
    }

    fn handle_key(&mut self, key: KeyEvent, stdout: &mut CrosstermBackend<Stdout>) -> Result<bool> {
        let Some(key) = self.normalize_key_event(key) else {
            return Ok(false);
        };
        if key.code == KeyCode::Char('c')
            && key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            return Ok(self.handle_ctrl_c());
        }
        if self.help || self.search_focus || self.outline_search_focus || self.command_focus {
            match key.code {
                KeyCode::Left => {
                    self.help = false;
                    self.search_focus = false;
                    self.outline_search_focus = false;
                    self.command_focus = false;
                    if self.visual_active() {
                        self.move_visual_cursor(-1);
                    } else {
                        self.previous_slide();
                    }
                    return Ok(false);
                }
                KeyCode::Right => {
                    self.help = false;
                    self.search_focus = false;
                    self.outline_search_focus = false;
                    self.command_focus = false;
                    if self.visual_active() {
                        self.move_visual_cursor(1);
                    } else {
                        self.next_slide();
                    }
                    return Ok(false);
                }
                _ => {}
            }
        }
        if self.search_focus {
            return Ok(self.handle_search_key(key));
        }
        if self.outline_search_focus {
            return Ok(self.handle_outline_search_key(key));
        }
        if self.command_focus {
            return self.handle_command_key(key, stdout);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('?') => {
                self.help = !self.help;
                self.outline = false;
            }
            KeyCode::Char('V') => {
                self.toggle_visual_mode();
            }
            KeyCode::Esc => {
                if self.help {
                    self.help = false;
                } else if self.outline {
                    self.outline = false;
                } else if !self.search.is_empty() {
                    self.clear_search();
                } else if self.visual_active() {
                    self.clear_visual_mode();
                } else {
                    return Ok(true);
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
            KeyCode::Char(':') => {
                self.command_focus = true;
                self.command.clear();
            }
            KeyCode::Char('r') => {
                self.reload_current_path(stdout)?;
            }
            KeyCode::Char('g') => {
                if self.visual_active() {
                    self.set_visual_cursor(0);
                } else {
                    self.jump_to_slide_top();
                }
            }
            KeyCode::Char('G') => {
                if self.visual_active() {
                    let last = self.current_layout_rows().saturating_sub(1);
                    self.set_visual_cursor(last);
                } else {
                    self.jump_to_slide_bottom();
                }
            }
            KeyCode::Char('n') if !self.outline => {
                self.advance_search_match(1);
            }
            KeyCode::Char('N') if !self.outline => {
                self.advance_search_match(-1);
            }
            KeyCode::Char('[') => {
                self.jump_heading(-1);
            }
            KeyCode::Char(']') => {
                self.jump_heading(1);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.outline {
                    if let Some(index) = self.outline_filtered.get(self.outline_index).copied() {
                        self.current = index;
                        self.outline = false;
                        self.reset_after_slide_change();
                    }
                } else if self.visual_active() {
                    self.move_visual_cursor(1);
                } else {
                    self.next_slide();
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
                if self.outline {
                    if self.outline_index > 0 {
                        self.outline_index -= 1;
                        self.ensure_outline_visible();
                    }
                } else if self.visual_active() {
                    self.move_visual_cursor(-1);
                } else {
                    self.previous_slide();
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if self.outline {
                    if let Some(index) = self.outline_filtered.get(self.outline_index).copied() {
                        self.current = index;
                        self.outline = false;
                        self.reset_after_slide_change();
                    }
                } else if key.code == KeyCode::Enter && self.follow_active_link()? {
                } else {
                    self.next_slide();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.outline {
                    if self.outline_index > 0 {
                        self.outline_index -= 1;
                        self.ensure_outline_visible();
                    }
                } else if self.visual_active() {
                    self.move_visual_cursor(-1);
                } else {
                    self.move_line_cursor(-1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.outline {
                    if self.outline_index + 1 < self.outline_filtered.len() {
                        self.outline_index += 1;
                        self.ensure_outline_visible();
                    }
                } else if self.visual_active() {
                    self.move_visual_cursor(1);
                } else {
                    self.move_line_cursor(1);
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

    fn handle_ctrl_c(&mut self) -> bool {
        if self.command_focus {
            self.command_focus = false;
            self.command.clear();
            return false;
        }
        if self.search_focus {
            self.search_focus = false;
            if self.search.is_empty() {
                self.clear_search();
            }
            return false;
        }
        if self.outline_search_focus {
            self.outline_search_focus = false;
            return false;
        }
        if self.help {
            self.help = false;
            return false;
        }
        if self.outline {
            self.outline = false;
            return false;
        }
        if !self.search.is_empty() {
            self.clear_search();
            return false;
        }
        if self.visual_active() {
            self.clear_visual_mode();
            return false;
        }
        true
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

    fn handle_command_key(
        &mut self,
        key: KeyEvent,
        stdout: &mut CrosstermBackend<Stdout>,
    ) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.command_focus = false;
                self.command.clear();
            }
            KeyCode::Enter => {
                self.command_focus = false;
                let command = self.command.trim().to_string();
                self.command.clear();
                if !command.is_empty() {
                    return self.execute_command(&command, stdout);
                }
            }
            KeyCode::Backspace => {
                self.command.pop();
            }
            KeyCode::Char(ch) => {
                self.command.push(ch);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_paste(&mut self, data: String) {
        if self.command_focus {
            self.command.push_str(&data);
            return;
        }
        if self.search_focus {
            self.search.push_str(&data);
            self.update_search_matches();
            return;
        }
        if self.outline_search_focus {
            self.outline_query.push_str(&data);
            self.recompute_outline();
        }
    }

    fn execute_command(
        &mut self,
        command: &str,
        stdout: &mut CrosstermBackend<Stdout>,
    ) -> Result<bool> {
        let trimmed = command.trim();
        if trimmed == "q" || trimmed == "quit" {
            return Ok(true);
        }

        if trimmed == "r" || trimmed == "reload" {
            self.reload_current_path(stdout)?;
            return Ok(false);
        }

        if trimmed == "p" || trimmed == "path" {
            self.status = self.dir.display().to_string();
            return Ok(false);
        }

        if let Some(path) = trimmed
            .strip_prefix("open ")
            .or_else(|| trimmed.strip_prefix("o "))
            .or_else(|| trimmed.strip_prefix("e "))
        {
            self.open_path(path.trim(), stdout)?;
            return Ok(false);
        }

        self.status = format!("unknown command: :{}", trimmed);
        Ok(false)
    }

    fn reload_current_path(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        self.clear_images(stdout)?;
        self.deck = load_deck(&self.dir)?;
        self.layout_cache.clear();
        self.current = self.current.min(self.deck.slides.len().saturating_sub(1));
        self.update_search_matches();
        self.recompute_outline();
        self.refresh_watch_state();
        self.status = format!("reloaded {} slides", self.deck.slides.len());
        Ok(())
    }

    fn open_path(&mut self, path: &str, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        let target = PathBuf::from(path);
        let resolved = if target.is_absolute() {
            target
        } else {
            self.dir.join(target)
        };

        self.clear_images(stdout)?;
        self.dir = resolved;
        self.deck = load_deck(&self.dir)?;
        self.layout_cache.clear();
        self.current = 0;
        self.text_scroll = 0;
        self.line_cursor = 0;
        self.visual_anchor = None;
        self.visual_cursor = 0;
        self.search.clear();
        self.search_matches.clear();
        self.search_match_index = 0;
        self.recompute_outline();
        self.refresh_watch_state();
        self.status = format!("opened {}", self.dir.display());
        Ok(())
    }

    fn poll_reload(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<bool> {
        if !self.watch {
            return Ok(false);
        }

        let latest = latest_mtime(&self.watched_paths);
        match (self.watched_mtime, latest) {
            (Some(previous), Some(current)) if current > previous => {
                self.reload_current_path(stdout)?;
                self.status = format!("auto-reloaded {} slides", self.deck.slides.len());
                Ok(true)
            }
            (None, Some(current)) => {
                self.watched_mtime = Some(current);
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn refresh_watch_state(&mut self) {
        self.watched_paths = watched_paths(&self.dir, &self.deck);
        self.watched_mtime = latest_mtime(&self.watched_paths);
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
        let layout = self
            .layout_for_current(content_width(text_rect.width))
            .clone();
        let placements = placements_for_view(
            &slide,
            &layout.images,
            scroll,
            text_rect.y,
            text_rect.x,
            text_rect.width,
            text_rect.height,
            self.visibility.pane_top,
            self.visibility.pane_left,
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
            if diagnostics_enabled() {
                self.status = "images cleared while tmux pane inactive".to_string();
            }
        }
        Ok(changed)
    }

    fn layout_for_current(&mut self, width: u16) -> &SlideLayout {
        let (cell_width_px, cell_height_px) = terminal_cell_size();
        let key = (self.current, width, cell_width_px, cell_height_px);
        self.layout_cache.entry(key).or_insert_with(|| {
            build_layout(
                &self.deck.slides[self.current],
                Viewport {
                    width,
                    height: self.body_rows as u16,
                    cell_width_px,
                    cell_height_px,
                    unicode_placeholders: self.image_backend.available(),
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

        let width = self
            .text_rect
            .map(|rect| content_width(rect.width))
            .unwrap_or(80);
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
        self.line_cursor = row;
        if self.visual_active() {
            self.visual_cursor = row;
        }
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
            .layout_for_current(
                self.text_rect
                    .map(|rect| content_width(rect.width))
                    .unwrap_or(80),
            )
            .total_rows;
        let max_scroll = total.saturating_sub(self.body_rows);
        let next = if delta < 0 {
            self.text_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.text_scroll.saturating_add(delta as usize)
        };
        self.text_scroll = next.min(max_scroll);
        if self.visual_active() {
            self.move_visual_cursor(delta);
        } else {
            self.move_line_cursor(delta);
        }
    }

    fn jump_heading(&mut self, delta: isize) {
        self.jump_to_matching_heading(delta, |level| level.is_some());
    }

    fn jump_to_matching_heading(&mut self, delta: isize, predicate: impl Fn(Option<u8>) -> bool) {
        let current_row = if self.visual_active() {
            self.visual_cursor
        } else {
            self.line_cursor
        };
        let width = self
            .text_rect
            .map(|rect| content_width(rect.width))
            .unwrap_or(80);
        let matches = self
            .layout_for_current(width)
            .lines
            .iter()
            .filter(|line| predicate(line.heading_level))
            .map(|line| line.row)
            .collect::<Vec<_>>();

        if matches.is_empty() {
            return;
        }

        let target = if delta < 0 {
            matches.iter().rev().copied().find(|row| *row < current_row)
        } else {
            matches.iter().copied().find(|row| *row > current_row)
        };

        if let Some(row) = target {
            self.line_cursor = row;
            self.ensure_line_cursor_visible();
            if self.visual_active() {
                self.visual_cursor = row;
                self.ensure_visual_cursor_visible();
            }
        }
    }

    fn scroll_status(&self) -> String {
        let (cell_width_px, cell_height_px) = terminal_cell_size();
        let total = self
            .layout_cache
            .get(&(
                self.current,
                self.text_rect
                    .map(|rect| content_width(rect.width))
                    .unwrap_or(80),
                cell_width_px,
                cell_height_px,
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
        let (cell_width_px, cell_height_px) = terminal_cell_size();
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
                        self.text_rect
                            .map(|rect| content_width(rect.width))
                            .unwrap_or(80),
                        cell_width_px,
                        cell_height_px,
                    ))
                    .map(|layout| layout.searchable_text.to_lowercase())
                    .unwrap_or_else(|| {
                        build_layout(
                            slide,
                            Viewport {
                                width: 80,
                                height: 24,
                                cell_width_px,
                                cell_height_px,
                                unicode_placeholders: self.image_backend.available(),
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

    fn follow_active_link(&mut self) -> Result<bool> {
        let Some((url, link_count)) = self
            .active_row()
            .and_then(|row| self.first_link_on_row(row))
        else {
            return Ok(false);
        };
        self.open_link(&url, link_count)?;
        Ok(true)
    }

    fn first_link_on_row(&mut self, row: usize) -> Option<(String, usize)> {
        let width = self
            .text_rect
            .map(|rect| content_width(rect.width))
            .unwrap_or(80);
        self.layout_for_current(width)
            .lines
            .iter()
            .find(|line| line.row == row)
            .and_then(|line| {
                line.link_urls
                    .first()
                    .map(|url| (url.clone(), line.link_urls.len()))
            })
    }

    fn open_link(&mut self, url: &str, link_count: usize) -> Result<()> {
        let slide_path = self.current_slide().path.clone();
        let target = resolve_link_target(&slide_path, url);
        open_link_target(&target)?;
        self.status = if link_count > 1 {
            format!(
                "opened {} (first of {} links on line)",
                target.display(),
                link_count
            )
        } else {
            format!("opened {}", target.display())
        };
        Ok(())
    }

    fn visual_active(&self) -> bool {
        self.visual_anchor.is_some()
    }

    fn toggle_visual_mode(&mut self) {
        if self.visual_active() {
            self.clear_visual_mode();
            return;
        }
        self.visual_anchor = Some(self.line_cursor);
        self.visual_cursor = self.line_cursor;
        self.status = "visual".to_string();
    }

    fn clear_visual_mode(&mut self) {
        self.visual_anchor = None;
        if self.status == "visual" {
            self.status.clear();
        }
    }

    fn visual_selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.visual_anchor?;
        Some((
            anchor.min(self.visual_cursor),
            anchor.max(self.visual_cursor),
        ))
    }

    fn active_row(&self) -> Option<usize> {
        if self.visual_active() {
            Some(self.visual_cursor)
        } else {
            Some(self.line_cursor)
        }
    }

    fn current_layout_rows(&mut self) -> usize {
        self.layout_for_current(
            self.text_rect
                .map(|rect| content_width(rect.width))
                .unwrap_or(80),
        )
        .total_rows
    }

    fn jump_to_slide_top(&mut self) {
        self.line_cursor = 0;
        self.text_scroll = 0;
    }

    fn jump_to_slide_bottom(&mut self) {
        let last_row = self.current_layout_rows().saturating_sub(1);
        self.line_cursor = last_row;
        self.ensure_line_cursor_visible();
    }

    fn move_line_cursor(&mut self, delta: isize) {
        let max_row = self.current_layout_rows().saturating_sub(1) as isize;
        let next = self.next_cursor_row(self.line_cursor, delta, max_row);
        self.line_cursor = next;
        self.ensure_line_cursor_visible();
    }

    fn set_visual_cursor(&mut self, row: usize) {
        let max_row = self.current_layout_rows().saturating_sub(1);
        self.visual_cursor = row.min(max_row);
        self.ensure_visual_cursor_visible();
    }

    fn move_visual_cursor(&mut self, delta: isize) {
        let max_row = self.current_layout_rows().saturating_sub(1) as isize;
        let next = self.next_cursor_row(self.visual_cursor, delta, max_row);
        self.visual_cursor = next;
        self.ensure_visual_cursor_visible();
    }

    fn next_cursor_row(&mut self, current: usize, delta: isize, max_row: isize) -> usize {
        let next = (current as isize + delta).clamp(0, max_row.max(0)) as usize;
        if delta == 0 {
            return next;
        }

        let direction = delta.signum();
        let width = self
            .text_rect
            .map(|rect| content_width(rect.width))
            .unwrap_or(80);
        let layout = self.layout_for_current(width);
        let mut row = next as isize;

        while row >= 0 && row <= max_row {
            let is_blank = layout
                .lines
                .iter()
                .find(|line| line.row == row as usize)
                .map(|line| line.search_text.trim().is_empty())
                .unwrap_or(true);
            if !is_blank {
                return row as usize;
            }
            row += direction;
        }

        next
    }

    fn ensure_line_cursor_visible(&mut self) {
        if self.body_rows == 0 {
            return;
        }
        if self.line_cursor < self.text_scroll {
            self.text_scroll = self.line_cursor;
        } else if self.line_cursor >= self.text_scroll + self.body_rows {
            self.text_scroll = self
                .line_cursor
                .saturating_sub(self.body_rows.saturating_sub(1));
        }
    }

    fn ensure_visual_cursor_visible(&mut self) {
        if self.body_rows == 0 {
            return;
        }
        if self.visual_cursor < self.text_scroll {
            self.text_scroll = self.visual_cursor;
        } else if self.visual_cursor >= self.text_scroll + self.body_rows {
            self.text_scroll = self
                .visual_cursor
                .saturating_sub(self.body_rows.saturating_sub(1));
        }
    }

    fn normalize_key_event(&mut self, key: KeyEvent) -> Option<KeyEvent> {
        match key.code {
            KeyCode::Esc => {
                self.escape_sequence = EscapeSequence::Esc;
                self.csi_buffer.clear();
                Some(key)
            }
            KeyCode::Char('[') | KeyCode::Char('O')
                if self.escape_sequence == EscapeSequence::Esc =>
            {
                self.escape_sequence = EscapeSequence::Csi;
                self.csi_buffer.clear();
                None
            }
            KeyCode::Char(ch)
                if self.escape_sequence == EscapeSequence::Csi
                    && (ch.is_ascii_digit() || ch == ';') =>
            {
                self.csi_buffer.push(ch);
                None
            }
            KeyCode::Char(ch) if self.escape_sequence == EscapeSequence::Csi => {
                self.escape_sequence = EscapeSequence::None;
                self.csi_buffer.clear();
                let code = match ch.to_ascii_uppercase() {
                    'A' => KeyCode::Up,
                    'B' => KeyCode::Down,
                    'C' => KeyCode::Right,
                    'D' => KeyCode::Left,
                    _ => KeyCode::Char(ch),
                };
                Some(KeyEvent { code, ..key })
            }
            KeyCode::Char('\u{8}') | KeyCode::Char('\u{7f}') => {
                self.escape_sequence = EscapeSequence::None;
                self.csi_buffer.clear();
                Some(KeyEvent {
                    code: KeyCode::Backspace,
                    ..key
                })
            }
            _ => {
                self.escape_sequence = EscapeSequence::None;
                self.csi_buffer.clear();
                Some(key)
            }
        }
    }

    fn next_slide(&mut self) {
        if self.current + 1 < self.deck.slides.len() {
            self.current += 1;
            self.clear_visual_mode();
            self.reset_after_slide_change();
        }
    }

    fn previous_slide(&mut self) {
        if self.current > 0 {
            self.current -= 1;
            self.clear_visual_mode();
            self.reset_after_slide_change();
        }
    }

    fn reset_after_slide_change(&mut self) {
        self.text_scroll = 0;
        self.line_cursor = 0;
        self.update_search_matches();
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn content_width(width: u16) -> u16 {
    width.saturating_sub(LINE_GUTTER_WIDTH)
}

fn terminal_cell_size() -> (u16, u16) {
    let Ok(window) = window_size() else {
        return (0, 0);
    };
    if window.columns == 0 || window.rows == 0 || window.width == 0 || window.height == 0 {
        return (0, 0);
    }
    (window.width / window.columns, window.height / window.rows)
}

fn watched_paths(dir: &Path, deck: &Deck) -> Vec<PathBuf> {
    if deck.slides.len() == 1 && dir.is_file() {
        return vec![deck.slides[0].path.clone()];
    }

    let mut paths = deck
        .slides
        .iter()
        .map(|slide| slide.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn latest_mtime(paths: &[PathBuf]) -> Option<SystemTime> {
    paths.iter().filter_map(path_mtime).max()
}

fn path_mtime(path: &PathBuf) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LinkTarget {
    External(String),
    File(PathBuf),
}

impl LinkTarget {
    fn display(&self) -> String {
        match self {
            Self::External(url) => url.clone(),
            Self::File(path) => path.display().to_string(),
        }
    }
}

fn resolve_link_target(slide_path: &Path, url: &str) -> LinkTarget {
    if is_external_link(url) {
        return LinkTarget::External(url.to_string());
    }

    let relative = url.split('#').next().unwrap_or(url);
    let base = slide_path.parent().unwrap_or_else(|| Path::new("."));
    let path = if relative.is_empty() {
        slide_path.to_path_buf()
    } else {
        let raw = Path::new(relative);
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            base.join(raw)
        }
    };

    LinkTarget::File(std::fs::canonicalize(&path).unwrap_or(path))
}

fn is_external_link(url: &str) -> bool {
    url.contains("://")
        || url.starts_with("mailto:")
        || url.starts_with("file:")
        || url.starts_with("www.")
}

fn open_link_target(target: &LinkTarget) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        match target {
            LinkTarget::External(url) => {
                cmd.arg(url);
            }
            LinkTarget::File(path) => {
                cmd.arg(path);
            }
        }
        cmd.spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg("start").arg("");
        match target {
            LinkTarget::External(url) => {
                cmd.arg(url);
            }
            LinkTarget::File(path) => {
                cmd.arg(path);
            }
        }
        cmd.spawn()?;
        return Ok(());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut cmd = Command::new("xdg-open");
        match target {
            LinkTarget::External(url) => {
                cmd.arg(url);
            }
            LinkTarget::File(path) => {
                cmd.arg(path);
            }
        }
        cmd.spawn()?;
        return Ok(());
    }
}

fn diagnostics_enabled() -> bool {
    match std::env::var("SS_DIAGNOSTICS") {
        Ok(value) => matches!(
            value.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::deck::model::{Block, DeckMetadata, HeadingBlock, Inline, ParagraphBlock, Slide};
    use crate::graphics::NoopBackend;
    use tempfile::tempdir;

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
            false,
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
            false,
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
            false,
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

    #[test]
    fn previous_slide_moves_back_and_resets_scroll() {
        let mut deck = sample_deck();
        deck.slides.push(Slide {
            id: 1,
            title: "second".to_string(),
            name: "01.md".to_string(),
            ..Slide::default()
        });
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.current = 1;
        app.text_scroll = 7;

        app.previous_slide();

        assert_eq!(app.current, 0);
        assert_eq!(app.text_scroll, 0);
    }

    #[test]
    fn previous_slide_stops_at_first_slide() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.text_scroll = 3;

        app.previous_slide();

        assert_eq!(app.current, 0);
        assert_eq!(app.text_scroll, 3);
    }

    #[test]
    fn next_slide_moves_forward_and_resets_scroll() {
        let mut deck = sample_deck();
        deck.slides.push(Slide {
            id: 1,
            title: "second".to_string(),
            name: "01.md".to_string(),
            ..Slide::default()
        });
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.text_scroll = 5;

        app.next_slide();

        assert_eq!(app.current, 1);
        assert_eq!(app.text_scroll, 0);
    }

    #[test]
    fn arrow_navigation_escapes_search_focus() {
        let mut deck = sample_deck();
        deck.slides.push(Slide {
            id: 1,
            title: "second".to_string(),
            name: "01.md".to_string(),
            ..Slide::default()
        });
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.current = 1;
        app.search_focus = true;
        app.search = "term".to_string();

        let _ = app.handle_key(
            KeyEvent::new(KeyCode::Left, crossterm::event::KeyModifiers::NONE),
            &mut CrosstermBackend::new(io::stdout()),
        );

        assert_eq!(app.current, 0);
        assert!(!app.search_focus);
    }

    #[test]
    fn ctrl_c_exits_search_before_quitting() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.search_focus = true;
        app.search = "term".to_string();

        let should_quit = app
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL),
                &mut CrosstermBackend::new(io::stdout()),
            )
            .expect("handle ctrl-c");

        assert!(!should_quit);
        assert!(!app.search_focus);
        assert_eq!(app.search, "term");
    }

    #[test]
    fn ctrl_c_exits_command_before_quitting() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.command_focus = true;
        app.command = "reload".to_string();

        let should_quit = app
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL),
                &mut CrosstermBackend::new(io::stdout()),
            )
            .expect("handle ctrl-c");

        assert!(!should_quit);
        assert!(!app.command_focus);
        assert!(app.command.is_empty());
    }

    #[test]
    fn ctrl_c_exits_overlay_modes_before_quitting() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.help = true;

        let should_quit = app
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL),
                &mut CrosstermBackend::new(io::stdout()),
            )
            .expect("handle ctrl-c");

        assert!(!should_quit);
        assert!(!app.help);

        app.outline = true;
        let should_quit = app
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL),
                &mut CrosstermBackend::new(io::stdout()),
            )
            .expect("handle ctrl-c");

        assert!(!should_quit);
        assert!(!app.outline);
    }

    #[test]
    fn ctrl_c_quits_at_root_presentation_mode() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );

        let should_quit = app
            .handle_key(
                KeyEvent::new(KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL),
                &mut CrosstermBackend::new(io::stdout()),
            )
            .expect("handle ctrl-c");

        assert!(should_quit);
    }

    #[test]
    fn paste_appends_raw_text_to_command_buffer() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.command_focus = true;

        app.handle_paste("open ./slides".to_string());

        assert_eq!(app.command, "open ./slides");
    }

    #[test]
    fn normalize_csi_arrow_sequence_maps_to_arrow_key() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );

        let esc = app.normalize_key_event(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(
            esc,
            Some(KeyEvent {
                code: KeyCode::Esc,
                ..
            })
        ));

        let bracket = app.normalize_key_event(KeyEvent::new(
            KeyCode::Char('['),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(bracket.is_none());

        let arrow = app.normalize_key_event(KeyEvent::new(
            KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(
            arrow,
            Some(KeyEvent {
                code: KeyCode::Up,
                ..
            })
        ));
    }

    #[test]
    fn resolve_link_target_handles_relative_markdown_links() {
        let slide_path = PathBuf::from("slides/01_intro.md");
        let target = resolve_link_target(&slide_path, "./docs/reference.md#api");
        assert_eq!(
            target,
            LinkTarget::File(PathBuf::from("slides").join("docs/reference.md"))
        );
    }

    #[test]
    fn resolve_link_target_preserves_external_urls() {
        let slide_path = PathBuf::from("slides/01_intro.md");
        let target = resolve_link_target(&slide_path, "https://example.com/docs");
        assert_eq!(
            target,
            LinkTarget::External("https://example.com/docs".to_string())
        );
    }

    #[test]
    fn first_link_on_row_returns_first_link_for_line() {
        let deck = Deck {
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
                    content: vec![
                        crate::deck::model::Inline::Text("before ".to_string()),
                        crate::deck::model::Inline::Link {
                            text: "example".to_string(),
                            url: "https://example.com".to_string(),
                        },
                    ],
                })],
                ..Slide::default()
            }],
        };
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.body_rows = 4;
        app.text_rect = Some(Rect::new(10, 5, 30, 4));

        let link = app.first_link_on_row(0);

        assert_eq!(link, Some(("https://example.com".to_string(), 1)));
    }

    #[test]
    fn first_link_on_row_returns_none_without_links() {
        let mut app = App::new(
            PathBuf::from("."),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.body_rows = 4;
        app.text_rect = Some(Rect::new(10, 5, 30, 4));

        let link = app.first_link_on_row(0);

        assert_eq!(link, None);
    }

    #[test]
    fn command_open_loads_new_markdown_target() {
        let temp = tempdir().expect("tempdir");
        let slides_dir = temp.path().join("slides");
        fs::create_dir_all(&slides_dir).expect("create slides dir");
        fs::write(slides_dir.join("01_one.md"), "# One\n").expect("write first slide");
        fs::write(temp.path().join("single.md"), "# Single\n").expect("write single slide");

        let mut app = App::new(
            slides_dir.clone(),
            load_deck(&slides_dir).expect("load initial deck"),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );

        let mut stdout = CrosstermBackend::new(io::stdout());
        let should_quit = app
            .execute_command(
                &format!("open {}", temp.path().join("single.md").display()),
                &mut stdout,
            )
            .expect("execute command");

        assert!(!should_quit);
        assert_eq!(app.deck.slides.len(), 1);
        assert_eq!(app.deck.slides[0].name, "single.md");
    }

    #[test]
    fn command_path_reports_current_target() {
        let mut app = App::new(
            PathBuf::from("/tmp/slides"),
            sample_deck(),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );

        let mut stdout = CrosstermBackend::new(io::stdout());
        let should_quit = app
            .execute_command("path", &mut stdout)
            .expect("execute command");

        assert!(!should_quit);
        assert_eq!(app.status, "/tmp/slides");
    }

    #[test]
    fn vertical_cursor_skips_blank_rows() {
        let deck = Deck {
            root: PathBuf::from("."),
            metadata: DeckMetadata {
                title: "deck".to_string(),
            },
            slides: vec![Slide {
                id: 0,
                title: "title".to_string(),
                name: "00.md".to_string(),
                blocks: vec![
                    Block::Paragraph(ParagraphBlock {
                        id: 0,
                        content: vec![crate::deck::model::Inline::Text("hello".to_string())],
                    }),
                    Block::Paragraph(ParagraphBlock {
                        id: 1,
                        content: vec![crate::deck::model::Inline::Text("world".to_string())],
                    }),
                ],
                ..Slide::default()
            }],
        };
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.body_rows = 4;
        app.text_rect = Some(Rect::new(0, 0, 40, 4));

        app.move_line_cursor(1);

        assert_eq!(app.line_cursor, 2);
    }

    #[test]
    fn visual_cursor_skips_blank_rows() {
        let deck = Deck {
            root: PathBuf::from("."),
            metadata: DeckMetadata {
                title: "deck".to_string(),
            },
            slides: vec![Slide {
                id: 0,
                title: "title".to_string(),
                name: "00.md".to_string(),
                blocks: vec![
                    Block::Paragraph(ParagraphBlock {
                        id: 0,
                        content: vec![crate::deck::model::Inline::Text("hello".to_string())],
                    }),
                    Block::Paragraph(ParagraphBlock {
                        id: 1,
                        content: vec![crate::deck::model::Inline::Text("world".to_string())],
                    }),
                ],
                ..Slide::default()
            }],
        };
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.body_rows = 4;
        app.text_rect = Some(Rect::new(0, 0, 40, 4));
        app.visual_anchor = Some(0);
        app.visual_cursor = 0;

        app.move_visual_cursor(1);

        assert_eq!(app.visual_cursor, 2);
    }

    #[test]
    fn bracket_motions_jump_between_headings_within_current_slide() {
        let deck = Deck {
            root: PathBuf::from("."),
            metadata: DeckMetadata {
                title: "deck".to_string(),
            },
            slides: vec![Slide {
                id: 0,
                title: "title".to_string(),
                name: "00.md".to_string(),
                blocks: vec![
                    Block::Heading(HeadingBlock {
                        id: 0,
                        level: 1,
                        content: vec![Inline::Text("Intro".to_string())],
                    }),
                    Block::Paragraph(ParagraphBlock {
                        id: 1,
                        content: vec![Inline::Text("alpha".to_string())],
                    }),
                    Block::Heading(HeadingBlock {
                        id: 2,
                        level: 2,
                        content: vec![Inline::Text("Details".to_string())],
                    }),
                    Block::Paragraph(ParagraphBlock {
                        id: 3,
                        content: vec![Inline::Text("beta".to_string())],
                    }),
                    Block::Heading(HeadingBlock {
                        id: 4,
                        level: 3,
                        content: vec![Inline::Text("Deep Dive".to_string())],
                    }),
                ],
                ..Slide::default()
            }],
        };
        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.body_rows = 12;
        app.text_rect = Some(Rect::new(0, 0, 40, 12));

        app.jump_heading(1);
        assert_eq!(app.line_cursor, 1);

        app.jump_heading(1);
        assert_eq!(app.line_cursor, 6);

        app.jump_heading(-1);
        assert_eq!(app.line_cursor, 1);
    }

    #[test]
    fn bracket_motion_does_not_change_slides() {
        let mut deck = sample_deck();
        deck.slides.push(Slide {
            id: 1,
            title: "second".to_string(),
            name: "01.md".to_string(),
            ..Slide::default()
        });
        deck.slides[0].blocks = vec![Block::Heading(HeadingBlock {
            id: 0,
            level: 1,
            content: vec![Inline::Text("Only heading".to_string())],
        })];

        let mut app = App::new(
            PathBuf::from("."),
            deck,
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );
        app.body_rows = 8;
        app.text_rect = Some(Rect::new(0, 0, 40, 8));

        app.jump_heading(1);

        assert_eq!(app.current, 0);
    }

    #[test]
    fn command_o_alias_loads_new_markdown_target() {
        let temp = tempdir().expect("tempdir");
        let slides_dir = temp.path().join("slides");
        fs::create_dir_all(&slides_dir).expect("create slides dir");
        fs::write(slides_dir.join("01_one.md"), "# One\n").expect("write first slide");
        fs::write(temp.path().join("single.md"), "# Single\n").expect("write single slide");

        let mut app = App::new(
            slides_dir.clone(),
            load_deck(&slides_dir).expect("load initial deck"),
            TmuxRuntime::default(),
            Box::new(NoopBackend),
            false,
        );

        let mut stdout = CrosstermBackend::new(io::stdout());
        let should_quit = app
            .execute_command(
                &format!("o {}", temp.path().join("single.md").display()),
                &mut stdout,
            )
            .expect("execute command");

        assert!(!should_quit);
        assert_eq!(app.deck.slides.len(), 1);
        assert_eq!(app.deck.slides[0].name, "single.md");
    }

    #[test]
    fn watched_paths_follow_loaded_markdown_files() {
        let temp = tempdir().expect("tempdir");
        let slides_dir = temp.path().join("slides");
        fs::create_dir_all(&slides_dir).expect("create slides dir");
        fs::write(slides_dir.join("01_one.md"), "# One\n").expect("write first slide");
        fs::write(slides_dir.join("02_two.md"), "# Two\n").expect("write second slide");

        let deck = load_deck(&slides_dir).expect("load deck");
        let watched = watched_paths(&slides_dir, &deck);

        assert_eq!(watched.len(), 2);
        assert!(
            watched
                .iter()
                .all(|path| path.extension().is_some_and(|ext| ext == "md"))
        );
    }
}
