mod app;
mod deck;
mod graphics;
mod layout;
mod markdown;
mod tmux;

fn main() -> anyhow::Result<()> {
    app::run()
}
