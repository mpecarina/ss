use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;

use crate::images::{build_placement, detect_backend, ImageBackend};
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
    outline_filtered: Vec<usize>,
    outline_index: usize,
    outline_scroll: usize,
    tmux: TmuxContext,
    image_backend: Box<dyn ImageBackend>,
    images_visible: bool,
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
            outline_filtered: Vec::new(),
            outline_index: 0,
            outline_scroll: 0,
            tmux,
            image_backend,
            images_visible: false,
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
            KeyCode::Char('r') => {
                self.clear_images(stdout)?;
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
                    self.clear_images(stdout)?;
                    self.current = 0;
                    self.pending_g = false;
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
                    self.clear_images(stdout)?;
                    self.current = self.slides.len().saturating_sub(1);
                    self.refresh_matches();
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('l') | KeyCode::Char(' ') => {
                self.pending_g = false;
                if self.outline {
                    if !self.outline_filtered.is_empty() {
                        self.clear_images(stdout)?;
                        self.current = self.outline_filtered[self.outline_index.min(self.outline_filtered.len() - 1)];
                        self.outline = false;
                        self.refresh_matches();
                    }
                } else if self.current + 1 < self.slides.len() {
                    self.clear_images(stdout)?;
                    self.current += 1;
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
                    self.clear_images(stdout)?;
                    self.current -= 1;
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
            self.draw_help(frame);
            return;
        }
        if self.outline {
            self.draw_outline(frame);
            return;
        }

        let size = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(size);

        let slide = &self.slides[self.current];
        let lines = self.highlighted_lines(&slide.content);
        let body = Paragraph::new(lines)
            .block(Block::default().title(slide.title.clone()).borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        frame.render_widget(body, chunks[0]);
        frame.render_widget(self.footer(slide), chunks[1]);
    }

    fn draw_outline(&mut self, frame: &mut ratatui::Frame) {
        let size = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(2)])
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
            let style = if absolute == self.outline_index {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("[{:<2}] {} | {}", slide_index + 1, slide.title, slide.name),
                style,
            )));
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled("no slides matched", Style::default().fg(Color::DarkGray))));
        }
        frame.render_widget(Paragraph::new(lines), chunks[1]);
        frame.render_widget(Paragraph::new("/ filter  enter open  j/k move  esc close  q quit"), chunks[2]);
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
        let block = Paragraph::new(text).block(Block::default().title("Help").borders(Borders::ALL));
        frame.render_widget(block, frame.area());
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
            Span::raw(slide.name.clone()),
            Span::raw("  "),
            Span::styled(format!("backend:{}", self.image_backend.name()), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(search_status, Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(self.status.clone(), Style::default().fg(Color::DarkGray)),
        ]);
        Paragraph::new(line)
    }

    fn highlighted_lines(&self, text: &str) -> Vec<Line<'static>> {
        if self.search.is_empty() || self.matches.is_empty() {
            return text.lines().map(|line| Line::from(line.to_string())).collect();
        }

        let mut spans = Vec::new();
        let mut start = 0;
        for (index, (match_start, match_end)) in self.matches.iter().enumerate() {
            if *match_start > start {
                spans.push(Span::raw(text[start..*match_start].to_string()));
            }
            let style = if index == self.match_index {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default().add_modifier(Modifier::REVERSED)
            };
            spans.push(Span::styled(text[*match_start..*match_end].to_string(), style));
            start = *match_end;
        }
        if start < text.len() {
            spans.push(Span::raw(text[start..].to_string()));
        }

        vec![Line::from(spans)]
    }

    fn refresh_matches(&mut self) {
        self.matches = find_matches(&self.slides[self.current].content, &self.search);
        if self.matches.is_empty() {
            self.match_index = 0;
        } else if self.match_index >= self.matches.len() {
            self.match_index = 0;
        }
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
            return Ok(());
        }
        let slide = &self.slides[self.current];
        let Some(placement) = build_placement(
            slide.images.first().unwrap_or(&Default::default()),
            100,
            30,
        ) else {
            return Ok(());
        };

        stdout.execute(crossterm::style::Print(self.image_backend.draw_sequence(&[placement])))?;
        stdout.flush()?;
        self.images_visible = true;
        Ok(())
    }

    fn poll_tmux_focus(&mut self, stdout: &mut CrosstermBackend<Stdout>) -> Result<()> {
        if self.last_focus_poll.elapsed() < Duration::from_millis(200) {
            return Ok(());
        }
        self.last_focus_poll = Instant::now();
        if self.images_visible && !self.tmux_visible() {
            self.clear_images(stdout)?;
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
        Ok(())
    }
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
}
