mod app;
mod images;
mod slides;
mod tmux;

fn main() -> anyhow::Result<()> {
    app::run()
}
