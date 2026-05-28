mod app;
mod images;
mod markdown;
mod slides;
mod tmux;

fn main() -> anyhow::Result<()> {
    app::run()
}
